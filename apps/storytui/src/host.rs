use std::io::{self, Stdout};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyEventKind, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures_util::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use story_core::engine::{EngineState, SideGenerationKind, build_view, reduce};
use story_core::engine_cmd::{EngineCommand, EngineEffect};
use story_model::TruncationStatus;
use story_model::{DomainErrorView, ProviderError, ProviderHealth, StoryPack};
use story_persistence::{PackLoader, SaveStore};
use story_provider::{
    BUILTIN_DEFAULT_MODEL, BUILTIN_MODELS, BUILTIN_PROVIDER_ID, LlmProvider, is_builtin_config,
};
use story_tui::{App, HomeRequest};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio_util::sync::CancellationToken;

use story_tts::TtsService;
use storytui::config::{
    PlayerProfile, RuntimeConfig, StageModels, UserSettings, effective_player_profile,
    load_user_settings, save_user_settings,
};

mod diagnostics;
mod images;
mod packs;
/// Save/load + autosave host operations. Public because the export
/// subcommand reuses [`saves::save_engine_snapshot`].
pub mod saves;
/// Provider/settings projection operations (AC-4 reprobe lives here).
pub mod settings;

type UiTerminal = Terminal<CrosstermBackend<Stdout>>;

/// Summary of a finished card import (dialog report content).
#[derive(Debug)]
pub struct ImportOutcome {
    pub pack_dir: String,
    pub lines: Vec<String>,
}

/// Messages the provider/engine tasks send back to the polling loop. Public
/// because the host automation handlers in the lib surface take them.
#[derive(Debug)]
pub enum HostMessage {
    ImportFinished {
        path: String,
        result: Result<ImportOutcome, String>,
    },
    Generation {
        generation_id: story_model::uuid::Uuid,
        model: String,
        result: Result<story_model::GeneratedTurnDraft, ProviderError>,
    },
    Director {
        generation_id: story_model::uuid::Uuid,
        result: Result<story_model::DirectorPlan, ProviderError>,
    },
    Writer {
        generation_id: story_model::uuid::Uuid,
        model: String,
        result: Result<story_model::WriterFinish, ProviderError>,
    },
    WriterChunk {
        generation_id: story_model::uuid::Uuid,
        text: String,
    },
}

/// Delay before the single automatic generation retry (AC-3): long enough to
/// read the status line, short enough that the app never feels stopped.
pub const AUTO_RETRY_DELAY: Duration = Duration::from_millis(1_500);
/// Periodic provider reprobe interval while the server is unavailable (AC-4).
pub const PROBE_INTERVAL: Duration = Duration::from_secs(15);
/// How often the idle loop wakes to check the reprobe schedule.
const PROBE_TICK: Duration = Duration::from_secs(1);

/// Story-screen notice surfaces (no new keys — the subtle tail row reuses the
/// variant-badge convention).
pub const AUTO_RESUME_NOTICE: &str = "이전 세션을 이어요 — Alt+Home 홈";
pub const AUTO_RETRY_NOTICE: &str = "자동 재시도…(1/1)";
pub const MANUAL_CONTINUE_NOTICE: &str = "잘림 — w로 이어쓰기";
pub const CONTINUE_BADGE: &str = "✎ 이어씀";
pub const EMPTY_SIDE_NOTICE: &str = "빈 결과 — 다시 시도";

/// Per-turn automation budget (AC-3/5/6/8): every automatic model re-call —
/// generation retry, truncation continue, side-command re-dispatch — happens
/// at most once per turn. A second failure hands control back to the player's
/// [R] overlay instead of looping against a weak model.
#[derive(Default)]
pub struct TurnAutomation {
    /// The failed generation awaiting its one automatic retry, with the
    /// instant it fires. `None` while nothing is scheduled.
    retry_due: Option<(story_model::uuid::Uuid, Instant)>,
    /// This turn's automatic retry is spent (armed, fired, or refused).
    retry_spent: bool,
    /// This turn's automatic truncation continue is spent.
    auto_continue_spent: bool,
    /// This turn's side-command re-dispatch is spent.
    side_retry_spent: bool,
    /// The most recent side command the host issued (AC-6 re-dispatch target).
    last_side_command: Option<EngineCommand>,
}

impl TurnAutomation {
    /// Arm the single automatic retry for this turn. Fails when the budget is
    /// spent or a retry is already scheduled.
    pub fn arm_retry(&mut self, generation_id: story_model::uuid::Uuid, now: Instant) -> bool {
        if self.retry_spent || self.retry_due.is_some() {
            return false;
        }
        self.retry_spent = true;
        self.retry_due = Some((generation_id, now + AUTO_RETRY_DELAY));
        true
    }

    /// Pop the armed retry once it is due.
    pub fn take_due_retry(&mut self, now: Instant) -> bool {
        match self.retry_due {
            Some((_, due)) if now >= due => {
                self.retry_due = None;
                true
            }
            _ => false,
        }
    }

    /// Drop an armed retry (the turn ended another way — a player cancel, a
    /// settings-driven cancel).
    pub fn cancel_retry(&mut self) {
        self.retry_due = None;
    }

    /// Whether a retry is currently scheduled.
    pub fn retry_armed(&self) -> bool {
        self.retry_due.is_some()
    }

    /// Whether this turn's automatic retry budget is spent.
    pub fn retry_spent(&self) -> bool {
        self.retry_spent
    }

    /// Whether this turn's automatic truncation continue is spent.
    pub fn auto_continue_spent(&self) -> bool {
        self.auto_continue_spent
    }

    /// Whether this turn's side-command re-dispatch is spent.
    pub fn side_retry_spent(&self) -> bool {
        self.side_retry_spent
    }

    /// A fresh turn request (or a committed turn) refills the budget.
    pub fn reset_turn(&mut self) {
        self.retry_due = None;
        self.retry_spent = false;
        self.auto_continue_spent = false;
        self.side_retry_spent = false;
    }

    /// Record the side command the host just issued — the AC-6 re-dispatch
    /// target when its result comes back empty.
    pub fn note_side_command(&mut self, command: EngineCommand) {
        self.last_side_command = Some(command);
    }

    /// The most recent side command, if any.
    pub fn last_side_command(&self) -> Option<&EngineCommand> {
        self.last_side_command.as_ref()
    }
}

/// AC-1 startup entry decision: resume immediately only when the setting is
/// on AND an autosave exists. A fresh install (no autosave) always sees the
/// home menu — the learning path stays intact.
pub fn should_auto_resume(auto_resume: bool, resume_available: bool) -> bool {
    auto_resume && resume_available
}

/// Apply the restored autosave to the session — the home 이어하기 body.
/// Shared by the home menu request and the AC-1 startup auto-resume, so the
/// two entries can never diverge.
pub fn apply_resume(
    app: &mut App,
    engine: &mut EngineState,
    pack: &StoryPack,
    resume_engine: &mut Option<EngineState>,
    scene_images: &mut SceneImageStores,
    use_director_writer: bool,
) {
    if let Some(mut restored) = resume_engine.take() {
        restored.use_director_writer = use_director_writer;
        *engine = restored;
        app.set_view(build_view(engine, pack, None, None));
        saves::refresh_app_lists(app, engine, pack);
        app.follow_story_end();
        app.set_can_continue(false);
        refresh_scene_images(app, pack, engine, scene_images);
    }
}

/// AC-4 gate: probe while idle, at most every [`PROBE_INTERVAL`].
pub fn probe_due(now: Instant, last_probe: Instant, loading: bool) -> bool {
    !loading && now.duration_since(last_probe) >= PROBE_INTERVAL
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<(Self, UiTerminal)> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            crossterm::cursor::Hide
        )?;
        // Avoid an eager `clear()` here. Crossterm's clear path may query the
        // cursor position, which is unsupported by some PTYs and is not
        // needed because alternate-screen entry starts from a fresh surface.
        let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        Ok((Self, terminal))
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(
            stdout,
            DisableMouseCapture,
            LeaveAlternateScreen,
            crossterm::cursor::Show
        );
    }
}

