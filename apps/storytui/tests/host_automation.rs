//! Host automation integration tests (T-2, AC-1..6/8).
//!
//! Drives the SAME handler functions the interactive loop runs —
//! [`storytui::host::handle_host_message`] (provider results),
//! [`storytui::host::handle_frame_automation`] (retry firing + periodic
//! reprobe), and [`storytui::host::post_accept_automation`] (truncation and
//! empty-side reactions) — against a wiremock OpenAI-compatible server, the
//! same style as the provider contract tests. No terminal is involved.

use std::sync::Arc;
use std::time::{Duration, Instant};

use story_core::engine::{EngineState, build_view, reduce};
use story_core::engine_cmd::EngineCommand;
use story_model::ProviderHealth;
use story_model::uuid::Uuid;
use story_provider::{OpenAiCompatibleProvider, ProviderConfig, ProviderError};
use story_tts::{TtsConfig, TtsService};
use story_tui::App;
use tokio::sync::mpsc::unbounded_channel;
use tokio_util::sync::CancellationToken;
use wiremock::MockServer;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, ResponseTemplate};

use storytui::config::{PlayerProfile, RuntimeConfig, UserSettings};
use storytui::host::{
    self, HostMessage, SceneImageStores, TurnAutomation, apply_resume, note_player_turn_command,
    post_accept_automation, probe_due, should_auto_resume,
};

/// A valid 3-choice turn (passes the engine's choice-quality count) wrapped
/// in an OpenAI-shaped chat-completion envelope.
fn envelope() -> serde_json::Value {
    let content = serde_json::json!({
        "narration": "비가 그쳤고, 유나는 창밖을 바라본다.",
        "choices": [
            {"id":"c1","label":"먼저 사과한다","intent":"고개를 숙이고 잘못을 인정한다","tone":null,"target":null,"risk":"medium","hidden_requirements":[],"preview":"관계가 누그러진다"},
            {"id":"c2","label":"화면을 보여준다","intent":"휴대폰의 메시지를 직접 보여준다","tone":null,"target":null,"risk":"safe","hidden_requirements":[],"preview":"반응이 달라진다"},
            {"id":"c3","label":"창가 자리를 부탁한다","intent":"직원에게 다른 자리로 이동하기를 요청한다","tone":null,"target":null,"risk":"medium","hidden_requirements":[],"preview":"이야기 무대가 바뀐다"}
        ],
        "state_patch": {
            "affection_delta": {},"trust_delta": {},"tension_delta": {},"relationship_transitions": {},
            "set_flags": {},"number_delta": {},"set_strings": {},"inventory_delta": {},"add_events": [],
            "next_location": null,"next_time_slot": null,"proposed_route": null
        },
        "summary": {
            "facts": [],"unresolved": [],"relationship_changes": [],"knowledge_changes": [],
            "interpretation": null,"address_form_changes": [],"important_player_actions": [],
            "source_history_ids": []
        }
    });
    serde_json::json!({
        "id": "chatcmpl-x",
        "object": "chat.completion",
        "model": "test",
        "choices": [
            {"index": 0, "message": {"role": "assistant", "content": content.to_string()}, "finish_reason": "stop"}
        ],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
    })
}

/// A valid 3-choice draft (passes choice quality) built in Rust — the same
/// shape the wire fixture parses into.
fn legal_draft() -> story_model::GeneratedTurnDraft {
    let choice = |id: &str, label: &str, intent: &str, risk, preview: &str| story_model::Choice {
        id: id.to_string(),
        label: label.to_string(),
        intent: intent.to_string(),
        tone: None,
        target: None,
        risk,
        hidden_requirements: Vec::new(),
        preview: Some(preview.to_string()),
    };
    story_model::GeneratedTurnDraft {
        generation_id: Uuid::nil(),
        narration: "비가 그쳤고, 유나는 창밖을 바라본다.".to_string(),
        choices: vec![
            choice(
                "c1",
                "먼저 사과한다",
                "고개를 숙이고 잘못을 인정한다",
                story_model::RiskLevel::Medium,
                "관계가 누그러진다",
            ),
            choice(
                "c2",
                "화면을 보여준다",
                "휴대폰의 메시지를 직접 보여준다",
                story_model::RiskLevel::Safe,
                "반응이 달라진다",
            ),
            choice(
                "c3",
                "창가 자리를 부탁한다",
                "직원에게 다른 자리로 이동하기를 요청한다",
                story_model::RiskLevel::Medium,
                "이야기 무대가 바뀐다",
            ),
        ],
        proposed_patch: story_model::StatePatch::default(),
        summary: story_model::TurnSummary {
            facts: Vec::new(),
            unresolved: Vec::new(),
            relationship_changes: Vec::new(),
            knowledge_changes: Vec::new(),
            interpretation: None,
            address_form_changes: Vec::new(),
            important_player_actions: Vec::new(),
            source_history_ids: Vec::new(),
        },
        usage: None,
    }
}

