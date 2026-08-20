//! Rainy-reunion 3-turn Mock end-to-end slice (ST-020/021 integration gate).
//!
//! Loads the committed `stories/rainy-reunion` Pack, drives the engine with a
//! [`MockProvider`] through the opening + 3 player turns, and pins the W5
//! invariants against real Pack rules:
//!
//! - turn advances by exactly 1 per accepted generation;
//! - only validated patches commit (affection accumulates; checkpoint restored
//!   on failure);
//! - new choices are presented each turn;
//! - the committed stable state is never corrupted.
//!
//! Plus two control cases: cancel mid-generation restores the checkpoint, and
//! a malformed (rule-violating) draft surfaces a recoverable error with the
//! stable state intact.

use std::path::PathBuf;

use story_core::engine::{EngineState, reduce};
use story_core::engine_cmd::{EngineCommand, EngineEffect};
use story_model::StoryPack;
use story_model::{CharacterId, Choice, GeneratedTurnDraft, RiskLevel, StatePatch, TurnSummary};
use story_persistence::PackLoader;
use story_provider::{LlmProvider, MockProvider, MockResponse};
use tokio_util::sync::CancellationToken;

const YUNA: &str = "yuna";

fn yuna() -> CharacterId {
    CharacterId(YUNA.to_string())
}

fn rainy_reunion_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `apps/storytui`; the workspace root is two levels
    // up.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent() // apps/
        .unwrap()
        .parent() // workspace root
        .unwrap()
        .join("stories")
        .join("rainy-reunion")
}

/// Build a draft whose `affection_delta` for yuna is a small legal delta that
/// passes the W3 validator against rainy-reunion's rules (`-3..=3` per turn,
/// projected stat within `-30..=30`).
fn legal_draft(narration: &str, affection_delta: i16) -> GeneratedTurnDraft {
    let mut patch = StatePatch::default();
    patch.affection_delta.insert(yuna(), affection_delta);
    draft_with_patch(narration, patch)
}

/// Build a draft whose patch references an unknown flag → rejected by the W3
/// validator (`patch_unknown_flag`).
fn malformed_draft(narration: &str) -> GeneratedTurnDraft {
    let mut patch = StatePatch::default();
    patch
        .set_flags
        .insert("__nonexistent_flag__".to_string(), true);
    draft_with_patch(narration, patch)
}