pub async fn run(config: RuntimeConfig) -> Result<(), String> {
    let mut config = config;
    let mut pack = PackLoader::load(&config.pack_dir)
        .map_err(|e| format!("Story Pack을 불러오지 못했습니다: {e}"))?;
    let mut user_settings = load_user_settings(&config.save_dir)?;
    user_settings.fold_legacy_stage_models(&config.provider.name);

    let mut provider_config = config.provider.clone();
    let mut repaired_main_model = false;
    // Which stages the preflight repairs — only those are pinned to the
    // profile below, so untouched stages keep inheriting their defaults.
    let mut repaired_stages = settings::StageRepairs::default();
    let mut auto_fallback_builtin = false;
    let pinned_builtin = user_settings.provider.as_deref() == Some(BUILTIN_PROVIDER_ID)
        || is_builtin_config(&provider_config);

    if pinned_builtin {
        if !is_builtin_config(&provider_config) {
            let mut builtin = story_provider::builtin_provider_config();
            if let Some(model) = &user_settings.model
                && BUILTIN_MODELS.contains(&model.as_str())
            {
                builtin.model = model.clone();
            }
            config.base_sampling = (builtin.temperature, builtin.top_p, builtin.top_k);
            user_settings.apply_sampling_taste(&mut builtin);
            provider_config = builtin;
            config.provider = provider_config.clone();
        } else if !BUILTIN_MODELS.contains(&provider_config.model.as_str()) {
            provider_config.model = BUILTIN_DEFAULT_MODEL.to_string();
            config.provider = provider_config.clone();
        }
    } else {
        let preferred_url = provider_config.base_url.clone();
        let preferred_name = provider_config.name.clone();
        let previous_model = provider_config.model.clone();
        match settings::discover_best_http_provider(
            &provider_config,
            &config.providers,
            &config.fallback_model,
        )
        .await
        {
            settings::HttpDiscoveryResult::Connected {
                config: adopted,
                advertised,
            } => {
                let mut adopted = *adopted;
                let switched_profile = adopted.name != preferred_name;
                if switched_profile {
                    tracing::info!(
                        from = %preferred_name,
                        to = %adopted.name,
                        base_url = %adopted.base_url,
                        model = %adopted.model,
                        "primary server unavailable; automatically selected a live configured provider"
                    );
                    config.base_sampling = (adopted.temperature, adopted.top_p, adopted.top_k);
                    user_settings.apply_sampling_taste(&mut adopted);
                    let saved = user_settings.stage_models.get(&adopted.name);
                    let default = config.stage_defaults.get(&adopted.name);
                    let current = StageModels {
                        director: config.director_model.clone(),
                        choice: config.choice_model.clone(),
                        writer: config.writer_model.clone(),
                    };
                    let (resolved, repairs) = settings::resolve_stage_assignments(
                        saved,
                        default,
                        &current,
                        &advertised,
                        &adopted.model,
                    );
                    config.director_model = resolved.director;
                    config.choice_model = resolved.choice;
                    config.writer_model = resolved.writer;
                    repaired_stages = repairs;
                } else if adopted.base_url != preferred_url || adopted.model != previous_model {
                    tracing::info!(
                        base_url = %adopted.base_url,
                        model = %adopted.model,
                        "automatically connected to a live local model server"
                    );
                    if previous_model.trim().is_empty() || adopted.base_url == preferred_url {
                        repaired_main_model = true;
                    }
                }
                provider_config = adopted;
                config.provider = provider_config.clone();
                if !advertised.is_empty() {
                    let repair_stage = |repaired: &mut bool, stage_model: &mut Option<String>| {
                        if let Some(current) = stage_model.clone()
                            && !advertised.iter().any(|model| model == &current)
                        {
                            tracing::warn!(
                                stale_model = %current,
                                fallback_model = %provider_config.model,
                                "stage model is not advertised; using the active model"
                            );
                            *stage_model = Some(provider_config.model.clone());
                            *repaired = true;
                        }
                    };
                    repair_stage(&mut repaired_stages.director, &mut config.director_model);
                    repair_stage(&mut repaired_stages.choice, &mut config.choice_model);
                    repair_stage(&mut repaired_stages.writer, &mut config.writer_model);
                }
            }
            settings::HttpDiscoveryResult::NoChatModel
            | settings::HttpDiscoveryResult::NoServer => {
                tracing::info!(
                    preferred = %preferred_name,
                    "no live HTTP model server found; falling back to the built-in offline story provider"
                );
                auto_fallback_builtin = true;
                let mut builtin = story_provider::builtin_provider_config();
                config.base_sampling = (builtin.temperature, builtin.top_p, builtin.top_k);
                user_settings.apply_sampling_taste(&mut builtin);
                provider_config = builtin;
                config.provider = provider_config.clone();
                config.director_model = None;
                config.choice_model = None;
                config.writer_model = None;
            }
        }
    }

    let mut provider = settings::build_provider(&provider_config)
        .map_err(|e| format!("provider를 만들지 못했습니다: {e}"))?;
    let save_store = SaveStore::open(&config.save_dir)
        .map_err(|e| format!("세이브 디렉터리를 열지 못했습니다: {e}"))?;

    let health = provider
        .health()
        .await
        .unwrap_or(ProviderHealth::Unavailable);
    tracing::info!(
        provider = %provider.id(),
        model = %provider_config.model,
        base_url = %provider_config.base_url,
        ?health,
        "provider health probe"
    );

    let (_guard, mut terminal) =
        TerminalGuard::enter().map_err(|e| format!("터미널을 초기화하지 못했습니다: {e}"))?;
    // Terminal graphics query must happen before the input EventStream starts
    // reading stdin. Failure just means text-only rendering.
    // Kept for the whole session: the Settings `b` cycle rebuilds the
    // effective picker from this detected base.
    let base_image_picker = match ratatui_image::picker::Picker::from_query_stdio() {
        Ok(picker) => Some(picker),
        Err(error) => {
            tracing::info!(
                %error,
                "terminal graphics query failed; scene images stay disabled"
            );
            None
        }
    };
    // Player identity + author's note: the user-settings overrides win over
    // the base `[player]` profile; the sections are (re)written idempotently,
    // so later persona/note edits just replace them.
    settings::apply_prompt_overlays(
        &mut pack.prompts.system,
        &effective_player_profile(&user_settings, &config.player),
        user_settings.effective_author_note(),
    );
    config.use_director_writer = user_settings
        .use_director_writer
        .unwrap_or(config.use_director_writer);
    // Same inheritance rule for the single-pass streaming render: the
    // persisted Settings `s` toggle wins over the profile's
    // `[provider] stream_turns` once the player has used it. The working
    // copy carries it too — the later model-repair and provider-switch
    // paths rebuild `config.provider` from `provider_config` and must not
    // silently revert the player's choice.
    provider_config.stream_turns = user_settings
        .stream_turns
        .unwrap_or(provider_config.stream_turns);
    config.provider.stream_turns = provider_config.stream_turns;
    let mut resume_engine = saves::restore_autosave(&save_store, &pack);
    let mut engine = EngineState::initial(&pack);
    engine.use_director_writer = config.use_director_writer;
    if let Some(restored) = resume_engine.as_mut() {
        restored.use_director_writer = config.use_director_writer;
    }
    let mut scene_images = SceneImageStores::default();
    let mut app = App::new(build_view(&engine, &pack, None, None));
    app.set_display_rules(compile_display_rules(&user_settings.display_rules));
    // A persisted protocol override (Settings `b` cycle) rides on top of the
    // detected capability; an unrecognized value behaves as auto.
    let (picker, manual) = build_image_picker(
        base_image_picker.as_ref(),
        user_settings.image_protocol.as_deref(),
    );
    app.set_image_picker(picker, manual);
    refresh_scene_images(&mut app, &pack, &engine, &mut scene_images);
    saves::refresh_app_lists(&mut app, &engine, &pack);
    app.set_can_continue(resume_engine.is_some());
    app.set_greetings(greeting_labels(&pack));
    app.set_current_pack_dir(&config.pack_dir.to_string_lossy());
    {
        let roster = settings::provider_roster_labels(&settings::effective_provider_roster(
            &config.providers,
        ));
        app.set_providers(&roster, &provider_config.name);
    }
    // The `v` toggle persists an explicit preference; absent means the [tts]
    // profile's `enabled` default decides.
    let tts_enabled = user_settings.tts_enabled.unwrap_or(config.tts.enabled);
    config.tts.enabled = tts_enabled;
    let images_enabled = user_settings.images_enabled.unwrap_or(true);
    app.set_images_enabled(images_enabled);
    app.set_expressions_enabled(user_settings.expressions_enabled.unwrap_or(true));
    let tts = Arc::new(TtsService::new(config.tts.clone()));
    {
        let status = tts.status().await;
        let images_enabled = app.images_enabled();
        let image_backend = app.image_backend_label();
        let expressions_enabled = app.expressions_enabled();
        settings::refresh_view(
            &mut app,
            &provider_config,
            health,
            config.director_model.clone(),
            config.choice_model.clone(),
            config.writer_model.clone(),
            &status,
            images_enabled,
            image_backend,
            config.use_director_writer,
            config.provider.stream_turns,
            provider.capabilities().supports_streaming,
            expressions_enabled,
            &user_settings,
            &config.player,
        );
    }
    // Start-menu status line: inform the player when running on automatic
    // offline fallback (gray — ready to play) or when a pinned HTTP server is down.
    app.set_home_status(if auto_fallback_builtin {
        Some(settings::builtin_fallback_home_status())
    } else if health == ProviderHealth::Unavailable {
        Some(settings::server_down_home_status())
    } else {
        None
    });
    // AC-1: with an autosave to resume and auto_resume on (default), skip the
    // home menu and land directly in the restored story — one status line
    // tells the player where they are and how back is home. A fresh install
    // (no autosave) or `auto_resume = false` keeps the home-first flow.
    if should_auto_resume(user_settings.auto_resume, resume_engine.is_some()) {
        apply_resume(
            &mut app,
            &mut engine,
            &pack,
            &mut resume_engine,
            &mut scene_images,
            config.use_director_writer,
        );
        app.set_story_notice(Some(AUTO_RESUME_NOTICE));
    } else {
        app.show_home();
    }
    if repaired_main_model {
        user_settings.model = Some(provider_config.model.clone());
    }
    // Pin only the stages the preflight repaired: a stage this run merely
    // kept inheriting stays unrecorded, so later `[[providers]]` default
    // edits keep applying.
    if repaired_stages.any() {
        settings::record_repaired_stages(
            &mut user_settings.stage_models,
            &config.provider.name,
            repaired_stages,
            &StageModels {
                director: config.director_model.clone(),
                choice: config.choice_model.clone(),
                writer: config.writer_model.clone(),
            },
        );
    }
    if repaired_main_model || repaired_stages.any() {
        save_user_settings(&config.save_dir, &user_settings)?;
    }
    let (tx, mut rx) = unbounded_channel();
    let mut cancellation: Option<(story_model::uuid::Uuid, CancellationToken)> = None;
    // Per-turn automation budget (AC-3/5/6) and the AC-4 reprobe schedule.
    let mut automation = TurnAutomation::default();
    // The health probe ran at startup; the schedule counts from there.
    let mut last_probe = Instant::now();

    let mut event_stream = EventStream::new();
    let mut frame_tick = tokio::time::interval(std::time::Duration::from_millis(16));
    frame_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut probe_tick = tokio::time::interval(PROBE_TICK);
    probe_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut needs_redraw = true;
    // Last elapsed-second value the loading overlay was drawn with (1 Hz gate).
    let mut drawn_loading_second: Option<u64> = None;

    loop {
        tokio::select! {
            Some(event_res) = event_stream.next() => {
                let event = event_res.map_err(|e| format!("입력 이벤트를 읽지 못했습니다: {e}"))?;
                match event {
                    Event::Key(key) if key.kind != KeyEventKind::Release => {
                        let pending = app.view.loading.as_ref().map(|v| v.generation_id);
                        if let Some(command) = app.handle_key(key, pending) {
                            note_player_turn_command(&command, &mut automation);
                            if let Err(error) = dispatch(command, &mut engine, &mut app, &pack, &provider, &config, &save_store, &tts, &mut scene_images, &tx, &mut cancellation).await {
                                surface_recoverable_host_error(&mut app, "dispatch(key)", error);
                            }
                        }
                        if let Err(error) = handle_ui_requests(
                            &mut app, &mut engine, &mut pack, &mut provider_config,
                            &mut provider, &mut user_settings, &mut config,
                            &save_store, &tts, &mut scene_images, base_image_picker.as_ref(),
                            &tx, &mut cancellation, &mut resume_engine, &mut automation,
                        ).await {
                            surface_recoverable_host_error(&mut app, "ui_requests(key)", error);
                        }
                        needs_redraw = true;
                    }
                    Event::Mouse(mouse) => {
                        app.handle_mouse(mouse);
                        if let Some(command) = app.take_mouse_command() {
                            note_player_turn_command(&command, &mut automation);
                            if let Err(error) = dispatch(
                                command,
                                &mut engine,
                                &mut app,
                                &pack,
                                &provider,
                                &config,
                                &save_store,
                                &tts,
                                &mut scene_images,
                                &tx,
                                &mut cancellation,
                            )
                            .await {
                                surface_recoverable_host_error(&mut app, "dispatch(mouse)", error);
                                needs_redraw = true;
                            }
                        }
                        if let Err(error) = handle_ui_requests(
                            &mut app, &mut engine, &mut pack, &mut provider_config,
                            &mut provider, &mut user_settings, &mut config,
                            &save_store, &tts, &mut scene_images, base_image_picker.as_ref(),
                            &tx, &mut cancellation, &mut resume_engine, &mut automation,
                        ).await {
                            surface_recoverable_host_error(&mut app, "ui_requests(mouse)", error);
                            needs_redraw = true;
                        }
                        // Movement carries no render state (hover is not
                        // rendered): a full frame per move event starved the
                        // loop while the cursor crossed the window. Clicks
                        // already dispatched their command; the wheel changed
                        // scroll — those redraw.
                        if matches!(
                            mouse.kind,
                            MouseEventKind::Down(_)
                                | MouseEventKind::ScrollUp
                                | MouseEventKind::ScrollDown
                        ) {
                            needs_redraw = true;
                        }
                    }
                    Event::Paste(text) => {
                        app.handle_paste(&text);
                        needs_redraw = true;
                    }
                    Event::Resize(..) => needs_redraw = true,
                    _ => {}
                    }
                if app.should_quit { break; }
            }
            Some(message) = rx.recv() => {
                match handle_host_message(
                    message, &mut engine, &mut app, &pack, &provider, &config, &save_store,
                    &tts, &mut scene_images, &tx, &mut cancellation, &mut automation,
                ).await {
                    Ok(redraw) => needs_redraw = redraw,
                    Err(error) => {
                        surface_recoverable_host_error(&mut app, "host_message", error);
                        needs_redraw = true;
                    }
                }
            }
            _ = frame_tick.tick(), if needs_redraw || app.view.loading.is_some() => {
                // While a generation is pending, the only thing that changes
                // is the elapsed-seconds label — 1 Hz. The 16 ms tick still
                // wakes us, but redrawing at 60 fps for a 1 Hz label burned
                // a measurable slice of a core (probe: 6.9 ms/frame on a
                // 120-turn stack) and starved the terminal. Streaming chunks
                // and events keep their own `needs_redraw`.
                match handle_frame_automation(
                    &mut engine, &mut app, &pack, &mut provider_config, &mut provider,
                    &mut user_settings, &mut config, &save_store, &tts, &mut scene_images,
                    &tx, &mut cancellation, &mut automation, &mut last_probe, Instant::now(),
                ).await {
                    Ok(true) => needs_redraw = true,
                    Ok(false) => {}
                    Err(error) => {
                        surface_recoverable_host_error(&mut app, "frame_automation", error);
                        needs_redraw = true;
                    }
                }
                let loading_second = app
                    .view
                    .loading
                    .as_ref()
                    .and_then(|_| app.generation_elapsed_seconds());
                if needs_redraw || loading_second != drawn_loading_second {
                    terminal
                        .draw(|frame| app.render(frame, frame.area()))
                        .map_err(|e| format!("화면을 그리지 못했습니다: {e}"))?;
                    drawn_loading_second = loading_second;
                }
                needs_redraw = false;
            }
            _ = probe_tick.tick() => {
                // AC-4: the idle loop still checks the reprobe schedule once
                // a second — the redraw-gated frame arm above is disabled
                // while nothing changes, so a server coming back would
                // otherwise go unnoticed until the next keypress.
                match handle_frame_automation(
                    &mut engine, &mut app, &pack, &mut provider_config, &mut provider,
                    &mut user_settings, &mut config, &save_store, &tts, &mut scene_images,
                    &tx, &mut cancellation, &mut automation, &mut last_probe, Instant::now(),
                ).await {
                    Ok(true) => needs_redraw = true,
                    Ok(false) => {}
                    Err(error) => {
                        surface_recoverable_host_error(&mut app, "probe_tick", error);
                        needs_redraw = true;
                    }
                }
            }
        }
    }

    if let Some((_, token)) = cancellation.take() {
        token.cancel();
    }
    Ok(())
}