/// A valid Director plan (passes plan validation and choice quality — the
/// same known-playable choice trio as [`envelope()`]) wrapped in an
/// OpenAI-shaped chat-completion envelope for the non-streaming Director
/// stage.
fn director_plan_envelope() -> serde_json::Value {
    let plan = serde_json::json!({
        "scene_goal": "비가 그친 창가에서 재회를 정리한다",
        "required_beats": ["자리를 옮긴다"],
        "proposed_patch": {
            "affection_delta": {},"trust_delta": {},"tension_delta": {},"relationship_transitions": {},
            "set_flags": {},"number_delta": {},"set_strings": {},"inventory_delta": {},"add_events": [],
            "next_location": null,"next_time_slot": null,"proposed_route": null
        },
        "choices": [
            {"id":"c1","label":"먼저 사과한다","intent":"고개를 숙이고 잘못을 인정한다","tone":null,"target":null,"risk":"medium","hidden_requirements":[],"preview":"관계가 누그러진다"},
            {"id":"c2","label":"화면을 보여준다","intent":"휴대폰의 메시지를 직접 보여준다","tone":null,"target":null,"risk":"safe","hidden_requirements":[],"preview":"반응이 달라진다"},
            {"id":"c3","label":"창가 자리를 부탁한다","intent":"직원에게 다른 자리로 이동하기를 요청한다","tone":null,"target":null,"risk":"medium","hidden_requirements":[],"preview":"이야기 무대가 바뀐다"}
        ],
        "end_condition": "scene continues"
    });
    serde_json::json!({
        "id": "chatcmpl-plan",
        "object": "chat.completion",
        "model": "test",
        "choices": [
            {"index": 0, "message": {"role": "assistant", "content": plan.to_string()}, "finish_reason": "stop"}
        ],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
    })
}

/// A 200 SSE stream whose narration is the same quality-gate-passing sentence
/// as [`envelope()`], served raw like the provider contract tests do.
fn writer_stream_response() -> ResponseTemplate {
    let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"비가 그쳤고, 유나는 창밖을 바라본다.\"}}]}\n\n\
               data: [DONE]\n\n";
    ResponseTemplate::new(200).set_body_raw(sse.as_bytes().to_vec(), "text/event-stream")
}

/// Loopback provider config; `max_retries` 0 keeps provider-internal retries
/// out of the host-level retry assertions.
fn provider_config_for(uri: &str) -> ProviderConfig {
    let mut config = ProviderConfig::defaults();
    config.base_url = uri.to_string();
    config.model = "test-model".to_string();
    config.request_timeout_ms = 5_000;
    config.connect_timeout_ms = 1_000;
    config.max_retries = 0;
    // Off keeps the Auto 400-fallback out of the way: a 400 must surface as
    // a host-visible failure, not be swallowed as a schema-capability loss.
    config.structured_output = story_provider::StructuredOutputMode::Off;
    config
}

/// A loopback URL whose port is closed: connect is refused instantly.
fn dead_provider_config() -> ProviderConfig {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    provider_config_for(&format!("http://127.0.0.1:{port}"))
}

