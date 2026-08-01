//! Deterministic benchmark runner (ST-036).
//!
//! Drives the engine through a fixed prompt bundle against a deterministic
//! [`MockProvider`], recording per-turn: prompt hash (over the rendered
//! system+user messages), sampling parameters, result patch digest
//! (SHA-256 of the proposed-patch JSON), and wall-clock time.
//!
//! # Determinism contract
//!
//! Given the same `StoryPack` + [`BenchConfig`], [`run_deterministic_bench`]
//! produces byte-identical `prompt_hash` and `patch_digest` vectors across
//! runs. Wall time is recorded but is NOT part of the determinism contract
//! (it depends on host scheduling). See `tests/bench.rs` for the
//! reproducibility gate.
//!
//! # Real-provider gate
//!
//! This runner uses the in-process [`MockProvider`] — no network, no real
//! model. A `#[ignore]` real-provider run is the operator's responsibility
//! (documented in `docs/RELEASE-RUNTIME-GATE.md`).

use std::time::Instant;

use sha2::{Digest, Sha256};
use story_core::engine::{EngineState, Transition, reduce};
use story_core::engine_cmd::{EngineCommand, EngineEffect};
use story_model::chrono::Utc;
use story_model::{
    CharacterId, Choice, GeneratedTurnDraft, RiskLevel, StoryPack, TurnRequest, TurnSummary,
};
use story_provider::{LlmProvider, MockProvider, MockResponse};
use tokio_util::sync::CancellationToken;

/// Per-benchmark configuration. The defaults mirror the operator TOML
/// (`temperature` 0.85, `top_p` 0.92, `top_k` 40, `max_tokens` 1800) so a
/// real-provider run is a drop-in replacement.
#[derive(Clone, Debug)]
pub struct BenchConfig {
    /// Number of player turns to drive (excluding the opening).
    pub turns: usize,
    /// Sampling temperature recorded per turn.
    pub temperature: f32,
    /// Nucleus sampling probability mass.
    pub top_p: f32,
    /// Top-k sampling cutoff.
    pub top_k: u32,
    /// Maximum output tokens per turn.
    pub max_tokens: u32,
}

impl Default for BenchConfig {
    fn default() -> Self {
        Self {
            turns: 10,
            temperature: 0.85,
            top_p: 0.92,
            top_k: 40,
            max_tokens: 1800,
        }
    }
}

/// One recorded turn.
#[derive(Clone, Debug)]
pub struct TurnRecord {
    /// 1-indexed turn counter (1 == opening accept).
    pub turn: u32,
    /// SHA-256 over the rendered `role\x00content\x00` runs of every message
    /// in the provider request. Same prompt ⇒ same hash.
    pub prompt_hash: String,
    /// SHA-256 over the pretty-printed JSON of the model's `proposed_patch`.
    /// Same patch ⇒ same digest.
    pub patch_digest: String,
    /// Sampling temperature recorded for this turn (mirrors [`BenchConfig`]).
    pub temperature: f32,
    /// `top_p` recorded for this turn.
    pub top_p: f32,
    /// `top_k` recorded for this turn.
    pub top_k: u32,
    /// `max_tokens` recorded for this turn.
    pub max_tokens: u32,
    /// Wall-clock time the provider call took, in milliseconds. NOT part of
    /// the determinism contract.
    pub wall_ms: u64,
    /// First 60 chars of the narration, for human inspection of the report.
    pub narration_excerpt: String,
}

/// The full benchmark report. Serialize-and-redact before attaching to a bug
/// report; the report itself never carries an API key.
#[derive(Clone, Debug)]
pub struct BenchReport {
    /// The config this run used.
    pub config: BenchConfig,
    /// One record per accepted turn (opening + each player turn).
    pub turns: Vec<TurnRecord>,
    /// Final committed turn counter.
    pub final_turn: u32,
    /// RFC-3339 timestamp the run finished.
    pub generated_at: String,
    /// Human-readable note describing the seed / provider substitution.
    pub note: String,
}

/// Run a deterministic benchmark: opening + `config.turns` player turns against
/// a scripted [`MockProvider`], recording per-turn prompt hash + patch digest.
///
/// Each player turn submits the first offered choice (`choice-0`); the Mock
/// returns a scripted draft that bumps affection for the primary character by
/// +1 each turn (legal under rainy-reunion's `-3..=3` per-turn delta limit).
pub fn run_deterministic_bench(pack: &StoryPack, config: &BenchConfig) -> BenchReport {
    // Block on the async provider calls. The Mock is in-process and never
    // blocks on IO, so this does not spawn a real runtime load.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("tokio runtime for bench");

    rt.block_on(async move { run_deterministic_bench_async(pack, config).await })
}