/// Per-turn automation budget bookkeeping for a player-initiated command: a
/// turn-starting request refills the budget, a cancel drops any armed retry.
/// Manual [R] regenerates deliberately do NOT refill — the overlay retry is
/// player sovereignty, and its failures stay manual.
pub fn note_player_turn_command(command: &EngineCommand, automation: &mut TurnAutomation) {
    match command {
        EngineCommand::SubmitChoice { .. }
        | EngineCommand::SubmitFreeAction { .. }
        | EngineCommand::StartNewGame => automation.reset_turn(),
        EngineCommand::CancelGeneration { .. } => {
            automation.cancel_retry();
        }
        _ => {}
    }
}

/// Handle one provider/engine message from the polling channel. Shared by the
/// interactive loop and the host automation tests, so the AC-3/5/6 behavior
/// is proven against the exact code the loop runs. Returns whether the frame
/// needs a redraw.
#[allow(clippy::too_many_arguments)]
pub async fn handle_host_message(
    message: HostMessage,
    engine: &mut EngineState,
    app: &mut App,
    pack: &StoryPack,
    provider: &Arc<dyn LlmProvider>,
    config: &RuntimeConfig,
    save_store: &SaveStore,
    tts: &TtsService,
    scene_images: &mut SceneImageStores,
    tx: &UnboundedSender<HostMessage>,
    cancellation: &mut Option<(story_model::uuid::Uuid, CancellationToken)>,
    automation: &mut TurnAutomation,
) -> Result<bool, String> {
    match message {
        HostMessage::Generation {
            generation_id,
            model,
            result,
        } => {
            let command = match result {
                Ok(draft) => {
                    app.record_generation_model(generation_id, model);
                    app.record_generation_duration(generation_id);
                    EngineCommand::AcceptGeneration {
                        draft: Box::new(draft),
                    }
                }
                Err(error) => {
                    diagnostics::record_provider_failure(&config.save_dir, generation_id, &error);
                    // AC-3: one automatic retry per turn — unless the player
                    // cancelled, which is a decision, not a failure (the
                    // provider surfaces cancellation as an Err).
                    let cancelled = matches!(error, ProviderError::Cancelled);
                    if !cancelled && automation.arm_retry(generation_id, Instant::now()) {
                        tracing::info!(
                            %generation_id,
                            stage = provider_failure_stage(&error),
                            %error,
                            "generation failed; arming the single automatic retry"
                        );
                        app.set_story_notice(Some(AUTO_RETRY_NOTICE));
                        return Ok(true);
                    }
                    tracing::warn!(
                        %generation_id,
                        stage = provider_failure_stage(&error),
                        %error,
                        "turn generation rejected"
                    );
                    EngineCommand::ProviderFailed {
                        generation_id,
                        error: provider_error_view(error),
                    }
                }
            };
            // The 이어씀 badge: an accept for a side-continue pending that
            // actually appended (the engine demotes Auto → ManualOnly only on
            // a non-empty continuation).
            let side_continue_accept = matches!(command, EngineCommand::AcceptGeneration { .. })
                && engine
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.side_kind == Some(SideGenerationKind::Continue));
            let previous_turn = app.view.status.turn;
            dispatch(
                command,
                engine,
                app,
                pack,
                provider,
                config,
                save_store,
                tts,
                scene_images,
                tx,
                cancellation,
            )
            .await?;
            if app.view.status.turn > previous_turn {
                // A committed turn refills the automation budget.
                automation.reset_turn();
            }
            if side_continue_accept && !app.view.empty_side_result {
                app.set_story_notice(Some(CONTINUE_BADGE));
            }
            post_accept_automation(
                engine,
                app,
                pack,
                provider,
                config,
                save_store,
                tts,
                scene_images,
                tx,
                cancellation,
                automation,
                // The badge above is the fresher surface for a manual or
                // automatic continue that just appended.
                !side_continue_accept,
            )
            .await?;
        }
        HostMessage::Director {
            generation_id,
            result,
        } => {
            let command = match result {
                Ok(plan) => EngineCommand::AcceptDirectorPlan {
                    generation_id,
                    plan: Box::new(plan),
                },
                Err(error) => {
                    diagnostics::record_provider_failure(&config.save_dir, generation_id, &error);
                    // A plan-shape failure (model could not emit valid
                    // JSON) degrades gracefully to a single-pass turn
                    // so a weak Director model never blocks the player.
                    // Transport/timeout failures still surface an error.
                    if matches!(error, ProviderError::InvalidGeneration(_)) {
                        tracing::warn!(
                            %generation_id,
                            "Director could not produce a plan; falling back to single-pass generation"
                        );
                        EngineCommand::FallbackToSinglePass { generation_id }
                    } else {
                        // AC-3: a transport/timeout failure gets the same one
                        // automatic retry per turn as a single-pass generation
                        // — unless the player cancelled, which is a decision,
                        // not a failure.
                        let cancelled = matches!(error, ProviderError::Cancelled);
                        if !cancelled && automation.arm_retry(generation_id, Instant::now()) {
                            tracing::info!(
                                %generation_id,
                                stage = provider_failure_stage(&error),
                                %error,
                                "director stage failed; arming the single automatic retry"
                            );
                            app.set_story_notice(Some(AUTO_RETRY_NOTICE));
                            return Ok(true);
                        }
                        tracing::warn!(
                            %generation_id,
                            stage = provider_failure_stage(&error),
                            %error,
                            "director stage rejected"
                        );
                        EngineCommand::ProviderFailed {
                            generation_id,
                            error: provider_error_view(error),
                        }
                    }
                }
            };
            dispatch(
                command,
                engine,
                app,
                pack,
                provider,
                config,
                save_store,
                tts,
                scene_images,
                tx,
                cancellation,
            )
            .await?;
        }
        HostMessage::Writer {
            generation_id,
            model,
            result,
        } => {
            let command = match result {
                Ok(finish) => {
                    app.record_generation_model(generation_id, model);
                    app.record_generation_duration(generation_id);
                    EngineCommand::AcceptWriterFinish {
                        finish: Box::new(finish),
                    }
                }
                Err(error) => {
                    diagnostics::record_provider_failure(&config.save_dir, generation_id, &error);
                    // AC-3: a Writer-stage transport/timeout failure gets the
                    // same one automatic retry per turn as a single-pass
                    // generation — unless the player cancelled, which is a
                    // decision, not a failure.
                    let cancelled = matches!(error, ProviderError::Cancelled);
                    if !cancelled && automation.arm_retry(generation_id, Instant::now()) {
                        tracing::info!(
                            %generation_id,
                            stage = provider_failure_stage(&error),
                            %error,
                            "writer stage failed; arming the single automatic retry"
                        );
                        app.set_story_notice(Some(AUTO_RETRY_NOTICE));
                        return Ok(true);
                    }
                    tracing::warn!(
                        %generation_id,
                        stage = provider_failure_stage(&error),
                        %error,
                        "writer stage rejected"
                    );
                    EngineCommand::ProviderFailed {
                        generation_id,
                        error: provider_error_view(error),
                    }
                }
            };
            let previous_turn = app.view.status.turn;
            dispatch(
                command,
                engine,
                app,
                pack,
                provider,
                config,
                save_store,
                tts,
                scene_images,
                tx,
                cancellation,
            )
            .await?;
            if app.view.status.turn > previous_turn {
                automation.reset_turn();
            }
            // AC-5/AC-8: react to the Writer path's truncation flag.
            post_accept_automation(
                engine,
                app,
                pack,
                provider,
                config,
                save_store,
                tts,
                scene_images,
                tx,
                cancellation,
                automation,
                true,
            )
            .await?;
        }
        HostMessage::ImportFinished { path, result } => match result {
            Ok(outcome) => app.show_import_report(&path, outcome.lines, Some(outcome.pack_dir)),
            Err(error) => {
                app.show_import_report(&path, vec![format!("임포트 실패: {error}")], None)
            }
        },
        HostMessage::WriterChunk {
            generation_id,
            text,
        } => {
            app.push_writer_chunk(generation_id, &text);
        }
    }
    Ok(true)
}