struct Session {
    engine: EngineState,
    app: App,
    pack: story_model::StoryPack,
    provider_config: ProviderConfig,
    provider: Arc<OpenAiCompatibleProvider>,
    user_settings: UserSettings,
    config: RuntimeConfig,
    save_store: story_persistence::SaveStore,
    tts: TtsService,
    images: SceneImageStores,
    tx: tokio::sync::mpsc::UnboundedSender<HostMessage>,
    rx: tokio::sync::mpsc::UnboundedReceiver<HostMessage>,
    cancellation: Option<(Uuid, CancellationToken)>,
    automation: TurnAutomation,
    last_probe: Instant,
    _dir: std::path::PathBuf,
}

async fn session_for(provider_config: ProviderConfig) -> Session {
    let dir = std::env::temp_dir().join(format!(
        "storytui-host-automation-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("save dir");

    let pack = story_core::tests_support::sample_pack();
    let engine = EngineState::initial(&pack);
    let app = App::new(build_view(&engine, &pack, None, None));
    let provider =
        Arc::new(OpenAiCompatibleProvider::new(provider_config.clone()).expect("provider"));
    let config = RuntimeConfig {
        provider: provider_config.clone(),
        providers: vec![provider_config.clone()],
        stage_defaults: Default::default(),
        fallback_model: String::new(),
        base_sampling: (0.85, 0.92, 40),
        director_model: None,
        choice_model: None,
        writer_model: None,
        tts: TtsConfig::default(),
        save_dir: dir.clone(),
        player: PlayerProfile::default(),
        use_director_writer: false,
        pack_dir: std::path::PathBuf::from("."),
        source_path: None,
    };
    let save_store = story_persistence::SaveStore::open(&dir).expect("save store");
    let tts = TtsService::new(TtsConfig::default());
    let (tx, rx) = unbounded_channel();
    Session {
        engine,
        app,
        pack,
        provider_config,
        provider,
        user_settings: UserSettings::default(),
        config,
        save_store,
        tts,
        images: SceneImageStores::default(),
        tx,
        rx,
        cancellation: None,
        automation: TurnAutomation::default(),
        // The startup health probe counts as the first probe.
        last_probe: Instant::now(),
        _dir: dir,
    }
}

/// Start the opening generation exactly like the interactive loop does.
async fn start_session(provider_config: ProviderConfig) -> Session {
    let mut session = session_for(provider_config).await;
    host::dispatch(
        EngineCommand::StartNewGame,
        &mut session.engine,
        &mut session.app,
        &session.pack,
        &session.provider,
        &session.config,
        &session.save_store,
        &session.tts,
        &mut session.images,
        &session.tx,
        &mut session.cancellation,
    )
    .await
    .expect("start new game dispatch");
    session
}

/// Start the opening generation on the 2-stage (Director → Writer) path: the
/// same body as [`start_session`] with the generation-mode flags flipped
/// before the dispatch, exactly like the settings `generation_toggle` does.
async fn start_two_stage_session(provider_config: ProviderConfig) -> Session {
    let mut session = session_for(provider_config).await;
    session.config.use_director_writer = true;
    session.engine.use_director_writer = true;
    host::dispatch(
        EngineCommand::StartNewGame,
        &mut session.engine,
        &mut session.app,
        &session.pack,
        &session.provider,
        &session.config,
        &session.save_store,
        &session.tts,
        &mut session.images,
        &session.tx,
        &mut session.cancellation,
    )
    .await
    .expect("start new game dispatch");
    session
}

/// Receive the next provider result with a generous timeout.
async fn next_generation(session: &mut Session) -> HostMessage {
    match tokio::time::timeout(Duration::from_secs(5), session.rx.recv()).await {
        Ok(Some(message)) => message,
        other => panic!("no provider result arrived: {other:?}"),
    }
}

/// Handle one provider result through the shared host handler.
async fn handle(session: &mut Session, message: HostMessage) -> bool {
    host::handle_host_message(
        message,
        &mut session.engine,
        &mut session.app,
        &session.pack,
        &session.provider,
        &session.config,
        &session.save_store,
        &session.tts,
        &mut session.images,
        &session.tx,
        &mut session.cancellation,
        &mut session.automation,
    )
    .await
    .expect("handle_host_message")
}

/// Run the frame automation with an injected `now` (the retry schedule).
async fn frame_at(session: &mut Session, now: Instant) -> bool {
    host::handle_frame_automation(
        &mut session.engine,
        &mut session.app,
        &session.pack,
        &mut session.provider_config,
        &mut session.provider,
        &mut session.user_settings,
        &mut session.config,
        &session.save_store,
        &session.tts,
        &mut session.images,
        &session.tx,
        &mut session.cancellation,
        &mut session.automation,
        &mut session.last_probe,
        now,
    )
    .await
    .expect("handle_frame_automation")
}

/// Commit the opening turn directly through the engine (no provider), for
/// the post-accept automation tests that need a committed turn.
fn commit_opening(session: &mut Session) {
    let transition =
        reduce(&session.engine, EngineCommand::StartNewGame, &session.pack).expect("StartNewGame");
    session.engine = transition.state;
    let generation_id = session
        .engine
        .pending
        .as_ref()
        .expect("opening pending")
        .generation_id;
    let mut content = legal_draft();
    content.generation_id = generation_id;
    let transition = reduce(
        &session.engine,
        EngineCommand::AcceptGeneration {
            draft: Box::new(content),
        },
        &session.pack,
    )
    .expect("AcceptGeneration");
    session.engine = transition.state;
    for effect in transition.effects {
        if let story_core::engine_cmd::EngineEffect::PublishView(view) = effect {
            session.app.set_view(*view);
        }
    }
}

// ---------------------------------------------------------------------------
// AC-3: automatic one-shot generation retry
// ---------------------------------------------------------------------------

#[tokio::test]
async fn generation_failure_autoretries_once_and_succeeds() {
    let server = MockServer::start().await;
    // Request 1 fails with 400 (the provider itself never retries 4xx).
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(400))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(envelope()))
        .mount(&server)
        .await;

    let mut session = start_session(provider_config_for(&server.uri())).await;
    let message = next_generation(&mut session).await;
    assert!(
        matches!(&message, HostMessage::Generation { .. }),
        "expected a generation result"
    );

    let redraw = handle(&mut session, message).await;
    assert!(redraw);
    // AC-3: the retry is armed with the status line, and NO error overlay
    // appeared — the flow was never blocked.
    assert!(session.automation.retry_armed(), "retry armed");
    assert!(session.automation.retry_spent());
    assert_eq!(session.app.story_notice(), Some(host::AUTO_RETRY_NOTICE));
    assert!(session.app.view.error.is_none(), "no overlay yet");
    assert!(session.app.view.loading.is_some(), "turn still pending");

    // The frame timer crosses the delay: the retry re-dispatches the turn.
    let fired = frame_at(
        &mut session,
        Instant::now() + host::AUTO_RETRY_DELAY + Duration::from_millis(50),
    )
    .await;
    assert!(fired, "the due retry is fired");

    // The retried generation succeeds and the turn commits.
    let message = next_generation(&mut session).await;
    let _ = handle(&mut session, message).await;
    assert_eq!(session.app.view.status.turn, 1, "opening committed");
    assert!(session.app.view.error.is_none());
    assert!(!session.automation.retry_armed());

    // Exactly two HTTP requests: the failure + the automatic retry (the
    // committed turn's choices pass quality, so no extra refresh call).
    let requests = server.received_requests().await.expect("recorded");
    assert_eq!(requests.len(), 2, "fail + retry only");
}

