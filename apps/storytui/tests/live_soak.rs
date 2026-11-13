//! Live multi-turn gameplay soak test against a local OpenAI-compatible server.
//!
//! Gated behind `#[ignore]` and `STORYTUI_LIVE_URL` so normal `cargo test` runs
//! stay offline and deterministic.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use story_core::engine::{EngineState, build_view};
use story_core::engine_cmd::EngineCommand;
use story_model::{ParsedAction, ProviderHealth, StoryPack};
use story_persistence::{PackLoader, SaveStore};
use story_provider::{LlmProvider, OpenAiCompatibleProvider, ProviderConfig, StructuredOutputMode};
use story_tts::{TtsConfig, TtsService};
use story_tui::App;
use tokio::sync::mpsc::unbounded_channel;
use tokio_util::sync::CancellationToken;

use storytui::config::{PlayerProfile, RuntimeConfig, UserSettings};
use storytui::host::{
    self, HostMessage, SceneImageStores, TurnAutomation, note_player_turn_command,
};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn load_pack(name: &str) -> StoryPack {
    let dir = repo_root().join("stories").join(name);
    PackLoader::load(&dir).unwrap_or_else(|e| panic!("failed to load {name}: {e}"))
}

fn live_config(url: &str, model: &str, save_dir: PathBuf, two_stage: bool) -> RuntimeConfig {
    let mut provider = ProviderConfig::defaults();
    provider.name = "local-live".to_string();
    provider.base_url = url.to_string();
    provider.model = model.to_string();
    provider.temperature = 0.75;
    provider.top_p = 0.92;
    provider.top_k = 40;
    provider.max_tokens = 4096;
    provider.min_narration_chars = 0;
    provider.request_timeout_ms = 120_000;
    provider.connect_timeout_ms = 5_000;
    provider.max_retries = 1;
    provider.structured_output = StructuredOutputMode::Auto;
    provider.stream_turns = true;
    // Weak local models: OpenAI penalties + Kobold/llama.cpp DRY extras.
    provider.frequency_penalty = 0.3;
    provider.presence_penalty = 0.15;
    provider.repetition_penalty = 1.05;
    provider.dry_multiplier = 0.8;
    provider.dry_base = 1.75;
    provider.dry_allowed_length = 2;
    provider.dry_penalty_range = 1024;
    provider.local_extra_samplers = true;

    RuntimeConfig {
        provider: provider.clone(),
        providers: vec![provider],
        director_model: None,
        choice_model: None,
        writer_model: None,
        stage_defaults: Default::default(),
        fallback_model: String::new(),
        tts: TtsConfig::default(),
        pack_dir: repo_root().join("stories").join("rainy-reunion"),
        save_dir,
        player: PlayerProfile::default(),
        source_path: None,
        use_director_writer: two_stage,
        base_sampling: (0.75, 0.92, 40),
    }
}

struct LiveSession {
    pack: StoryPack,
    config: RuntimeConfig,
    provider_config: ProviderConfig,
    provider: Arc<dyn LlmProvider>,
    user_settings: UserSettings,
    save_store: SaveStore,
    tts: TtsService,
    scene_images: SceneImageStores,
    engine: EngineState,
    app: App,
    tx: tokio::sync::mpsc::UnboundedSender<HostMessage>,
    rx: tokio::sync::mpsc::UnboundedReceiver<HostMessage>,
    cancellation: Option<(story_model::uuid::Uuid, CancellationToken)>,
    automation: TurnAutomation,
    last_probe: Instant,
    save_dir: PathBuf,
    last_draft: Option<story_model::GeneratedTurnDraft>,
    quality_recovers: u32,
    quality_hard_fails: u32,
    /// Turn numbers (pre-recover) where a quality recoverable fired.
    quality_recover_at: Vec<u32>,
}