/// AC-5/6/8 post-accept automation: react to the just-published view —
/// auto-continue a truncated turn once, re-dispatch a side command whose
/// result came back empty once, and surface the manual-only guide. Each
/// action is capped once per turn by [`TurnAutomation`].
#[allow(clippy::too_many_arguments)]
pub async fn post_accept_automation(
    engine: &mut EngineState,
    app: &mut App,
    pack: &StoryPack,
    provider: &Arc<dyn LlmProvider>,
    config: &RuntimeConfig,
    save_store: &SaveStore,
    tts: &TtsService,
    scene_images: &mut SceneImageStores,
    tx: &UnboundedSender<HostMessage>,
    cancellation: &mut Option<(story_model::uuid::Uuid, CancellationToken)>,
    automation: &mut TurnAutomation,
    allow_manual_guide: bool,
) -> Result<(), String> {
    // AC-6: an empty side result applied nothing — re-issue the same side
    // command once; if the re-run is empty too, guide only.
    if app.view.empty_side_result {
        if !automation.side_retry_spent
            && let Some(command) = automation.last_side_command().cloned()
        {
            automation.side_retry_spent = true;
            app.set_story_notice(Some(EMPTY_SIDE_NOTICE));
            dispatch(
                command,
                engine,
                app,
                pack,
                provider,
                config,
                save_store,
                tts,
                scene_images,
                tx,
                cancellation,
            )
            .await?;
            return Ok(());
        }
        app.set_story_notice(Some(EMPTY_SIDE_NOTICE));
        return Ok(());
    }
    match app.view.truncation {
        TruncationStatus::Auto
            if !automation.auto_continue_spent
                && engine.pending.is_none()
                && engine.retry_pending.is_none() =>
        {
            // AC-5: one automatic continue per truncated turn. The engine
            // demotes Auto → ManualOnly after a non-empty append; this flag
            // additionally keeps an empty-accept from re-firing.
            automation.auto_continue_spent = true;
            automation.note_side_command(EngineCommand::ContinueNarration);
            dispatch(
                EngineCommand::ContinueNarration,
                engine,
                app,
                pack,
                provider,
                config,
                save_store,
                tts,
                scene_images,
                tx,
                cancellation,
            )
            .await?;
        }
        // AC-8: the repair budget was spent — never auto-continue; the
        // shortest manual guide instead.
        TruncationStatus::ManualOnly if allow_manual_guide => {
            app.set_story_notice(Some(MANUAL_CONTINUE_NOTICE));
        }
        TruncationStatus::ManualOnly | TruncationStatus::Auto | TruncationStatus::None => {}
    }
    Ok(())
}

/// AC-3/AC-4 frame-time automation: fire the due automatic retry and run the
/// periodic reprobe schedule. `now` is injected so tests can cross the delay
/// without sleeping. Returns whether a redraw is needed.
#[allow(clippy::too_many_arguments)]
pub async fn handle_frame_automation(
    engine: &mut EngineState,
    app: &mut App,
    pack: &StoryPack,
    provider_config: &mut story_provider::ProviderConfig,
    provider: &mut Arc<dyn LlmProvider>,
    user_settings: &mut UserSettings,
    config: &mut RuntimeConfig,
    save_store: &SaveStore,
    tts: &TtsService,
    scene_images: &mut SceneImageStores,
    tx: &UnboundedSender<HostMessage>,
    cancellation: &mut Option<(story_model::uuid::Uuid, CancellationToken)>,
    automation: &mut TurnAutomation,
    last_probe: &mut Instant,
    now: Instant,
) -> Result<bool, String> {
    let mut needs_redraw = false;
    if automation.take_due_retry(now) {
        // A cancel between arming and firing left nothing to regenerate.
        if engine.pending.is_some() || engine.retry_pending.is_some() {
            tracing::info!("automatic retry firing");
            dispatch(
                EngineCommand::RegenerateCurrentTurn,
                engine,
                app,
                pack,
                provider,
                config,
                save_store,
                tts,
                scene_images,
                tx,
                cancellation,
            )
            .await?;
        }
        app.set_story_notice(None);
        needs_redraw = true;
    }
    if probe_due(now, *last_probe, app.view.loading.is_some())
        && (settings::should_auto_discover(provider_config, user_settings)
            || app
                .provider_health()
                .is_none_or(|health| health == ProviderHealth::Unavailable))
    {
        *last_probe = now;
        if settings::reprobe_availability(
            app,
            provider_config,
            provider,
            user_settings,
            config,
            tts,
        )
        .await
        {
            needs_redraw = true;
        }
    }
    Ok(needs_redraw)
}

/// Switch the session to the pack at `pack_dir`: reset the engine, drop the
/// stale autosave resume, and refresh every pack-derived view. The old pack
/// stays in charge when the new one fails to load.
#[allow(clippy::too_many_arguments)]
fn swap_pack(
    app: &mut App,
    pack_dir: &str,
    pack: &mut StoryPack,
    engine: &mut EngineState,
    scene_images: &mut SceneImageStores,
    config: &RuntimeConfig,
    player: &PlayerProfile,
    author_note: Option<&str>,
    cancellation: &mut Option<(story_model::uuid::Uuid, CancellationToken)>,
    resume_engine: &mut Option<EngineState>,
) {
    let new_pack = match PackLoader::load(std::path::Path::new(pack_dir)) {
        Ok(pack) => pack,
        Err(error) => {
            tracing::warn!(pack = %pack_dir, %error, "imported pack failed to load");
            app.view.error = Some(story_model::DomainErrorView {
                code: "pack_load_failed".to_string(),
                title_ko: "팩을 불러오지 못했습니다".to_string(),
                message_ko: error.to_string(),
                recoverable: true,
            });
            return;
        }
    };
    if let Some((_, token)) = cancellation.take() {
        token.cancel();
    }
    *pack = new_pack;
    settings::apply_prompt_overlays(&mut pack.prompts.system, player, author_note);
    *engine = EngineState::initial(pack);
    engine.use_director_writer = config.use_director_writer;
    *resume_engine = None;
    // Paired clear: the app's encoded images and this store must reset
    // together or the previous pack's images survive the switch.
    reset_scene_images(app, scene_images);
    app.set_view(build_view(engine, pack, None, None));
    saves::refresh_app_lists(app, engine, pack);
    app.follow_story_end();
    app.set_can_continue(false);
    app.set_greetings(greeting_labels(pack));
    app.set_current_pack_dir(pack_dir);
    refresh_scene_images(app, pack, engine, scene_images);
    tracing::info!(pack = %pack_dir, "switched to the imported pack");
}

/// Alternate-opening labels for the picker: index 0 is always the pack's own
/// opening; `greetings/N.md` follow (importer convention, contiguous).
fn greeting_labels(pack: &StoryPack) -> Vec<String> {
    let mut labels = vec!["기본 오프닝".to_string()];
    for index in 1..=64 {
        let path = pack.pack_root.join("greetings").join(format!("{index}.md"));
        let Ok(text) = std::fs::read_to_string(&path) else {
            break;
        };
        let first = text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("대체 오프닝");
        let label: String = first.chars().take(40).collect();
        labels.push(label);
    }
    labels
}

/// Read the greeting file the player picked (`index > 0`; 0 keeps the pack
/// opening).
fn read_greeting(pack: &StoryPack, index: usize) -> Option<String> {
    if index == 0 {
        return None;
    }
    let path = pack.pack_root.join("greetings").join(format!("{index}.md"));
    std::fs::read_to_string(path).ok()
}

/// Both resolved-image caches the host owns — location art and expression
/// sprites. One bundle so every dispatch/reset/refresh signature threads a
/// single handle and the pack-switch lockstep cannot forget one side.
#[derive(Default)]
pub struct SceneImageStores {
    pub locations: images::LocationImageStore,
    pub expressions: images::ExpressionImageStore,
}

/// Refresh both image stores and push newly resolved images into the view.
/// Runs on every publish; a location or emotion decodes at most once per
/// pack session, so the common case is a no-op scan.
fn refresh_scene_images(
    app: &mut App,
    pack: &StoryPack,
    engine: &EngineState,
    stores: &mut SceneImageStores,
) {
    refresh_location_images(app, pack, engine, stores);
    refresh_expression_images(app, pack, engine, stores);
}