#[tokio::test]
async fn second_failure_hands_control_to_the_manual_overlay() {
    let server = MockServer::start().await;
    // Every request fails: the automatic retry cannot save the turn.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(400))
        .mount(&server)
        .await;

    let mut session = start_session(provider_config_for(&server.uri())).await;
    let message = next_generation(&mut session).await;
    let _ = handle(&mut session, message).await;
    assert!(session.automation.retry_armed());

    let _ = frame_at(
        &mut session,
        Instant::now() + host::AUTO_RETRY_DELAY + Duration::from_millis(50),
    )
    .await;
    let message = next_generation(&mut session).await;
    let _ = handle(&mut session, message).await;

    // AC-3: the budget is spent — the manual [R]/[Esc] overlay takes over.
    assert!(
        session.app.view.error.is_some(),
        "the second failure surfaces the overlay"
    );
    assert!(!session.automation.retry_armed());
    assert!(session.automation.retry_spent());

    let requests = server.received_requests().await.expect("recorded");
    assert_eq!(requests.len(), 2, "no third automatic attempt");
}

#[tokio::test]
async fn player_cancel_never_autoretries() {
    let server = MockServer::start().await;
    // A delayed success keeps the request in flight while the player cancels.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(envelope())
                .set_delay(Duration::from_secs(2)),
        )
        .mount(&server)
        .await;

    let mut session = start_session(provider_config_for(&server.uri())).await;
    let (generation_id, _) = session.cancellation.as_ref().expect("in flight");
    let command = EngineCommand::CancelGeneration {
        generation_id: *generation_id,
    };
    // Same bookkeeping the key path runs before dispatch.
    note_player_turn_command(&command, &mut session.automation);
    host::dispatch(
        command,
        &mut session.engine,
        &mut session.app,
        &session.pack,
        &session.provider,
        &session.config,
        &session.save_store,
        &session.tts,
        &mut session.images,
        &session.tx,
        &mut session.cancellation,
    )
    .await
    .expect("cancel dispatch");

    // The provider surfaces the cancellation as Err(Cancelled).
    let message = next_generation(&mut session).await;
    let _ = handle(&mut session, message).await;

    assert!(
        !session.automation.retry_armed() && !session.automation.retry_spent(),
        "a cancel is a decision, not a failure: no retry armed or spent"
    );
    assert_eq!(session.app.story_notice(), None, "no retry notice");
    assert!(session.app.view.error.is_none(), "cancel shows no overlay");
}