impl LiveSession {
    fn new(pack_name: &str, url: &str, model: &str, two_stage: bool) -> Self {
        let pack = load_pack(pack_name);
        let save_dir = std::env::temp_dir().join(format!(
            "storytui-live-soak-{pack_name}-{}",
            story_model::uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&save_dir).expect("save dir");
        let config = live_config(url, model, save_dir.clone(), two_stage);
        let provider_config = config.provider.clone();
        let provider: Arc<dyn LlmProvider> =
            Arc::new(OpenAiCompatibleProvider::new(provider_config.clone()).expect("provider"));
        let save_store = SaveStore::open(&save_dir).expect("save store");
        let tts = TtsService::new(config.tts.clone());
        let mut engine = EngineState::initial(&pack);
        engine.use_director_writer = two_stage;
        let app = App::new(build_view(&engine, &pack, None, None));
        let (tx, rx) = unbounded_channel();

        Self {
            pack,
            config,
            provider_config,
            provider,
            user_settings: UserSettings::default(),
            save_store,
            tts,
            scene_images: SceneImageStores::default(),
            engine,
            app,
            tx,
            rx,
            cancellation: None,
            automation: TurnAutomation::default(),
            last_probe: Instant::now(),
            save_dir,
            last_draft: None,
            quality_recovers: 0,
            quality_hard_fails: 0,
            quality_recover_at: Vec::new(),
        }
    }

    /// Write a human-readable quality report of committed history for review.
    fn write_quality_report(&self, path: &std::path::Path) -> std::io::Result<()> {
        use std::collections::BTreeMap;
        use std::fmt::Write as _;
        let history = &self.engine.game.history;
        let mut body = String::new();
        let chars: Vec<usize> = history
            .iter()
            .map(|e| e.narration.chars().count())
            .collect();
        let avg = if chars.is_empty() {
            0.0
        } else {
            chars.iter().sum::<usize>() as f64 / chars.len() as f64
        };
        let min = chars.iter().copied().min().unwrap_or(0);
        let max = chars.iter().copied().max().unwrap_or(0);

        // Pairwise near-duplicate + soft-issue rates over consecutive commits.
        let mut similar_pairs = 0usize;
        let mut soft_pairs = 0usize;
        for window in history.windows(2) {
            let report = story_core::narration_quality::assess_narration(
                &{
                    let mut g = self.engine.game.clone();
                    g.history = vec![window[0].clone()];
                    g
                },
                &window[1].narration,
                &window[1].applied_patch,
                None,
                true,
            );
            let hard = report.issues.iter().any(|i| {
                matches!(
                    i,
                    story_core::narration_quality::NarrationQualityIssue::ExactDuplicate { .. }
                        | story_core::narration_quality::NarrationQualityIssue::HighSimilarity { .. }
                        | story_core::narration_quality::NarrationQualityIssue::RepeatedDialogue { .. }
                )
            });
            let soft = report.issues.iter().any(|i| {
                matches!(
                    i,
                    story_core::narration_quality::NarrationQualityIssue::ReusedMotifs { .. }
                        | story_core::narration_quality::NarrationQualityIssue::StalledProgress
                )
            });
            if hard {
                similar_pairs += 1;
            }
            if soft {
                soft_pairs += 1;
            }
        }

        // Dialogue-line streak: identical fingerprints across consecutive turns.
        let mut dialogue_streak_max = 0usize;
        let mut dialogue_streak_runs = 0usize;
        let mut dialogue_shared_pairs = 0usize;
        {
            let mut run = 1usize;
            let mut prev: Option<std::collections::BTreeSet<String>> = None;
            for entry in history {
                let fps: std::collections::BTreeSet<String> =
                    story_core::narration_quality::dialogue_fingerprints(&entry.narration)
                        .into_iter()
                        .collect();
                if let Some(p) = &prev {
                    let shared = fps.intersection(p).count();
                    if shared > 0 {
                        dialogue_shared_pairs += 1;
                        run += 1;
                    } else {
                        if run >= 3 {
                            dialogue_streak_runs += 1;
                        }
                        dialogue_streak_max = dialogue_streak_max.max(run);
                        run = 1;
                    }
                }
                prev = Some(fps);
            }
            if run >= 3 {
                dialogue_streak_runs += 1;
            }
            dialogue_streak_max = dialogue_streak_max.max(run);
        }
        let pair_count = history.len().saturating_sub(1);
        let similar_pct = if pair_count == 0 {
            0.0
        } else {
            similar_pairs as f64 / pair_count as f64 * 100.0
        };
        let soft_pct = if pair_count == 0 {
            0.0
        } else {
            soft_pairs as f64 / pair_count as f64 * 100.0
        };

        // Whole-run motif document frequency (not just the 4-turn window).
        let mut motif_freq: BTreeMap<String, usize> = BTreeMap::new();
        for entry in history {
            for token in entry.narration.split(|c: char| !c.is_alphanumeric()) {
                let t = token.trim();
                if t.chars().count() >= 2 {
                    *motif_freq.entry(t.to_string()).or_default() += 1;
                }
            }
        }
        let mut ranked: Vec<_> = motif_freq.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ranked.retain(|(tok, n)| *n >= 8 && tok.chars().count() >= 2);
        let top_motifs: Vec<String> = ranked
            .into_iter()
            .take(20)
            .map(|(tok, n)| format!("{tok}×{n}"))
            .collect();

        // Location / time progression from patches.
        let mut loc_moves = 0usize;
        let mut time_moves = 0usize;
        let mut patch_empty = 0usize;
        for entry in history {
            let p = &entry.applied_patch;
            if p.next_location.is_some() {
                loc_moves += 1;
            }
            if p.next_time_slot.is_some() {
                time_moves += 1;
            }
            let empty = p.affection_delta.is_empty()
                && p.trust_delta.is_empty()
                && p.tension_delta.is_empty()
                && p.relationship_transitions.is_empty()
                && p.set_flags.is_empty()
                && p.number_delta.is_empty()
                && p.set_strings.is_empty()
                && p.inventory_delta.is_empty()
                && p.add_events.is_empty()
                && p.next_location.is_none()
                && p.next_time_slot.is_none()
                && p.proposed_route.is_none();
            if empty {
                patch_empty += 1;
            }
        }

        let recover_list = if self.quality_recover_at.is_empty() {
            "none".to_string()
        } else {
            self.quality_recover_at
                .iter()
                .map(|t| t.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };

        writeln!(
            &mut body,
            "# Live soak quality report\n\n\
             - pack turns committed: {}\n\
             - narration chars: avg={avg:.0} min={min} max={max}\n\
             - consecutive hard-similar pairs: {similar_pairs}/{pair_count} ({similar_pct:.1}%)\n\
             - consecutive soft-issue pairs (motif/stall): {soft_pairs}/{pair_count} ({soft_pct:.1}%)\n\
             - consecutive turns sharing ≥1 dialogue fingerprint: {dialogue_shared_pairs}/{pair_count}\n\
             - max consecutive dialogue-share streak: {dialogue_streak_max} (runs≥3: {dialogue_streak_runs})\n\
             - empty applied patches: {patch_empty}/{}\n\
             - location moves via patch: {loc_moves}, time-slot moves: {time_moves}\n\
             - manual [R] recovers during soak: {} (at turns: {recover_list})\n\
             - hard quality overlays that exhausted recover budget: {}\n\
             - top recurring tokens (freq≥8): {}\n",
            history.len(),
            history.len(),
            self.quality_recovers,
            self.quality_hard_fails,
            if top_motifs.is_empty() {
                "(none)".to_string()
            } else {
                top_motifs.join(", ")
            },
        )
        .unwrap();

        writeln!(
            &mut body,
            "## Sample narrations\n\n\
             (opening, every 5th turn, and the final three — ~700 chars each)\n"
        )
        .unwrap();
        let mut sample_idxs: Vec<usize> = Vec::new();
        if !history.is_empty() {
            sample_idxs.push(0);
            let mut t = 5usize;
            while t <= history.len() {
                sample_idxs.push(t - 1);
                t += 5;
            }
            for back in 1..=3 {
                if history.len() >= back {
                    sample_idxs.push(history.len() - back);
                }
            }
            sample_idxs.sort_unstable();
            sample_idxs.dedup();
        }
        for idx in sample_idxs {
            let entry = &history[idx];
            let preview: String = entry.narration.chars().take(700).collect();
            let truncated = if entry.narration.chars().count() > 700 {
                format!("{preview}…")
            } else {
                preview
            };
            let loc = entry
                .applied_patch
                .next_location
                .as_ref()
                .map(|l| l.0.as_str())
                .unwrap_or("-");
            let time = entry
                .applied_patch
                .next_time_slot
                .map(|t| format!("{t:?}"))
                .unwrap_or_else(|| "-".into());
            writeln!(
                &mut body,
                "### Turn {} (chars={}, next_loc={loc}, next_time={time})\n\n{}\n\n---\n",
                entry.turn,
                entry.narration.chars().count(),
                truncated
            )
            .unwrap();
        }

        // Full dump for deep review.
        let mut full = String::new();
        writeln!(&mut full, "# Full narration dump ({} turns)\n", history.len()).unwrap();
        for entry in history {
            writeln!(
                &mut full,
                "## Turn {}\n\n{}\n\n---\n",
                entry.turn, entry.narration
            )
            .unwrap();
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
            let dump = parent.join("live-quality-narrations-full.md");
            std::fs::write(&dump, full)?;
        }
        std::fs::write(path, body)
    }

    async fn run_command(&mut self, cmd: EngineCommand, label: &str) {
        // Weak local models often need several [R] regenerations for quality
        // gates on long soaks; allow a small budget before failing the soak.
        self.run_command_inner(cmd, label, 5).await;
    }

    async fn run_command_inner(&mut self, cmd: EngineCommand, label: &str, recovers_left: u32) {
        note_player_turn_command(&cmd, &mut self.automation);
        host::dispatch(
            cmd,
            &mut self.engine,
            &mut self.app,
            &self.pack,
            &self.provider,
            &self.config,
            &self.save_store,
            &self.tts,
            &mut self.scene_images,
            &self.tx,
            &mut self.cancellation,
        )
        .await
        .unwrap_or_else(|e| panic!("[{label}] dispatch failed: {e}"));

        let deadline = Instant::now() + Duration::from_secs(120);
        while self.app.view.loading.is_some()
            || self.engine.pending.is_some()
            || self.automation.retry_armed()
        {
            if Instant::now() >= deadline {
                panic!("[{label}] timed out waiting for turn completion");
            }
            tokio::select! {
                Some(msg) = self.rx.recv() => {
                    if let HostMessage::Generation { result: Ok(draft), .. } = &msg {
                        self.last_draft = Some(draft.clone());
                    }
                    host::handle_host_message(
                        msg,
                        &mut self.engine,
                        &mut self.app,
                        &self.pack,
                        &self.provider,
                        &self.config,
                        &self.save_store,
                        &self.tts,
                        &mut self.scene_images,
                        &self.tx,
                        &mut self.cancellation,
                        &mut self.automation,
                    )
                    .await
                    .unwrap_or_else(|e| panic!("[{label}] handle_host_message failed: {e}"));
                }
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    host::handle_frame_automation(
                        &mut self.engine,
                        &mut self.app,
                        &self.pack,
                        &mut self.provider_config,
                        &mut self.provider,
                        &mut self.user_settings,
                        &mut self.config,
                        &self.save_store,
                        &self.tts,
                        &mut self.scene_images,
                        &self.tx,
                        &mut self.cancellation,
                        &mut self.automation,
                        &mut self.last_probe,
                        Instant::now(),
                    )
                    .await
                    .unwrap_or_else(|e| panic!("[{label}] handle_frame_automation failed: {e}"));
                }
            }
            if let Some(err) = self.app.view.error.clone() {
                let motifs =
                    story_core::narration_quality::recurring_motifs(&self.engine.game.history);
                let engine_issues = self
                    .engine
                    .last_narration_quality
                    .as_ref()
                    .map(|r| format!("{:?}", r.issues))
                    .unwrap_or_else(|| "(none stored)".into());
                let draft_issues = self.last_draft.as_ref().map(|draft| {
                    story_core::narration_quality::assess_narration(
                        &self.engine.game,
                        &draft.narration,
                        &draft.proposed_patch,
                        Some(&draft.summary),
                        true,
                    )
                    .issues
                });
                eprintln!(
                    "[{label}] diagnostic: pressure={}, engine_issues={engine_issues}, draft_issues={draft_issues:?}, motifs={motifs:?}",
                    self.engine.quality_pressure
                );
                if err.recoverable && recovers_left > 0 && self.engine.retry_pending.is_some() {
                    if err.code.contains("quality") || err.code.contains("narration") {
                        self.quality_recovers += 1;
                        self.quality_recover_at.push(self.app.view.status.turn);
                    }
                    eprintln!(
                        "[{label}] recoverable error surfaced ({}: {}), testing [R] regenerate ({} left)",
                        err.code,
                        err.message_ko,
                        recovers_left - 1
                    );
                    Box::pin(self.run_command_inner(
                        EngineCommand::RegenerateCurrentTurn,
                        &format!("{label}/recover"),
                        recovers_left - 1,
                    ))
                    .await;
                    return;
                }
                if err.code.contains("quality") || err.code.contains("narration") {
                    self.quality_hard_fails += 1;
                }
                let report_path = repo_root().join("target/live-quality-report.md");
                let _ = self.write_quality_report(&report_path);
                panic!(
                    "[{label}] surfaced app error: {} ({}) - {} (partial report: {})",
                    err.title_ko,
                    err.code,
                    err.message_ko,
                    report_path.display()
                );
            }
        }
        eprintln!(
            "[{label}] ok -> turn={}, choices={}, narration_chars={}",
            self.app.view.status.turn,
            self.app.view.choices.len(),
            self.app.view.narration.chars().count(),
        );
    }
}

impl Drop for LiveSession {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.save_dir);
    }
}