/// Refresh the per-location image store. The opening location is always in
/// play (the empty-stack opening shows its image).
fn refresh_location_images(
    app: &mut App,
    pack: &StoryPack,
    engine: &EngineState,
    stores: &mut SceneImageStores,
) {
    let primary = engine.game.characters.keys().next().map(|id| id.0.clone());
    let initial = &pack.metadata.initial_state.location;
    // Distinct locations in play, order-preserving.
    let mut seen = std::collections::HashSet::new();
    let mut locations = vec![initial.0.clone()];
    seen.insert(initial.0.clone());
    for location in story_tui::location_timeline(&engine.game.history, initial)
        .into_iter()
        .flatten()
    {
        if seen.insert(location.clone()) {
            locations.push(location);
        }
    }
    for (location, resolved) in stores
        .locations
        .load_missing(pack, &locations, primary.as_deref())
    {
        app.set_location_image(&location, resolved);
    }
}

/// Refresh the per-emotion sprite store. Zero inference: the emotions come
/// from `classify_emotion` over the opening narration and the history, so a
/// weak local setup pays no extra generation for expression sprites.
/// Skipped entirely while the player keeps sprites off (`f`).
fn refresh_expression_images(
    app: &mut App,
    pack: &StoryPack,
    engine: &EngineState,
    stores: &mut SceneImageStores,
) {
    if !app.expressions_enabled() {
        return;
    }
    let Some(character) = engine.game.characters.keys().next().map(|id| id.0.clone()) else {
        return;
    };
    // Distinct emotions in play, order-preserving; the opening narration is
    // the transcript's first beat and is classified exactly like the App's
    // empty-stack branch classifies it.
    let mut seen = std::collections::HashSet::new();
    let mut emotions = Vec::new();
    for emotion in std::iter::once(story_tui::classify_emotion(&app.view.narration))
        .chain(story_tui::emotion_timeline(&engine.game.history))
    {
        if seen.insert(emotion) {
            emotions.push(emotion);
        }
    }
    for (emotion, resolved) in stores.expressions.load_missing(pack, &character, &emotions) {
        app.set_expression_image(emotion, resolved);
    }
}

/// Clear the per-location AND per-emotion images on BOTH sides in lockstep.
///
/// Invariant: the host's decode stores and the app's encoded image maps must
/// be reset together. Clearing only one side lets the previous pack's images
/// survive a pack switch — the stale-portrait bug fixed 2026-10-19 (a fresh
/// cache that answered `Unchanged` instead of `Clear` for an imageless pack).
fn reset_scene_images(app: &mut App, stores: &mut SceneImageStores) {
    stores.locations.clear();
    stores.expressions.clear();
    app.reset_location_images();
    app.reset_expression_images();
}

/// Recognized `image_protocol` values in cycle order after auto.
const IMAGE_PROTOCOL_CYCLE: [&str; 4] = ["kitty", "sixel", "iterm2", "halfblocks"];

/// Next value for the Settings `b` cycle: auto (`None`) first, then each
/// recognized protocol in [`IMAGE_PROTOCOL_CYCLE`] order, wrapping back to
/// auto. An unrecognized stored value is treated as auto. Without a detected
/// picker (`base_available = false`) the cell size is unknown, so only
/// halfblocks — which needs no cell query — can be forced.
fn next_image_protocol_setting(current: Option<&str>, base_available: bool) -> Option<String> {
    let position = current.and_then(|value| {
        IMAGE_PROTOCOL_CYCLE
            .iter()
            .position(|candidate| *candidate == value)
    });
    let start = position.map_or(0, |index| index + 1);
    IMAGE_PROTOCOL_CYCLE[start..]
        .iter()
        .find(|candidate| base_available || **candidate == "halfblocks")
        .map(|candidate| candidate.to_string()) // empty slice = back to auto
}

/// Build the effective picker for an `image_protocol` setting. Auto keeps the
/// detected picker; halfblocks is always buildable; the other protocols ride
/// on the detected picker's cell size, so they require a base. Unrecognized
/// values behave as auto. Returns the picker plus whether it is player-forced.
fn build_image_picker(
    base: Option<&ratatui_image::picker::Picker>,
    setting: Option<&str>,
) -> (Option<ratatui_image::picker::Picker>, bool) {
    let forced = match (setting, base.is_some()) {
        (Some("kitty"), true) => Some(ratatui_image::picker::ProtocolType::Kitty),
        (Some("sixel"), true) => Some(ratatui_image::picker::ProtocolType::Sixel),
        (Some("iterm2"), true) => Some(ratatui_image::picker::ProtocolType::Iterm2),
        (Some("halfblocks"), _) => Some(ratatui_image::picker::ProtocolType::Halfblocks),
        _ => None,
    };
    match forced {
        Some(protocol) => {
            let mut picker = match base {
                Some(base) => base.clone(),
                None => ratatui_image::picker::Picker::halfblocks(),
            };
            picker.set_protocol_type(protocol);
            (Some(picker), true)
        }
        None => (base.cloned(), false),
    }
}