// ---------------------------------------------------------------------------
// F-1 (G3 review): 2-stage Director→Writer failures join the AC-3 retry
// budget — a transport failure in either stage arms the same one-shot
// automatic retry instead of jumping straight to the [R]/[Esc] overlay.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn writer_stage_failure_autoretries_once_and_commits() {
    let server = MockServer::start().await;
    // The Director stage always plans fine; the FIRST Writer call fails with
    // 400 (the provider never retries 4xx), the retried one succeeds.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("Director stage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(director_plan_envelope()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("Writer stage"))
        .respond_with(ResponseTemplate::new(400))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("Writer stage"))
        .respond_with(writer_stream_response())
        .mount(&server)
        .await;

    let mut session = start_two_stage_session(provider_config_for(&server.uri())).await;
    let message = next_generation(&mut session).await;
    assert!(
        matches!(&message, HostMessage::Director { result: Ok(_), .. }),
        "expected a director plan result"
    );
    let _ = handle(&mut session, message).await;

    // The Writer stage fails: the retry is armed with the status line and NO
    // error overlay appeared — the flow was never blocked.
    let message = next_generation(&mut session).await;
    assert!(
        matches!(&message, HostMessage::Writer { result: Err(_), .. }),
        "expected a writer failure"
    );
    let redraw = handle(&mut session, message).await;
    assert!(redraw);
    assert!(session.automation.retry_armed(), "retry armed");
    assert!(session.automation.retry_spent());
    assert_eq!(session.app.story_notice(), Some(host::AUTO_RETRY_NOTICE));
    assert!(session.app.view.error.is_none(), "no overlay yet");
    assert!(session.app.view.loading.is_some(), "turn still pending");

    // The frame timer crosses the delay: the retry restarts the 2-stage turn
    // (RegenerateCurrentTurn re-requests the Director first — the same
    // command a manual [R] issues).
    let fired = frame_at(
        &mut session,
        Instant::now() + host::AUTO_RETRY_DELAY + Duration::from_millis(50),
    )
    .await;
    assert!(fired, "the due retry is fired");

    let message = next_generation(&mut session).await;
    let _ = handle(&mut session, message).await; // retried Director Ok
    let message = next_generation(&mut session).await;
    let _ = handle(&mut session, message).await; // retried Writer Ok → commit
    assert_eq!(session.app.view.status.turn, 1, "opening committed");
    assert!(session.app.view.error.is_none());
    assert!(!session.automation.retry_armed());

    // Exactly four HTTP requests: director + writer fail + retried director
    // + retried writer (the plan's choices pass quality — no refresh call).
    let requests = server.received_requests().await.expect("recorded");
    assert_eq!(requests.len(), 4, "fail + retry only");
}