#[tokio::test]
#[ignore = "live soak: set STORYTUI_LIVE_URL=http://127.0.0.1:8080 STORYTUI_LIVE_MODEL=local-model"]
async fn live_multi_pack_soak_through_host_loop() {
    let Ok(url) = std::env::var("STORYTUI_LIVE_URL") else {
        eprintln!("STORYTUI_LIVE_URL unset — skipping");
        return;
    };
    let model = std::env::var("STORYTUI_LIVE_MODEL").unwrap_or_else(|_| "local-model".into());

    // 1. rainy-reunion: opening + choice + free action + continue + suggest + group scene + regenerate + choice
    {
        let mut s = LiveSession::new("rainy-reunion", &url, &model, false);
        assert_eq!(
            s.provider
                .health()
                .await
                .unwrap_or(ProviderHealth::Unavailable),
            ProviderHealth::Ready
        );
        s.run_command(EngineCommand::StartNewGame, "rainy/opening")
            .await;
        assert!(s.app.view.status.turn >= 1);
        assert!(!s.app.view.choices.is_empty());

        let c0 = s.app.view.choices[0].id.clone();
        s.run_command(
            EngineCommand::SubmitChoice { choice_id: c0 },
            "rainy/choice-1",
        )
        .await;

        s.run_command(
            EngineCommand::SubmitFreeAction {
                action: Box::new(ParsedAction {
                    action_type: story_model::ActionType::Action,
                    target: None,
                    intent: None,
                    tone: None,
                    content: "커피잔을 옆으로 밀어두고 유나의 눈을 똑바로 바라보며 왜 오늘 부른 건지 묻는다.".into(),
                    original_text: "커피잔을 옆으로 밀어두고 유나의 눈을 똑바로 바라보며 왜 오늘 부른 건지 묻는다.".into(),
                    confidence: None,
                }),
            },
            "rainy/free-action",
        )
        .await;

        s.run_command(EngineCommand::ContinueNarration, "rainy/continue")
            .await;
        s.run_command(EngineCommand::SuggestAction, "rainy/suggest")
            .await;
        assert!(
            s.engine.suggested_action.is_some(),
            "suggest should populate suggested_action"
        );
        s.run_command(EngineCommand::GroupScene, "rainy/group-scene")
            .await;
        s.run_command(EngineCommand::RegenerateCurrentTurn, "rainy/regenerate")
            .await;

        let c1 = s.app.view.choices[0].id.clone();
        s.run_command(
            EngineCommand::SubmitChoice { choice_id: c1 },
            "rainy/choice-2",
        )
        .await;
    }

    // 2. mira-ko: opening + 3 consecutive choices
    {
        let mut s = LiveSession::new("mira-ko", &url, &model, false);
        s.run_command(EngineCommand::StartNewGame, "mira/opening")
            .await;
        for step in 1..=3 {
            let cid = s.app.view.choices[0].id.clone();
            s.run_command(
                EngineCommand::SubmitChoice { choice_id: cid },
                &format!("mira/choice-{step}"),
            )
            .await;
        }
    }

    // 3. starlight-station: single-pass (matches default local config).
    // meromero currently fails director_choice_quality repeatedly on the
    // 2-stage path; keep that covered by dedicated host_automation mocks.
    {
        let mut s = LiveSession::new("starlight-station", &url, &model, false);
        s.run_command(EngineCommand::StartNewGame, "starlight/opening")
            .await;
        for step in 1..=2 {
            let cid = s.app.view.choices[0].id.clone();
            s.run_command(
                EngineCommand::SubmitChoice { choice_id: cid },
                &format!("starlight/choice-{step}"),
            )
            .await;
        }
    }
}