#[allow(clippy::too_many_arguments)] // consumes all host-owned UI requests in one place
async fn handle_ui_requests(
    app: &mut App,
    engine: &mut EngineState,
    pack: &mut StoryPack,
    provider_config: &mut story_provider::ProviderConfig,
    provider: &mut Arc<dyn LlmProvider>,
    user_settings: &mut UserSettings,
    config: &mut RuntimeConfig,
    save_store: &SaveStore,
    tts: &TtsService,
    scene_images: &mut SceneImageStores,
    base_image_picker: Option<&ratatui_image::picker::Picker>,
    tx: &UnboundedSender<HostMessage>,
    cancellation: &mut Option<(story_model::uuid::Uuid, CancellationToken)>,
    resume_engine: &mut Option<EngineState>,
    automation: &mut TurnAutomation,
) -> Result<(), String> {
    if app.take_settings_opened() {
        if let Some((generation_id, token)) = cancellation.take() {
            token.cancel();
            automation.cancel_retry();
            app.set_story_notice(None);
            dispatch(
                EngineCommand::CancelGeneration { generation_id },
                engine,
                app,
                pack,
                provider,
                config,
                save_store,
                tts,
                scene_images,
                tx,
                cancellation,
            )
            .await?;
        }
        // Keep the "turn the server on, then open settings" promise: while
        // no model is selected or running in auto-fallback built-in mode,
        // opening settings retries auto-setup first.
        let reprobe = settings::reprobe_if_unconfigured(
            app,
            provider_config,
            provider,
            user_settings,
            config,
            tts,
        )
        .await;
        match &reprobe {
            // An adopted server already refreshed the model list above.
            Some(settings::DiscoveryOutcome::Connected { .. }) => {
                if app.home_status_text()
                    == Some(settings::builtin_fallback_home_status().0.as_str())
                {
                    app.set_home_status(None);
                }
            }
            Some(other) => {
                app.set_home_status(Some(settings::unconfigured_home_status(other)));
            }
            None => {
                settings::refresh_model_list(app, provider.as_ref(), provider_config, false).await
            }
        }
        // Push the host-owned data the new settings modals edit: the note
        // prefill and the pack's lorebook (edits hot-swap via the engine and
        // persist only when the pack ships its lorebook as a file).
        app.set_author_note(user_settings.effective_author_note().unwrap_or_default());
        let persisted =
            story_persistence::lorebook_path(std::path::Path::new(app.current_pack_dir())).exists();
        app.set_lorebook_data(&pack.lorebook.entries, persisted);
    }
    if app.take_model_reload_requested() {
        settings::refresh_model_list(app, provider.as_ref(), provider_config, true).await;
    }
    if let Some(name) = app.take_selected_provider() {
        let roster = settings::effective_provider_roster(&config.providers);
        settings::apply_provider(
            app,
            &name,
            &roster,
            provider_config,
            provider,
            user_settings,
            config,
            tts,
        )
        .await?;
    }
    if let Some(model) = app.take_selected_model() {
        settings::apply_model(
            app,
            model,
            config,
            provider_config,
            provider,
            user_settings,
            &config.save_dir,
            tts,
        )
        .await?;
    }
    if let Some(model) = app.take_selected_director_model() {
        settings::apply_director_model(
            app,
            model,
            config,
            provider_config,
            provider.as_ref(),
            user_settings,
            tts,
        )
        .await?;
    }
    if let Some(model) = app.take_selected_choice_model() {
        settings::apply_choice_model(
            app,
            model,
            config,
            provider_config,
            provider.as_ref(),
            user_settings,
            tts,
        )
        .await?;
    }
    if let Some(model) = app.take_selected_writer_model() {
        settings::apply_writer_model(
            app,
            model,
            config,
            provider_config,
            provider.as_ref(),
            user_settings,
            tts,
        )
        .await?;
    }
    if let Some(request) = app.take_home_request() {
        match request {
            HomeRequest::StartNew => {
                // An alternate opening pick rewrites opening_md for this
                // session (the same host-owned pack mutation the player
                // profile already does).
                if let Some(index) = app.take_greeting_selection()
                    && index > 0
                    && let Some(text) = read_greeting(pack, index)
                {
                    pack.opening_md = text;
                }
                let mut new_engine = EngineState::initial(pack);
                new_engine.use_director_writer = config.use_director_writer;
                *engine = new_engine;
                app.set_view(build_view(engine, pack, None, None));
                saves::refresh_app_lists(app, engine, pack);
                app.follow_story_end();
                app.close_home();
                refresh_scene_images(app, pack, engine, scene_images);
                // A fresh run refills the per-turn automation budget.
                automation.reset_turn();
                dispatch(
                    EngineCommand::StartNewGame,
                    engine,
                    app,
                    pack,
                    provider,
                    config,
                    save_store,
                    tts,
                    scene_images,
                    tx,
                    cancellation,
                )
                .await?;
            }
            HomeRequest::Continue => {
                apply_resume(
                    app,
                    engine,
                    pack,
                    resume_engine,
                    scene_images,
                    config.use_director_writer,
                );
            }
            HomeRequest::PickPack => {
                let current = app.current_pack_dir().to_string();
                let entries = packs::scan_packs(std::path::Path::new(&current));
                let dirs: Vec<String> = entries.iter().map(|e| e.dir.clone()).collect();
                let labels: Vec<String> = entries
                    .iter()
                    .map(|e| format!("{} · {}", e.title, e.language))
                    .collect();
                app.open_pack_picker(dirs, labels, &current);
            }
            HomeRequest::ReturnToGame => app.close_home(),
        }
    }
    if app.take_images_toggle_request() {
        let enabled = !app.images_enabled();
        app.set_images_enabled(enabled);
        user_settings.images_enabled = Some(enabled);
        save_user_settings(&config.save_dir, user_settings)?;
        app.update_settings_images(enabled, app.image_backend_label());
    }
    if app.take_image_backend_cycle_request() {
        let next = next_image_protocol_setting(
            user_settings.image_protocol.as_deref(),
            base_image_picker.is_some(),
        );
        user_settings.image_protocol = next.clone();
        save_user_settings(&config.save_dir, user_settings)?;
        let (picker, manual) = build_image_picker(base_image_picker, next.as_deref());
        app.set_image_picker(picker, manual);
        app.update_settings_images(app.images_enabled(), app.image_backend_label());
        // Encoded images still carry the previous protocol — resolve every
        // location/emotion again so the transcript reflects the new one
        // immediately (same lockstep as a pack switch).
        reset_scene_images(app, scene_images);
        refresh_scene_images(app, pack, engine, scene_images);
    }
    if app.take_expressions_toggle_request() {
        let enabled = !app.expressions_enabled();
        app.set_expressions_enabled(enabled);
        user_settings.expressions_enabled = Some(enabled);
        save_user_settings(&config.save_dir, user_settings)?;
        app.update_settings_expressions(enabled);
        if enabled {
            // A session that STARTED with sprites off has a cold sprite
            // store — resolve the beats in play now instead of waiting for
            // the next publish, so re-enabling really is instant.
            refresh_scene_images(app, pack, engine, scene_images);
        }
    }
    if app.take_tts_toggle_request() {
        let enabled = !tts.enabled().await;
        tts.set_enabled(enabled).await;
        user_settings.tts_enabled = Some(enabled);
        save_user_settings(&config.save_dir, user_settings)?;
        let status = tts.status().await;
        app.update_settings_tts(story_tui::TtsDisplay {
            enabled: status.enabled,
            backend: status.backend.as_str().to_string(),
            voice: status.voice.clone(),
        });
    }
    if app.take_generation_toggle_request() {
        // Flip the session value AND the live engine: the next
        // SubmitChoice/SubmitFreeAction reads the engine flag, while saves
        // and new games read the config. An in-flight generation (cancelled
        // when Settings opened) is unaffected — accept commands never
        // consult the flag.
        let two_stage = !config.use_director_writer;
        config.use_director_writer = two_stage;
        engine.use_director_writer = two_stage;
        user_settings.use_director_writer = Some(two_stage);
        save_user_settings(&config.save_dir, user_settings)?;
        app.update_settings_generation(two_stage);
    }
    if app.take_stream_toggle_request() {
        // Same next-generation semantics as the `g` toggle: the host's
        // config decides at RequestGeneration time, so a turn already in
        // flight (cancelled when Settings opened) finishes under the old
        // setting. The two-stage path ignores this flag — its Writer always
        // streams — which the Settings row notes. The working copy stays in
        // lockstep so the reprobe/provider-switch paths that rebuild
        // `config.provider` from `provider_config` never resurrect a stale
        // value (review R1).
        let stream = !config.provider.stream_turns;
        config.provider.stream_turns = stream;
        provider_config.stream_turns = stream;
        user_settings.stream_turns = Some(stream);
        save_user_settings(&config.save_dir, user_settings)?;
        app.update_settings_stream(stream);
    }
    if app.take_sampling_style_cycle_request() {
        // The preset rides the request config, so the next turn generates
        // with the new triple; the provider rebuild makes that certain.
        settings::cycle_sampling_style(app, provider_config, provider, user_settings, config)
            .await?;
    }
    if let Some((name, persona)) = app.take_persona_submit_request() {
        // The pack's system prompt is re-read on every turn build, so the
        // rewritten profile section applies from the next turn on.
        let (label, name_text, persona_text) =
            settings::activate_persona(name, persona, pack, user_settings, &config.player, config)?;
        app.update_settings_persona(label, name_text, persona_text);
    }
    if let Some(note) = app.take_author_note_submit() {
        // Same next-turn semantics as the persona: the pack's system prompt
        // is re-read on every turn build.
        let effective =
            settings::activate_author_note(note, pack, user_settings, &config.player, config)?;
        app.set_author_note(&effective);
    }
    if let Some(entries) = app.take_lorebook_request() {
        // Hot-swap the session first (the very next turn matches against the
        // new entries), then persist when the pack ships lorebook.toml —
        // otherwise the edit stays session-only, matching the dialog notice.
        dispatch(
            EngineCommand::SetLorebookEntries {
                entries: entries.clone(),
            },
            engine,
            app,
            pack,
            provider,
            config,
            save_store,
            tts,
            scene_images,
            tx,
            cancellation,
        )
        .await?;
        pack.lorebook.entries = entries;
        let pack_dir = std::path::Path::new(app.current_pack_dir());
        if let Err(message) = story_persistence::save_lorebook(pack_dir, &pack.lorebook) {
            tracing::info!(%message, "lorebook edit stays session-only");
        }
    }
    if app.take_continue_request() {
        // Side generation: appends to the last narration without a new turn.
        automation.note_side_command(EngineCommand::ContinueNarration);
        dispatch(
            EngineCommand::ContinueNarration,
            engine,
            app,
            pack,
            provider,
            config,
            save_store,
            tts,
            scene_images,
            tx,
            cancellation,
        )
        .await?;
    }
    if app.take_group_scene_request() {
        // Side generation: a cast group-conversation scene bit attaches to
        // the last committed narration (choices/state stay untouched).
        automation.note_side_command(EngineCommand::GroupScene);
        dispatch(
            EngineCommand::GroupScene,
            engine,
            app,
            pack,
            provider,
            config,
            save_store,
            tts,
            scene_images,
            tx,
            cancellation,
        )
        .await?;
    }
    if app.take_suggest_action_request() {
        // Side generation: the suggested action lands in the input buffer.
        automation.note_side_command(EngineCommand::SuggestAction);
        dispatch(
            EngineCommand::SuggestAction,
            engine,
            app,
            pack,
            provider,
            config,
            save_store,
            tts,
            scene_images,
            tx,
            cancellation,
        )
        .await?;
    }
    if app.take_cycle_variant_back_request() {
        // Swap the last committed turn's narration to the previous kept
        // variant (engine rewrites + autosaves; the view carries the badge).
        dispatch(
            EngineCommand::CycleTurnVariant { delta: -1 },
            engine,
            app,
            pack,
            provider,
            config,
            save_store,
            tts,
            scene_images,
            tx,
            cancellation,
        )
        .await?;
    }
    if app.take_cycle_variant_forward_request() {
        dispatch(
            EngineCommand::CycleTurnVariant { delta: 1 },
            engine,
            app,
            pack,
            provider,
            config,
            save_store,
            tts,
            scene_images,
            tx,
            cancellation,
        )
        .await?;
    }
    if let Some(digit) = app.take_quick_action_request()
        && let Some(text) = user_settings.quick_action(&digit)
    {
        app.set_input_text(text);
    }
    // A completed suggest generation leaves its text for the input buffer;
    // the player edits and sends it themselves.
    if let Some(suggestion) = engine.take_suggested_action() {
        app.set_input_text(&suggestion);
    }
    if let Some(path) = app.take_import_request() {
        // ADR-003: the host owns all file I/O. The importer runs off the UI
        // loop; its outcome returns as `HostMessage::ImportFinished`.
        let tx = tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = story_import::import_card(
                std::path::Path::new(&path),
                None,
                &story_import::ImportOptions::default(),
            )
            .map(|output| {
                let report = &output.report;
                let mut lines = Vec::new();
                if report.lorebook_entries > 0 {
                    lines.push(format!("로어북 엔트리 {}개", report.lorebook_entries));
                }
                for mapped in report.mapped.iter().take(5) {
                    lines.push(format!("· {mapped}"));
                }
                for note in report.approximations.iter().take(3) {
                    lines.push(format!("≈ {note}"));
                }
                for warning in &report.warnings {
                    lines.push(format!("! {warning}"));
                }
                if !report.ignored_fields.is_empty() {
                    lines.push(format!(
                        "무시된 필드 {}건 — sources/card.json에 보존",
                        report.ignored_fields.len()
                    ));
                }
                ImportOutcome {
                    pack_dir: output.pack_dir.display().to_string(),
                    lines,
                }
            });
            let _ = tx.send(HostMessage::ImportFinished { path, result });
        });
    }
    if let Some(pack_dir) = app.take_pack_swap_request() {
        let player = effective_player_profile(user_settings, &config.player);
        swap_pack(
            app,
            &pack_dir,
            pack,
            engine,
            scene_images,
            config,
            &player,
            user_settings.effective_author_note(),
            cancellation,
            resume_engine,
        );
    }
    if let Some(request) = app.take_browse_request() {
        // ADR-003: the host owns all file I/O. Like the pack scan, a local
        // read_dir is fast enough to run inline in the event loop. An empty
        // request asks for the start directory (HOME, falling back to `.`).
        let target = if request.is_empty() {
            std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .filter(|path| path.is_dir())
                .unwrap_or_else(|| std::path::PathBuf::from("."))
        } else {
            std::path::PathBuf::from(request)
        };
        let entries: Vec<(String, bool)> = packs::list_dir_for_browse(&target)
            .unwrap_or_default()
            .into_iter()
            .map(|entry| (entry.name, entry.is_dir))
            .collect();
        app.open_file_browser(&target.display().to_string(), entries);
    }
    if let Some(dir) = app.take_pack_delete_request() {
        // Guarded in packs::delete_pack; the picker reopens with the
        // remaining packs either way.
        let current = app.current_pack_dir().to_string();
        let current_path = std::path::Path::new(&current);
        let scan_root = current_path
            .parent()
            .map(|parent| parent.to_path_buf())
            .unwrap_or_else(|| current_path.to_path_buf());
        let outcome = packs::delete_pack(std::path::Path::new(&dir), &scan_root, current_path);
        if let Err(error) = &outcome {
            app.set_home_status(Some((error.clone(), true)));
        }
        let entries = packs::scan_packs(current_path);
        let dirs: Vec<String> = entries.iter().map(|e| e.dir.clone()).collect();
        let labels: Vec<String> = entries
            .iter()
            .map(|e| format!("{} · {}", e.title, e.language))
            .collect();
        app.open_pack_picker(dirs, labels, &current);
        // After the reopen above: the notice renders inside the picker so a
        // tall listing can never hide it (G3 F-3).
        if let Err(error) = outcome {
            app.set_pack_picker_notice(Some(error));
        }
    }
    if let Some(request) = app.take_save_load_request() {
        saves::handle_request(
            request,
            engine,
            app,
            pack,
            save_store,
            config.use_director_writer,
            cancellation,
        );
        refresh_scene_images(app, pack, engine, scene_images);
    }
    Ok(())
}