#[tokio::test]
async fn two_stage_second_failure_hands_control_to_the_manual_overlay() {
    let server = MockServer::start().await;
    // The Director stage always plans fine; every Writer call fails.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("Director stage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(director_plan_envelope()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("Writer stage"))
        .respond_with(ResponseTemplate::new(400))
        .mount(&server)
        .await;

    let mut session = start_two_stage_session(provider_config_for(&server.uri())).await;
    let message = next_generation(&mut session).await; // Director Ok
    let _ = handle(&mut session, message).await;
    let message = next_generation(&mut session).await; // Writer Err #1 → armed
    let _ = handle(&mut session, message).await;
    assert!(session.automation.retry_armed());

    let _ = frame_at(
        &mut session,
        Instant::now() + host::AUTO_RETRY_DELAY + Duration::from_millis(50),
    )
    .await;
    let message = next_generation(&mut session).await; // retried Director Ok
    let _ = handle(&mut session, message).await;
    let message = next_generation(&mut session).await; // Writer Err #2
    let _ = handle(&mut session, message).await;

    // AC-3: the budget is spent — the manual [R]/[Esc] overlay takes over.
    assert!(
        session.app.view.error.is_some(),
        "the second failure surfaces the overlay"
    );
    assert!(!session.automation.retry_armed());
    assert!(session.automation.retry_spent());

    let requests = server.received_requests().await.expect("recorded");
    assert_eq!(requests.len(), 4, "no third automatic attempt");
}

#[tokio::test]
async fn two_stage_player_cancel_never_autoretries() {
    let server = MockServer::start().await;
    // A delayed plan keeps the Director request in flight while the player
    // cancels.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(director_plan_envelope())
                .set_delay(Duration::from_secs(2)),
        )
        .mount(&server)
        .await;

    let mut session = start_two_stage_session(provider_config_for(&server.uri())).await;
    let (generation_id, _) = session.cancellation.as_ref().expect("in flight");
    let command = EngineCommand::CancelGeneration {
        generation_id: *generation_id,
    };
    // Same bookkeeping the key path runs before dispatch.
    note_player_turn_command(&command, &mut session.automation);
    host::dispatch(
        command,
        &mut session.engine,
        &mut session.app,
        &session.pack,
        &session.provider,
        &session.config,
        &session.save_store,
        &session.tts,
        &mut session.images,
        &session.tx,
        &mut session.cancellation,
    )
    .await
    .expect("cancel dispatch");

    // The provider surfaces the cancellation as Err(Cancelled) on the
    // Director message.
    let message = next_generation(&mut session).await;
    assert!(
        matches!(
            &message,
            HostMessage::Director {
                result: Err(ProviderError::Cancelled),
                ..
            }
        ),
        "expected a cancelled director result"
    );
    let _ = handle(&mut session, message).await;

    assert!(
        !session.automation.retry_armed() && !session.automation.retry_spent(),
        "a cancel is a decision, not a failure: no retry armed or spent"
    );
    assert_eq!(session.app.story_notice(), None, "no retry notice");
    assert!(session.app.view.error.is_none(), "cancel shows no overlay");
}

// ---------------------------------------------------------------------------
// AC-4: periodic reprobe
// ---------------------------------------------------------------------------

#[test]
fn probe_gate_skips_loading_and_respects_the_interval() {
    let now = Instant::now();
    assert!(!probe_due(now, now, true), "never while generating");
    assert!(!probe_due(now, now, false), "interval not elapsed");
    assert!(
        probe_due(now + host::PROBE_INTERVAL, now, false),
        "due after the interval"
    );
}