fn draft_with_patch(narration: &str, patch: StatePatch) -> GeneratedTurnDraft {
    GeneratedTurnDraft {
        // The host stamps the echoed generation_id after the provider call;
        // the value here is a placeholder.
        generation_id: story_model::uuid::Uuid::nil(),
        narration: narration.to_string(),
        choices: vec![
            choice("choice-0", "조용히 말을 건넨다", RiskLevel::Low),
            choice("choice-1", "잠시 기다린다", RiskLevel::Safe),
            choice("choice-2", "단도직입적으로 묻는다", RiskLevel::Medium),
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

/// Pull the RequestGeneration effect from a transition (there is exactly one
/// per StartNewGame / SubmitChoice).
fn request_effect(t: &story_core::engine::Transition) -> &EngineEffect {
    t.effects
        .iter()
        .find(|e| matches!(e, EngineEffect::RequestGeneration { .. }))
        .expect("a RequestGeneration effect was emitted")
}

fn request_generation_id(e: &EngineEffect) -> story_model::uuid::Uuid {
    match e {
        EngineEffect::RequestGeneration { generation_id, .. } => *generation_id,
        _ => panic!("not a RequestGeneration: {e:?}"),
    }
}

fn request_turn_request(e: &EngineEffect) -> story_model::TurnRequest {
    match e {
        EngineEffect::RequestGeneration { request, .. } => request.clone(),
        _ => panic!("not a RequestGeneration: {e:?}"),
    }
}

/// Drive one generation to completion: call the provider, stamp the echoed
/// generation_id (the real provider echoes it from the request on the wire),
/// and feed AcceptGeneration.
async fn drive_generation(
    state: &EngineState,
    req_effect: &EngineEffect,
    provider: &MockProvider,
    pack: &StoryPack,
) -> story_core::engine::Transition {
    let gid = request_generation_id(req_effect);
    let request = request_turn_request(req_effect);
    let cancel = CancellationToken::new();
    let mut draft = provider
        .generate_turn(request, cancel)
        .await
        .expect("mock provider generate_turn");
    draft.generation_id = gid; // echo — real provider does this on the wire
    reduce(
        state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(draft),
        },
        pack,
    )
    .expect("AcceptGeneration")
}

fn load_pack() -> StoryPack {
    let root = rainy_reunion_root();
    assert!(root.is_dir(), "stories/rainy-reunion not found at {root:?}");
    PackLoader::load(&root).unwrap_or_else(|e| panic!("rainy-reunion failed to load: {e}"))
}

fn build_provider_with_drafts(drafts: Vec<GeneratedTurnDraft>) -> MockProvider {
    let mut provider = MockProvider::new("mock-e2e");
    for d in drafts {
        provider = provider.with(MockResponse::Draft(d));
    }
    provider
}

// ---------------------------------------------------------------------------
// happy path: opening + 3 player turns
// ---------------------------------------------------------------------------

#[test]
fn rainy_reunion_initial_state_uses_pack_defaults_and_known_facts() {
    let pack = load_pack();
    let state = EngineState::initial(&pack);
    let yuna_state = state.game.characters.get(&yuna()).expect("yuna state");

    assert_eq!(yuna_state.affection, 0);
    assert_eq!(yuna_state.trust, 0);
    assert_eq!(yuna_state.tension, 8);
    assert_eq!(pack.rules.tension_stat_limits().min, 0);
    assert_eq!(pack.rules.tension_stat_limits().max, 30);
    assert_eq!(state.game.numbers.get("yuna.tension"), Some(&8));
    assert!(
        yuna_state
            .known_facts
            .contains("카페 나무는 두 사람이 대학 시절 자주 오던 곳이다")
    );
}

#[tokio::test]
async fn rainy_reunion_three_turn_happy_path() {
    let pack = load_pack();
    // Opening + 3 follow-up turns = 4 drafts.
    let provider = build_provider_with_drafts(vec![
        legal_draft("오프닝: 유나가 빈 의자를 가리킨다.", 1),
        legal_draft("턴1: 유나가 시선을 내린다.", 1),
        legal_draft("턴2: 유나가 고개를 끄덕인다.", 1),
        legal_draft("턴3: 유나가 말을 잇는다.", 1),
    ]);

    // StartNewGame → opening RequestGeneration.
    let init = EngineState::initial(&pack);
    let start = reduce(&init, EngineCommand::StartNewGame, &pack).expect("start");
    let req = request_effect(&start).clone();
    let mut state = start.state;
    assert!(state.pending.is_some());
    assert_eq!(state.game.turn, 0);

    // Opening accept → turn 1, affection 1.
    let t = drive_generation(&state, &req, &provider, &pack).await;
    state = t.state;
    assert_eq!(state.game.turn, 1, "opening → turn 1");
    assert_eq!(
        state.game.characters.get(&yuna()).unwrap().affection,
        1,
        "opening patch committed"
    );
    assert_eq!(
        state.game.history.len(),
        1,
        "one history entry after opening"
    );
    assert_eq!(
        state.game.history.last().unwrap().choices.len(),
        3,
        "opening offers 3 choices"
    );

    // Three follow-up turns.
    let mut expected_affection = 1i16;
    for turn in 1..=3u32 {
        let before_revision = state.game.revision;
        let before_history = state.game.history.len();

        let submit = reduce(
            &state,
            EngineCommand::SubmitChoice {
                choice_id: "choice-0".to_string(),
            },
            &pack,
        )
        .expect("submit");
        let req = request_effect(&submit).clone();
        state = submit.state;
        assert!(
            state.pending.is_some(),
            "pending set during turn {}",
            turn + 1
        );

        let t = drive_generation(&state, &req, &provider, &pack).await;
        state = t.state;

        expected_affection += 1;
        assert_eq!(state.game.turn, turn + 1, "turn advanced to {}", turn + 1);
        assert_eq!(
            state.game.characters.get(&yuna()).unwrap().affection,
            expected_affection,
            "affection accumulated on turn {}",
            turn + 1
        );
        assert_eq!(
            state.game.history.len(),
            before_history + 1,
            "history grew by exactly one"
        );
        assert_eq!(
            state.game.revision,
            before_revision + 1,
            "revision bumped exactly once"
        );
        assert!(state.pending.is_none(), "pending cleared after accept");
        assert_eq!(
            state.game.history.last().unwrap().choices.len(),
            3,
            "new choices presented"
        );
    }

    // Final: 4 history entries (opening + 3 turns), turn 4, affection 4.
    assert_eq!(state.game.turn, 4);
    assert_eq!(state.game.characters.get(&yuna()).unwrap().affection, 4);
    assert_eq!(state.game.history.len(), 4);
    assert_eq!(
        state.game.last_choice.as_ref().unwrap().choice_id,
        "choice-0"
    );
}

// ---------------------------------------------------------------------------
// cancel mid-generation restores checkpoint
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cancel_mid_generation_restores_checkpoint() {
    let pack = load_pack();
    let provider = build_provider_with_drafts(vec![legal_draft("오프닝.", 1)]);

    let init = EngineState::initial(&pack);
    let start = reduce(&init, EngineCommand::StartNewGame, &pack).expect("start");
    let req = request_effect(&start).clone();
    let mut state = start.state;

    // Get to turn 1 so the checkpoint has real content.
    let t = drive_generation(&state, &req, &provider, &pack).await;
    state = t.state;
    assert_eq!(state.game.turn, 1);

    let checkpoint_snapshot = story_model::serde_json::to_vec(&state.game).unwrap();

    // SubmitChoice → pending with checkpoint = turn-1 state.
    let submit = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let pending_gid = submit.state.pending.as_ref().unwrap().generation_id;
    state = submit.state;
    assert!(state.pending.is_some());

    // Cancel → checkpoint restored.
    let cancel = reduce(
        &state,
        EngineCommand::CancelGeneration {
            generation_id: pending_gid,
        },
        &pack,
    )
    .expect("cancel");
    let restored = story_model::serde_json::to_vec(&cancel.state.game).unwrap();
    assert_eq!(
        restored, checkpoint_snapshot,
        "cancel restored the checkpoint byte-identically"
    );
    assert!(cancel.state.pending.is_none(), "pending cleared");
}

// ---------------------------------------------------------------------------
// malformed draft → ShowRecoverableError, stable state intact
// ---------------------------------------------------------------------------

#[tokio::test]
async fn malformed_draft_surfaces_error_and_keeps_state_intact() {
    let pack = load_pack();
    let provider = build_provider_with_drafts(vec![
        legal_draft("오프닝.", 1),
        malformed_draft("이 장면은 버려져야 한다."),
    ]);

    let init = EngineState::initial(&pack);
    let start = reduce(&init, EngineCommand::StartNewGame, &pack).expect("start");
    let req = request_effect(&start).clone();
    let mut state = start.state;

    // Opening accept → turn 1.
    let t = drive_generation(&state, &req, &provider, &pack).await;
    state = t.state;
    assert_eq!(state.game.turn, 1);

    let checkpoint_snapshot = story_model::serde_json::to_vec(&state.game).unwrap();

    // SubmitChoice → pending.
    let submit = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let req = request_effect(&submit).clone();
    state = submit.state;

    // Drive the (malformed) generation.
    let gid = request_generation_id(&req);
    let request = request_turn_request(&req);
    let cancel = CancellationToken::new();
    let mut draft = provider
        .generate_turn(request, cancel)
        .await
        .expect("mock generate");
    draft.generation_id = gid;

    let t = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(draft),
        },
        &pack,
    )
    .expect("failed accept returns a Transition (with error effect)");

    // Stable state byte-identical to the checkpoint.
    let restored = story_model::serde_json::to_vec(&t.state.game).unwrap();
    assert_eq!(
        restored, checkpoint_snapshot,
        "malformed draft must not corrupt the committed stable state"
    );
    assert!(t.state.pending.is_none(), "pending cleared on failure");

    // A ShowRecoverableError effect was emitted alongside a fresh view.
    let has_error = t
        .effects
        .iter()
        .any(|e| matches!(e, EngineEffect::ShowRecoverableError(_)));
    assert!(has_error, "expected ShowRecoverableError effect");
    let err_view = t
        .effects
        .iter()
        .find_map(|e| match e {
            EngineEffect::ShowRecoverableError(v) => Some(v),
            _ => None,
        })
        .unwrap();
    assert_eq!(err_view.code, "patch_unknown_flag");
    assert!(err_view.recoverable);

    // History length and turn unchanged: no partial commit.
    assert_eq!(t.state.game.turn, 1);
    assert_eq!(t.state.game.history.len(), 1);
}