/// Compile the TOML-only display rules (`[[display_rules]]`) for the
/// transcript renderer: capped at [`storytui::config::MAX_DISPLAY_RULES`],
/// disabled rules are kept out, and an invalid pattern is skipped with a
/// warning rather than failing the session. Pure.
fn compile_display_rules(rules: &[storytui::config::DisplayRule]) -> Vec<(regex::Regex, String)> {
    let mut compiled = Vec::new();
    // The cap counts APPLIED rules: neither a disabled rule nor an invalid
    // pattern may eat a slot a later valid rule could use.
    for rule in rules.iter().filter(|rule| rule.enabled) {
        if compiled.len() >= storytui::config::MAX_DISPLAY_RULES {
            break;
        }
        match regex::Regex::new(&rule.pattern) {
            Ok(pattern) => compiled.push((pattern, rule.replacement.clone())),
            Err(error) => {
                tracing::warn!(pattern = %rule.pattern, %error, "invalid display rule skipped");
            }
        }
    }
    compiled
}

/// Capture the final sent prompt for the Settings `r` viewer: system-role
/// messages form the system section, everything else the user section.
/// Takes the message slice directly — the single-pass, Director, and Writer
/// requests all carry the same `messages` shape. Session memory only — never
/// persisted, never logged.
fn capture_prompt_preview(app: &mut App, messages: &[story_model::ChatMessage]) {
    let mut system = String::new();
    let mut user = String::new();
    for message in messages {
        if message.role == "system" {
            system.push_str(&message.content);
            system.push('\n');
        } else {
            if !user.is_empty() {
                user.push_str("\n---\n");
            }
            user.push_str(&message.content);
        }
    }
    app.set_prompt_preview(&system, &user);
}

/// Pump a provider streaming call's writer events to the UI while awaiting
/// the final result. Shared by the MVP turn path (`generate_turn_stream`)
/// and the Director/Writer path (`write_stream`) so both forward chunks
/// identically.
async fn pump_writer_events<T, F>(
    mut events_rx: tokio::sync::mpsc::Receiver<story_model::WriterEvent>,
    write: F,
    tx: &UnboundedSender<HostMessage>,
    generation_id: story_model::uuid::Uuid,
) -> Result<T, ProviderError>
where
    F: std::future::Future<Output = Result<T, ProviderError>>,
{
    tokio::pin!(write);
    loop {
        tokio::select! {
            event = events_rx.recv() => match event {
                Some(story_model::WriterEvent::Chunk { text }) => {
                    let _ = tx.send(HostMessage::WriterChunk { generation_id, text });
                }
                Some(story_model::WriterEvent::Error { detail }) => {
                    tracing::warn!(%generation_id, %detail, "writer stream soft error");
                }
                Some(story_model::WriterEvent::Done { .. }) | None => {}
            },
            result = &mut write => break result,
        }
    }
}

fn surface_recoverable_host_error(app: &mut App, context: &str, error: String) {
    tracing::error!(%context, %error, "recoverable host error surfaced instead of exiting");
    app.view.error = Some(DomainErrorView {
        code: "host_error".to_string(),
        title_ko: "작업을 처리할 수 없습니다".to_string(),
        message_ko: error,
        recoverable: true,
    });
}

#[allow(clippy::too_many_arguments)] // host adapter keeps all effect-owned handles explicit
pub async fn dispatch(
    command: EngineCommand,
    engine: &mut EngineState,
    app: &mut App,
    pack: &story_model::StoryPack,
    provider: &Arc<dyn LlmProvider>,
    config: &RuntimeConfig,
    save_store: &SaveStore,
    tts: &TtsService,
    scene_images: &mut SceneImageStores,
    tx: &UnboundedSender<HostMessage>,
    cancellation: &mut Option<(story_model::uuid::Uuid, CancellationToken)>,
) -> Result<(), String> {
    if matches!(command, EngineCommand::CancelGeneration { .. })
        && let Some((_, token)) = cancellation.as_ref()
    {
        token.cancel();
    }
    let transition = match reduce(engine, command, pack) {
        Ok(transition) => transition,
        Err(e) => {
            let code = e.reason_code().unwrap_or("engine_error").to_string();
            tracing::warn!(%code, error = %e, "engine command rejected; surfacing recoverable error");
            app.view.error = Some(DomainErrorView {
                code,
                title_ko: "명령을 처리할 수 없습니다".to_string(),
                message_ko: e.to_string(),
                recoverable: true,
            });
            return Ok(());
        }
    };
    *engine = transition.state;
    saves::refresh_app_lists(app, engine, pack);

    let mut deferred_error: Option<DomainErrorView> = None;

    for effect in transition.effects {
        match effect {
            EngineEffect::PublishView(view) => {
                // Narration TTS: only a genuinely NEW turn speaks. Choice
                // refreshes and error views republish the same turn number.
                let turn_completed = view.status.turn > app.view.status.turn;
                let narration = view.narration.clone();
                app.set_view(*view);
                if turn_completed && !narration.trim().is_empty() {
                    tts.speak(narration).await;
                }
                refresh_scene_images(app, pack, engine, scene_images);
            }
            EngineEffect::ShowRecoverableError(error) => app.view.error = Some(error),
            // No model selected yet (first run before auto-setup succeeded):
            // block the HTTP round-trip with a friendly message instead of a
            // confusing provider failure, and clear pending/loading so
            // PublishView cannot leave an unresolvable spinner behind.
            EngineEffect::RequestGeneration { .. }
            | EngineEffect::RequestDirector { .. }
            | EngineEffect::RequestWriter { .. }
                if provider.model_name().trim().is_empty() =>
            {
                if let Some(pending) = engine.pending.take() {
                    engine.game = pending.checkpoint;
                    engine.lineage.clear();
                }
                deferred_error = Some(DomainErrorView {
                    code: "model_not_selected".to_string(),
                    title_ko: "모델이 아직 선택되지 않았어요".to_string(),
                    message_ko: "설정 메뉴에서 모델을 고른 뒤 시작해 주세요.".to_string(),
                    recoverable: true,
                });
            }
            EngineEffect::RequestGeneration {
                generation_id,
                mut request,
            } => {
                if let Some((_, token)) = cancellation.take() {
                    token.cancel();
                }
                let token = CancellationToken::new();
                *cancellation = Some((generation_id, token.clone()));
                let tx = tx.clone();
                let provider = Arc::clone(provider);
                let is_choice_refresh = engine.pending.as_ref().is_some_and(|pending| {
                    pending.generation_id == generation_id && pending.choice_refresh
                });
                if is_choice_refresh && let Some(model) = &config.choice_model {
                    request.model = model.clone();
                }
                let model = if request.model.is_empty() || request.model == "mock" {
                    provider.model_name().to_string()
                } else {
                    request.model.clone()
                };
                capture_prompt_preview(app, &request.messages);
                tracing::info!(
                    %generation_id,
                    %model,
                    choice_refresh = is_choice_refresh,
                    max_tokens = request.max_tokens,
                    streaming = config.provider.stream_turns
                        && provider.capabilities().supports_streaming,
                    "invoking generation"
                );
                // MVP streaming path (issue #2): when enabled and the
                // provider advertises SSE support, forward decoded narration
                // chunks through the same WriterChunk → streaming_narration
                // render chain the Director/Writer path uses. The provider
                // itself falls back to a non-streaming request when the
                // stream fails before the first chunk, so this stays
                // best-effort — the completion message is identical either
                // way (ADR-003: streaming render state lives in host/TUI,
                // the engine is untouched).
                let use_stream =
                    config.provider.stream_turns && provider.capabilities().supports_streaming;
                tokio::spawn(async move {
                    let result = if use_stream {
                        let (events, events_rx) = tokio::sync::mpsc::channel(32);
                        let write = provider.generate_turn_stream(request, events, token);
                        pump_writer_events(events_rx, write, &tx, generation_id).await
                    } else {
                        provider.generate_turn(request, token).await
                    };
                    let _ = tx.send(HostMessage::Generation {
                        generation_id,
                        model,
                        result,
                    });
                });
            }
            EngineEffect::PersistStableState {
                slot_id,
                description,
            } => {
                if let Err(error) =
                    saves::save_engine_snapshot(save_store, engine, slot_id, description)
                {
                    tracing::error!(%error, "stable state autosave failed");
                    app.view.error = Some(saves::save_error_view("자동 저장 실패", error));
                }
            }
            EngineEffect::RequestDirector {
                generation_id,
                mut request,
            } => {
                if let Some((_, token)) = cancellation.take() {
                    token.cancel();
                }
                let token = CancellationToken::new();
                *cancellation = Some((generation_id, token.clone()));
                let tx = tx.clone();
                let provider = Arc::clone(provider);
                if let Some(ref m) = config.director_model {
                    request.model = m.clone();
                }
                let director_model = if !request.model.is_empty() {
                    request.model.clone()
                } else {
                    provider.model_name().to_string()
                };
                capture_prompt_preview(app, &request.messages);
                tracing::info!(%generation_id, model = %director_model, "invoking Director stage");
                tokio::spawn(async move {
                    let result = provider.direct(request, token).await;
                    let _ = tx.send(HostMessage::Director {
                        generation_id,
                        result,
                    });
                });
            }
            EngineEffect::RequestWriter {
                generation_id,
                mut request,
            } => {
                // A narration-quality repair receives a fresh generation id.
                // Replace the completed Director/Writer token so Cancel maps
                // to the repair request rather than the prior attempt.
                if let Some((_, token)) = cancellation.take() {
                    token.cancel();
                }
                let token = CancellationToken::new();
                *cancellation = Some((generation_id, token.clone()));
                let tx = tx.clone();
                let provider = Arc::clone(provider);
                if let Some(ref m) = config.writer_model {
                    request.model = m.clone();
                }
                let model = if !request.model.is_empty() {
                    request.model.clone()
                } else {
                    provider.model_name().to_string()
                };
                capture_prompt_preview(app, &request.messages);
                tracing::info!(%generation_id, model = %model, "invoking Writer stage");
                tokio::spawn(async move {
                    let (writer_events, writer_events_rx) = tokio::sync::mpsc::channel(32);
                    let write = provider.write_stream(request, writer_events, token);
                    let result =
                        pump_writer_events(writer_events_rx, write, &tx, generation_id).await;
                    let _ = tx.send(HostMessage::Writer {
                        generation_id,
                        model,
                        result,
                    });
                });
            }
        }
    }
    if let Some(error) = deferred_error {
        app.view.loading = None;
        app.view.error = Some(error);
    }
    Ok(())
}