#[tokio::test]
async fn reprobe_marks_the_server_back_and_clears_the_home_notice() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let mut session = session_for(provider_config_for(&server.uri())).await;
    // The world as the startup probe saw it: server down.
    let tts_status = session.tts.status().await;
    host::settings::refresh_view(
        &mut session.app,
        &session.provider_config,
        ProviderHealth::Unavailable,
        None,
        None,
        None,
        &tts_status,
        true,
        None,
        false,
        true,
        &session.user_settings,
        &session.config.player,
    );
    session
        .app
        .set_home_status(Some(host::settings::server_down_home_status()));
    // Schedule due.
    session.last_probe = Instant::now() - host::PROBE_INTERVAL;

    let redraw = frame_at(&mut session, Instant::now()).await;
    assert!(redraw, "availability changed → redraw");
    assert_eq!(
        session.app.provider_health(),
        Some(ProviderHealth::Ready),
        "settings status row refreshed to 연결됨"
    );
    assert_eq!(
        session.app.home_status_text(),
        None,
        "the server-down home line is gone"
    );

    // Healthy and freshly probed: the gate stays quiet.
    let quiet = frame_at(&mut session, Instant::now()).await;
    assert!(!quiet, "no probe churn while ready");
}

#[tokio::test]
async fn reprobe_stays_quiet_while_the_server_is_still_down() {
    let mut session = session_for(dead_provider_config()).await;
    let tts_status = session.tts.status().await;
    host::settings::refresh_view(
        &mut session.app,
        &session.provider_config,
        ProviderHealth::Unavailable,
        None,
        None,
        None,
        &tts_status,
        true,
        None,
        false,
        true,
        &session.user_settings,
        &session.config.player,
    );
    session.last_probe = Instant::now() - host::PROBE_INTERVAL;

    let redraw = frame_at(&mut session, Instant::now()).await;
    assert!(!redraw, "no change while still down");
    assert_eq!(
        session.app.provider_health(),
        Some(ProviderHealth::Unavailable)
    );
    // A still-down probe re-arms the schedule (retry in PROBE_INTERVAL).
    assert!(probe_due(
        Instant::now() + host::PROBE_INTERVAL,
        session.last_probe,
        false
    ));
}

// ---------------------------------------------------------------------------
// AC-1: auto-resume entry
// ---------------------------------------------------------------------------

#[test]
fn auto_resume_requires_both_the_setting_and_an_autosave() {
    assert!(should_auto_resume(true, true));
    assert!(!should_auto_resume(false, true), "off keeps home first");
    assert!(!should_auto_resume(true, false), "fresh install sees home");
}

#[tokio::test]
async fn apply_resume_lands_in_the_restored_story_and_skips_home() {
    let mut session = session_for(dead_provider_config()).await;
    let mut restored = EngineState::initial(&session.pack);
    restored.game.turn = 3;
    // The AC-1 startup path never opens the launcher at all.
    assert!(!session.app.home_visible());
    let mut resume = Some(restored);

    apply_resume(
        &mut session.app,
        &mut session.engine,
        &session.pack,
        &mut resume,
        &mut session.images,
        false,
    );
    session.app.set_story_notice(Some(host::AUTO_RESUME_NOTICE));

    assert_eq!(session.app.view.status.turn, 3, "restored view published");
    assert!(!session.app.home_visible(), "home was skipped");
    assert!(!session.app.can_continue(), "the resume was consumed");
    assert_eq!(session.app.story_notice(), Some(host::AUTO_RESUME_NOTICE));
    assert!(resume.is_none(), "the resume engine is spent");
}

// ---------------------------------------------------------------------------
// AC-5/AC-8: truncation auto-continue and manual-only guide
// ---------------------------------------------------------------------------