/// Long endurance soak: keep playing until turn 100 on a single pack.
///
/// Gated behind `#[ignore]` + `STORYTUI_LIVE_URL`. Expect ~5–20 minutes
/// depending on the local model.
#[tokio::test]
#[ignore = "live 100-turn soak: set STORYTUI_LIVE_URL and STORYTUI_LIVE_MODEL"]
async fn live_hundred_turn_soak_rainy_reunion() {
    let Ok(url) = std::env::var("STORYTUI_LIVE_URL") else {
        eprintln!("STORYTUI_LIVE_URL unset — skipping");
        return;
    };
    let model = std::env::var("STORYTUI_LIVE_MODEL").unwrap_or_else(|_| "local-model".into());
    let target_turn: u32 = std::env::var("STORYTUI_LIVE_TURNS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100);

    let mut s = LiveSession::new("rainy-reunion", &url, &model, false);
    assert_eq!(
        s.provider
            .health()
            .await
            .unwrap_or(ProviderHealth::Unavailable),
        ProviderHealth::Ready
    );

    let started = Instant::now();
    s.run_command(EngineCommand::StartNewGame, "t100/opening")
        .await;
    assert!(s.app.view.status.turn >= 1);
    assert!(!s.app.view.choices.is_empty());

    let free_actions = [
        "반송된 편지 세 통에 대해 직접 묻는다.",
        "테이블 위 봉투를 열어 보자고 제안한다.",
        "전시회 날 나타나지 못한 일을 사과한다.",
        "그때 가족에게 무슨 일이 있었는지 조심스럽게 묻는다.",
        "창밖 비를 바라보며 커피를 살짝 돌려 식힌다.",
        "자리 이동을 제안하며 조용한 구석으로 몸을 돌린다.",
    ];

    let mut step = 0u32;
    while s.app.view.status.turn < target_turn {
        step += 1;
        let turn_before = s.app.view.status.turn;
        assert!(
            !s.app.view.choices.is_empty(),
            "no choices at turn {turn_before}"
        );

        // Every 7th step: free action; otherwise rotate among choices.
        if step.is_multiple_of(7) {
            let text = free_actions[((step / 7) as usize) % free_actions.len()];
            s.run_command(
                EngineCommand::SubmitFreeAction {
                    action: Box::new(ParsedAction {
                        action_type: story_model::ActionType::Action,
                        target: None,
                        intent: None,
                        tone: None,
                        content: text.into(),
                        original_text: text.into(),
                        confidence: None,
                    }),
                },
                &format!("t100/free-{step}"),
            )
            .await;
        } else {
            let streak = story_core::narrative::same_scene_streak(&s.engine.game);
            let rotate_idx =
                ((step as usize).saturating_sub(1)) % s.app.view.choices.len();
            let cid = if streak >= story_core::narrative::SCENE_STREAK_FORCE {
                s.engine
                    .game
                    .history
                    .last()
                    .into_iter()
                    .flat_map(|entry| entry.choices.iter())
                    .find(|choice| {
                        story_core::narrative::classify_choice(choice) == "situation"
                    })
                    .map(|choice| choice.id.clone())
                    .unwrap_or_else(|| s.app.view.choices[rotate_idx].id.clone())
            } else {
                s.app.view.choices[rotate_idx].id.clone()
            };
            s.run_command(
                EngineCommand::SubmitChoice { choice_id: cid },
                &format!("t100/choice-{step}"),
            )
            .await;
        }

        assert!(
            s.app.view.status.turn > turn_before,
            "turn did not advance from {turn_before} at step {step}"
        );
        assert!(
            s.app.view.error.is_none(),
            "error stuck after step {step}: {:?}",
            s.app.view.error
        );

        if s.app.view.status.turn.is_multiple_of(10) || s.app.view.status.turn >= target_turn {
            eprintln!(
                "[t100] progress turn={} step={} elapsed={:.1}s choices={}",
                s.app.view.status.turn,
                step,
                started.elapsed().as_secs_f32(),
                s.app.view.choices.len()
            );
        }
    }

    assert!(
        s.app.view.status.turn >= target_turn,
        "expected >= {target_turn} turns, got {}",
        s.app.view.status.turn
    );
    let report_path = repo_root().join("target/live-quality-report.md");
    s.write_quality_report(&report_path)
        .unwrap_or_else(|e| panic!("write quality report: {e}"));

    // Fail closed on the failure mode humans noticed: identical dialogue
    // fingerprints spanning 3+ consecutive committed turns.
    let mut run = 1usize;
    let mut max_run = 1usize;
    let mut prev: Option<std::collections::BTreeSet<String>> = None;
    for entry in &s.engine.game.history {
        let fps: std::collections::BTreeSet<String> =
            story_core::narration_quality::dialogue_fingerprints(&entry.narration)
                .into_iter()
                .collect();
        if let Some(p) = &prev {
            if fps.intersection(p).next().is_some() {
                run += 1;
                max_run = max_run.max(run);
            } else {
                run = 1;
            }
        }
        prev = Some(fps);
    }
    assert!(
        max_run < 3,
        "dialogue fingerprint shared across {max_run} consecutive turns (limit 2); see {}",
        report_path.display()
    );

    eprintln!(
        "[t100] done turn={} steps={} elapsed={:.1}s recovers={} dialogue_streak_max={} report={}",
        s.app.view.status.turn,
        step,
        started.elapsed().as_secs_f32(),
        s.quality_recovers,
        max_run,
        report_path.display()
    );
}