fn provider_error_view(error: ProviderError) -> DomainErrorView {
    let player_message = match &error {
        ProviderError::InvalidGeneration(_) => "모델 응답 형식을 이해하지 못했습니다.".to_string(),
        ProviderError::Protocol(detail)
            if detail.starts_with("response decode failed:")
                || detail.starts_with("response body read failed:") =>
        {
            "모델 서버의 응답이 중간에 끊겼습니다. 잠시 후 다시 시도해 주세요.".to_string()
        }
        _ => error.to_string(),
    };
    let (code, title) = match error {
        ProviderError::Unavailable(_) => ("provider_unavailable", "모델 서버에 연결할 수 없습니다"),
        ProviderError::Loading(_) => ("provider_loading", "모델이 아직 로딩 중입니다"),
        ProviderError::Protocol(_) => ("provider_protocol", "모델 서버 응답 오류"),
        ProviderError::Timeout { .. } => ("provider_timeout", "모델 응답 시간 초과"),
        ProviderError::Cancelled => ("provider_cancelled", "생성을 취소했습니다"),
        ProviderError::UnsupportedCapability(_) => {
            ("provider_unsupported", "지원하지 않는 모델 기능")
        }
        // Serde/parser details are useful for diagnostics but are not stable
        // or actionable for players (for example: "key must be a string").
        // Keep the detail out of the TUI and offer a retry-oriented message.
        ProviderError::InvalidGeneration(_) => ("invalid_generation", "응답 형식 오류"),
    };
    DomainErrorView {
        code: code.to_string(),
        title_ko: title.to_string(),
        message_ko: player_message,
        recoverable: true,
    }
}

/// Classify the boundary that rejected a generation without exposing the
/// model's raw output to the player-facing error overlay. The provider emits
/// the same label with completion metadata (model, finish reason and size).
fn provider_failure_stage(error: &ProviderError) -> &'static str {
    match error {
        ProviderError::Protocol(detail) if detail.starts_with("response decode failed:") => {
            "http_envelope_decode"
        }
        ProviderError::InvalidGeneration(detail) if detail.starts_with("json_extract:") => {
            "assistant_json_extract"
        }
        ProviderError::InvalidGeneration(detail) if detail.starts_with("json_syntax:") => {
            "assistant_json_syntax"
        }
        ProviderError::InvalidGeneration(detail) if detail.starts_with("schema:") => {
            "assistant_schema"
        }
        ProviderError::InvalidGeneration(detail) if detail.starts_with("validation:") => {
            "assistant_validation"
        }
        ProviderError::InvalidGeneration(_) => "assistant_contract_unknown",
        ProviderError::Timeout { .. } => "request_timeout",
        ProviderError::Unavailable(_) | ProviderError::Loading(_) => "provider_unavailable",
        ProviderError::Cancelled => "cancelled",
        ProviderError::Protocol(_) => "http_protocol",
        ProviderError::UnsupportedCapability(_) => "unsupported_capability",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_image_picker, compile_display_rules, next_image_protocol_setting,
        provider_error_view, provider_failure_stage,
    };
    use story_model::ProviderError;
    use storytui::config::DisplayRule;

    #[test]
    fn display_rule_compilation_caps_skips_and_preserves_order() {
        let rules: Vec<DisplayRule> = (0..storytui::config::MAX_DISPLAY_RULES + 4)
            .map(|index| DisplayRule {
                pattern: format!("패턴{index}"),
                replacement: format!("치환{index}"),
                enabled: index != 1, // one disabled rule in the middle
            })
            .collect();
        let compiled = compile_display_rules(&rules);
        // The cap holds, disabled rules are out, order is preserved.
        assert_eq!(compiled.len(), storytui::config::MAX_DISPLAY_RULES);
        assert!(
            !compiled
                .iter()
                .any(|(_, replacement)| replacement == "치환1"),
            "disabled rule is skipped"
        );
        assert_eq!(compiled[0].1, "치환0");
        assert_eq!(compiled[1].1, "치환2");

        // An invalid pattern is skipped with the rest intact.
        let rules = vec![
            DisplayRule {
                pattern: "(unbalanced".to_string(),
                replacement: "x".to_string(),
                enabled: true,
            },
            DisplayRule {
                pattern: "숫자".to_string(),
                replacement: "N".to_string(),
                enabled: true,
            },
        ];
        let compiled = compile_display_rules(&rules);
        assert_eq!(compiled.len(), 1);
        assert!(compiled[0].0.is_match("한 숫자"));

        // An invalid pattern must not consume a capped slot: twice as many
        // rules as the cap with every other one invalid still fills the cap
        // with valid rules only.
        let rules: Vec<DisplayRule> = (0..storytui::config::MAX_DISPLAY_RULES * 4)
            .map(|index| DisplayRule {
                pattern: if index % 2 == 0 {
                    "(bad".to_string()
                } else {
                    format!("좋은{index}")
                },
                replacement: format!("r{index}"),
                enabled: true,
            })
            .collect();
        let compiled = compile_display_rules(&rules);
        assert_eq!(compiled.len(), storytui::config::MAX_DISPLAY_RULES);
        assert!(
            compiled
                .iter()
                .all(|(pattern, _)| pattern.as_str().starts_with("좋은")),
            "only valid rules fill the cap"
        );
    }

    #[test]
    fn image_protocol_cycle_skips_unavailable_protocols() {
        // With a detected picker: auto → kitty → sixel → iterm2 → halfblocks → auto.
        assert_eq!(
            next_image_protocol_setting(None, true).as_deref(),
            Some("kitty")
        );
        assert_eq!(
            next_image_protocol_setting(Some("kitty"), true).as_deref(),
            Some("sixel")
        );
        assert_eq!(
            next_image_protocol_setting(Some("iterm2"), true).as_deref(),
            Some("halfblocks")
        );
        assert_eq!(next_image_protocol_setting(Some("halfblocks"), true), None);
        // Without a detected picker only halfblocks is reachable.
        assert_eq!(
            next_image_protocol_setting(None, false).as_deref(),
            Some("halfblocks")
        );
        assert_eq!(next_image_protocol_setting(Some("halfblocks"), false), None);
        // An unrecognized stored value is treated as auto.
        assert_eq!(
            next_image_protocol_setting(Some("kity"), true).as_deref(),
            Some("kitty")
        );
    }

    #[test]
    fn build_image_picker_forces_only_recognized_values() {
        let base = ratatui_image::picker::Picker::halfblocks();
        // Auto keeps the detected picker and is never manual.
        let (picker, manual) = build_image_picker(Some(&base), None);
        assert_eq!(
            picker.as_ref().map(|p| p.protocol_type()),
            Some(ratatui_image::picker::ProtocolType::Halfblocks)
        );
        assert!(!manual);
        // A forced protocol rides on the base picker's cell size.
        let (picker, manual) = build_image_picker(Some(&base), Some("sixel"));
        assert_eq!(
            picker.as_ref().map(|p| p.protocol_type()),
            Some(ratatui_image::picker::ProtocolType::Sixel)
        );
        assert!(manual);
        // Halfblocks can be forced even without a detected picker.
        let (picker, manual) = build_image_picker(None, Some("halfblocks"));
        assert_eq!(
            picker.as_ref().map(|p| p.protocol_type()),
            Some(ratatui_image::picker::ProtocolType::Halfblocks)
        );
        assert!(manual);
        // kitty/sixel/iterm2 without a base fall back to auto.
        let (picker, manual) = build_image_picker(None, Some("kitty"));
        assert!(picker.is_none());
        assert!(!manual);
        // Unrecognized values behave as auto.
        let (picker, manual) = build_image_picker(Some(&base), Some("kity"));
        assert!(picker.is_some());
        assert!(!manual);
    }

    #[test]
    fn invalid_generation_hides_parser_internals_from_player() {
        let view = provider_error_view(ProviderError::InvalidGeneration(
            "key must be a string at line 1 column 2".to_string(),
        ));
        assert_eq!(view.code, "invalid_generation");
        assert!(!view.message_ko.contains("key must be a string"));
        assert_eq!(view.title_ko, "응답 형식 오류");
        assert_eq!(view.message_ko, "모델 응답 형식을 이해하지 못했습니다.");
    }

    #[test]
    fn incomplete_http_body_hides_decoder_detail_from_player() {
        let view = provider_error_view(ProviderError::Protocol(
            "response decode failed: expected value at line 1 column 1".to_string(),
        ));
        assert_eq!(view.code, "provider_protocol");
        assert_eq!(view.title_ko, "모델 서버 응답 오류");
        assert!(!view.message_ko.contains("response decode failed"));
        assert_eq!(
            view.message_ko,
            "모델 서버의 응답이 중간에 끊겼습니다. 잠시 후 다시 시도해 주세요."
        );
    }

    #[test]
    fn failure_stage_keeps_json_schema_and_envelope_failures_separate() {
        assert_eq!(
            provider_failure_stage(&ProviderError::Protocol(
                "response decode failed: expected value".to_string()
            )),
            "http_envelope_decode"
        );
        assert_eq!(
            provider_failure_stage(&ProviderError::InvalidGeneration(
                "json_syntax: EOF while parsing an object".to_string()
            )),
            "assistant_json_syntax"
        );
        assert_eq!(
            provider_failure_stage(&ProviderError::InvalidGeneration(
                "schema: UUID parsing failed".to_string()
            )),
            "assistant_schema"
        );
    }
    #[test]
    fn enabling_expressions_mid_session_resolves_sprites_immediately() {
        use super::{SceneImageStores, images, refresh_expression_images};
        use story_core::engine::{EngineState, build_view};
        use story_tui::App;

        let dir = tempfile::tempdir().expect("tempdir");
        let pack = images::tests::pack_at(dir.path());
        let sprite = dir.path().join("images/expressions/yuna/neutral.png");
        images::tests::write_png(&sprite);

        let engine = EngineState::initial(&pack);
        let mut app = App::new(build_view(&engine, &pack, None, None));
        let mut stores = SceneImageStores::default();

        // Off: the refresh is skipped entirely — nothing resolves.
        app.set_expressions_enabled(false);
        refresh_expression_images(&mut app, &pack, &engine, &mut stores);
        assert_eq!(app.expression_sprite_count(), 0);

        // A session that STARTED off must not wait for the next publish:
        // turning ON resolves the beats in play right away (F-1).
        app.set_expressions_enabled(true);
        refresh_expression_images(&mut app, &pack, &engine, &mut stores);
        assert_eq!(
            app.expression_sprite_count(),
            1,
            "cold toggle-on resolves the opening emotion immediately"
        );
    }
}