#[tokio::test]
async fn truncation_auto_continues_exactly_once_per_turn() {
    let mut session = session_for(dead_provider_config()).await;
    commit_opening(&mut session);
    assert!(session.engine.pending.is_none(), "idle between turns");

    // The engine published a truncated Writer turn (finish_reason "length").
    session.app.view.truncation = story_model::TruncationStatus::Auto;
    post_accept_automation(
        &mut session.engine,
        &mut session.app,
        &session.pack,
        &session.provider,
        &session.config,
        &session.save_store,
        &session.tts,
        &mut session.images,
        &session.tx,
        &mut session.cancellation,
        &mut session.automation,
        true,
    )
    .await
    .expect("post accept");
    let first = session.engine.pending.as_ref().map(|p| p.generation_id);
    assert!(first.is_some(), "ContinueNarration was auto-dispatched");
    assert_eq!(
        session.engine.pending.as_ref().and_then(|p| p.side_kind),
        Some(story_core::engine::SideGenerationKind::Continue)
    );
    assert!(session.automation.auto_continue_spent());

    // The empty-accept loop guard: a still-Auto view cannot fire a second
    // automatic continue within the turn.
    session.app.view.truncation = story_model::TruncationStatus::Auto;
    let second = session.engine.pending.as_ref().map(|p| p.generation_id);
    post_accept_automation(
        &mut session.engine,
        &mut session.app,
        &session.pack,
        &session.provider,
        &session.config,
        &session.save_store,
        &session.tts,
        &mut session.images,
        &session.tx,
        &mut session.cancellation,
        &mut session.automation,
        true,
    )
    .await
    .expect("post accept again");
    assert_eq!(first, second, "no second dispatch — cap 1 per turn");
}

#[tokio::test]
async fn manual_only_truncation_guides_without_dispatching() {
    let mut session = session_for(dead_provider_config()).await;
    commit_opening(&mut session);

    // AC-8: the repair budget was spent — the engine says ManualOnly.
    session.app.view.truncation = story_model::TruncationStatus::ManualOnly;
    post_accept_automation(
        &mut session.engine,
        &mut session.app,
        &session.pack,
        &session.provider,
        &session.config,
        &session.save_store,
        &session.tts,
        &mut session.images,
        &session.tx,
        &mut session.cancellation,
        &mut session.automation,
        true,
    )
    .await
    .expect("post accept");
    assert!(
        session.engine.pending.is_none(),
        "no generation was started"
    );
    assert_eq!(
        session.app.story_notice(),
        Some(host::MANUAL_CONTINUE_NOTICE)
    );
    assert!(
        !session.automation.auto_continue_spent(),
        "budget untouched"
    );
}

// ---------------------------------------------------------------------------
// AC-6: empty side-result re-dispatch
// ---------------------------------------------------------------------------

#[tokio::test]
async fn empty_side_result_is_redispatched_once_then_guided() {
    let mut session = session_for(dead_provider_config()).await;
    commit_opening(&mut session);
    session
        .automation
        .note_side_command(EngineCommand::ContinueNarration);

    // The engine reported an empty Continue accept.
    session.app.view.empty_side_result = true;
    post_accept_automation(
        &mut session.engine,
        &mut session.app,
        &session.pack,
        &session.provider,
        &session.config,
        &session.save_store,
        &session.tts,
        &mut session.images,
        &session.tx,
        &mut session.cancellation,
        &mut session.automation,
        true,
    )
    .await
    .expect("post accept");
    let first = session.engine.pending.as_ref().map(|p| p.generation_id);
    assert!(first.is_some(), "the same side command was re-dispatched");
    assert!(session.automation.side_retry_spent());
    assert_eq!(session.app.story_notice(), Some(host::EMPTY_SIDE_NOTICE));

    // The re-run is empty too: guide only, no second re-dispatch.
    session.engine.pending = None;
    session.app.view.empty_side_result = true;
    post_accept_automation(
        &mut session.engine,
        &mut session.app,
        &session.pack,
        &session.provider,
        &session.config,
        &session.save_store,
        &session.tts,
        &mut session.images,
        &session.tx,
        &mut session.cancellation,
        &mut session.automation,
        true,
    )
    .await
    .expect("post accept");
    assert!(session.engine.pending.is_none(), "no second re-dispatch");
    assert_eq!(session.app.story_notice(), Some(host::EMPTY_SIDE_NOTICE));
}