async fn run_deterministic_bench_async(pack: &StoryPack, config: &BenchConfig) -> BenchReport {
    let primary = primary_character_id(pack);
    let mut provider = MockProvider::new("bench-mock");
    // Script opening + N player-turn drafts. Each draft is a function of the
    // *turn index* (so the narration differs turn-to-turn, exercising the
    // prompt hash), but the patch is constant (+1 affection) so the patch
    // digest is reproducible.
    let total_drafts = config.turns + 1;
    for i in 0..total_drafts {
        provider = provider.with(MockResponse::Draft(deterministic_draft(
            i as u32,
            primary.clone(),
        )));
    }

    let mut turns: Vec<TurnRecord> = Vec::with_capacity(total_drafts);
    let mut state = EngineState::initial(pack);

    // Opening turn.
    let mut last_transition = reduce(&state, EngineCommand::StartNewGame, pack).expect("start");
    let (opening_record, post_state) =
        drive_pending(&last_transition, &provider, pack, config).await;
    state = post_state;
    turns.push(opening_record);

    // Player turns: submit the first published choice each time.
    for i in 0..config.turns {
        // Resolve the choice id from the most recent published view on the
        // prior transition (opening's `start` or the previous submit).
        let choice_id = current_first_choice_id(&last_transition);
        let submit = reduce(
            &state,
            EngineCommand::SubmitChoice {
                choice_id: choice_id.clone(),
            },
            pack,
        )
        .expect("SubmitChoice");
        let (record, post_state) = drive_pending(&submit, &provider, pack, config).await;
        state = post_state;
        last_transition = submit;
        let _ = i;
        turns.push(record);
    }

    let final_turn = state.game.turn;
    BenchReport {
        config: config.clone(),
        turns,
        final_turn,
        generated_at: Utc::now().to_rfc3339(),
        note: format!(
            "Deterministic Mock bench — no real model invoked. Replace the provider with a \
             real MLX/OpenAI-compatible server and re-run with the same StoryPack + BenchConfig \
             to compare prompt hashes against a live model. Sampling temp={:.2} top_p={:.2} \
             top_k={} max_tokens={}.",
            config.temperature, config.top_p, config.top_k, config.max_tokens
        ),
    }
}

async fn drive_pending(
    transition: &Transition,
    provider: &MockProvider,
    pack: &StoryPack,
    config: &BenchConfig,
) -> (TurnRecord, EngineState) {
    let req_effect = transition
        .effects
        .iter()
        .find(|e| matches!(e, EngineEffect::RequestGeneration { .. }))
        .expect("RequestGeneration emitted");
    let (request, gid) = match req_effect {
        EngineEffect::RequestGeneration {
            generation_id,
            request,
        } => (request.clone(), *generation_id),
        _ => unreachable!(),
    };

    let prompt_hash = prompt_hash(&request);

    let started = Instant::now();
    let cancel = CancellationToken::new();
    let mut draft = provider
        .generate_turn(request, cancel)
        .await
        .expect("mock generate_turn");
    let wall_ms = started.elapsed().as_millis() as u64;

    let patch_digest = patch_digest(&draft.proposed_patch);
    let narration_excerpt = draft.narration.chars().take(60).collect();

    // Stamp the echoed generation id (real provider does this on the wire).
    draft.generation_id = gid;
    let accept = reduce(
        &transition.state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(draft),
        },
        pack,
    )
    .expect("AcceptGeneration");
    let turn_index = accept.state.game.turn;

    let record = TurnRecord {
        turn: turn_index,
        prompt_hash,
        patch_digest,
        temperature: config.temperature,
        top_p: config.top_p,
        top_k: config.top_k,
        max_tokens: config.max_tokens,
        wall_ms,
        narration_excerpt,
    };
    (record, accept.state)
}

fn current_first_choice_id(transition: &Transition) -> String {
    // Choices live on the published view model — pull the first one from the
    // most recent PublishView effect in this transition.
    for e in &transition.effects {
        if let EngineEffect::PublishView(view) = e
            && let Some(first) = view.choices.first()
        {
            return first.id.clone();
        }
    }
    // Fall back to a deterministic placeholder (no view published yet).
    "choice-0".to_string()
}

fn primary_character_id(pack: &StoryPack) -> CharacterId {
    pack.characters
        .keys()
        .next()
        .cloned()
        .unwrap_or_else(|| CharacterId("__primary__".to_string()))
}

/// The scripted Mock draft for turn `i`. Narration varies (so the prompt hash
/// changes as history grows); the patch is constant (so the patch digest is
/// reproducible).
fn deterministic_draft(i: u32, primary: CharacterId) -> GeneratedTurnDraft {
    let mut patch = story_model::StatePatch::default();
    patch.affection_delta.insert(primary, 1);
    GeneratedTurnDraft {
        generation_id: story_model::uuid::Uuid::nil(),
        narration: format!("벤치 턴 {i}: 유나가 고개를 끄덕인다."),
        choices: vec![
            choice("choice-0", "조용히 기다린다", RiskLevel::Low),
            choice("choice-1", "말을 건넨다", RiskLevel::Medium),
            choice("choice-2", "시선을 피한다", RiskLevel::Safe),
        ],
        proposed_patch: patch,
        summary: TurnSummary {
            facts: vec![],
            unresolved: vec![],
            relationship_changes: vec![],
            knowledge_changes: vec![],
            interpretation: None,
            address_form_changes: vec![],
            important_player_actions: vec![],
            source_history_ids: vec![],
        },
        usage: None,
    }
}

fn choice(id: &str, label: &str, risk: RiskLevel) -> Choice {
    Choice {
        id: id.to_string(),
        label: label.to_string(),
        intent: String::new(),
        tone: None,
        target: None,
        risk,
        hidden_requirements: vec![],
        preview: None,
    }
}

fn prompt_hash(req: &TurnRequest) -> String {
    let mut h = Sha256::new();
    for m in &req.messages {
        h.update(m.role.as_bytes());
        h.update(b"\x00");
        h.update(m.content.as_bytes());
        h.update(b"\x00");
    }
    hex_lower(h.finalize().as_slice())
}

fn patch_digest(patch: &story_model::StatePatch) -> String {
    let json = serde_json::to_string_pretty(patch).expect("StatePatch serializes");
    let mut h = Sha256::new();
    h.update(json.as_bytes());
    hex_lower(h.finalize().as_slice())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}
