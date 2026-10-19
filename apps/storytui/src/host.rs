use std::io::{self, Stdout};
use std::sync::Arc;

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
use story_core::engine::{EngineState, build_view, reduce};
use story_core::engine_cmd::{EngineCommand, EngineEffect};
use story_model::{DomainErrorView, ProviderError, ProviderHealth, StoryPack};
use story_persistence::{PackLoader, SaveStore};
use story_provider::{LlmProvider, OpenAiCompatibleProvider};
use story_tui::{App, HomeRequest};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio_util::sync::CancellationToken;

use story_tts::TtsService;
use storytui::config::{RuntimeConfig, UserSettings, load_user_settings, save_user_settings};

mod diagnostics;
mod images;
mod packs;
pub(crate) mod saves;
mod settings;

type UiTerminal = Terminal<CrosstermBackend<Stdout>>;

/// Summary of a finished card import (dialog report content).
struct ImportOutcome {
    pack_dir: String,
    lines: Vec<String>,
}

enum HostMessage {
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
    apply_player_profile(&mut pack, &config.player);
    let mut provider_config = config.provider.clone();
    let mut repaired_main_model = false;
    let mut repaired_stage_models = false;
    // First-run auto-setup: with no model selected yet (a fresh clone — the
    // checked-in template intentionally carries no model id), find a local
    // OpenAI-compatible server and adopt it for this session. The model is
    // persisted below through the repaired-model path; the URL never is.
    let mut discovery: Option<settings::DiscoveryOutcome> = None;
    if provider_config.model.trim().is_empty() {
        match settings::discover_session_provider(&provider_config).await {
            settings::DiscoveryOutcome::Connected { base_url, model } => {
                tracing::info!(
                    base_url = %base_url,
                    model = %model,
                    "no model configured; adopting a detected local server"
                );
                provider_config.base_url = base_url;
                provider_config.model = model;
                config.provider = provider_config.clone();
                repaired_main_model = true;
            }
            other => discovery = Some(other),
        }
    }
    let mut provider = Arc::new(
        OpenAiCompatibleProvider::new(provider_config.clone())
            .map_err(|e| format!("provider를 만들지 못했습니다: {e}"))?,
    );
    match provider.list_models().await {
        Ok(advertised) if !advertised.is_empty() => {
            if let Some(model) = settings::reconcile_active_model(
                &provider_config.model,
                &config.fallback_model,
                &advertised,
            ) && model != provider_config.model
            {
                tracing::warn!(
                    stale_model = %provider_config.model,
                    fallback_model = %model,
                    "configured model is not advertised; using an available local model"
                );
                provider_config.model = model;
                repaired_main_model = true;
                config.provider = provider_config.clone();
                provider = Arc::new(
                    OpenAiCompatibleProvider::new(provider_config.clone())
                        .map_err(|e| format!("대체 provider를 만들지 못했습니다: {e}"))?,
                );
            }
            for stage_model in [
                &mut config.director_model,
                &mut config.choice_model,
                &mut config.writer_model,
            ] {
                if let Some(current) = stage_model.clone()
                    && !advertised.iter().any(|model| model == &current)
                {
                    tracing::warn!(
                        stale_model = %current,
                        fallback_model = %provider_config.model,
                        "stage model is not advertised; using the active model"
                    );
                    *stage_model = Some(provider_config.model.clone());
                    repaired_stage_models = true;
                }
            }
        }
        Ok(_) => tracing::warn!("provider advertised an empty model list"),
        Err(error) => tracing::warn!(%error, "provider model preflight failed"),
    }
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
    let image_picker = match ratatui_image::picker::Picker::from_query_stdio() {
        Ok(picker) => Some(picker),
        Err(error) => {
            tracing::info!(
                %error,
                "terminal graphics query failed; scene images stay disabled"
            );
            None
        }
    };
    let mut resume_engine = saves::restore_autosave(&save_store, &pack);
    let mut engine = EngineState::initial(&pack);
    engine.use_director_writer = config.use_director_writer;
    if let Some(restored) = resume_engine.as_mut() {
        restored.use_director_writer = config.use_director_writer;
    }
    let mut scene_image_cache = images::SceneImageCache::default();
    let mut app = App::new(build_view(&engine, &pack, None, None));
    app.set_image_picker(image_picker);
    refresh_scene_images(&mut app, &pack, &engine, &mut scene_image_cache);
    app.set_lists(&engine.game.history, &engine.game.memories);
    app.set_can_continue(resume_engine.is_some());
    app.set_greetings(greeting_labels(&pack));
    app.set_current_pack_dir(&config.pack_dir.to_string_lossy());
    {
        // The roster is exactly the load()-assembled set: base profile first,
        // then every `[[providers]]` entry — never prepended again, or the
        // active profile would render twice.
        let roster: Vec<(String, String)> = config
            .providers
            .iter()
            .map(|entry| (entry.name.clone(), entry.base_url.clone()))
            .collect();
        app.set_providers(&roster, &provider_config.name);
    }
    let mut user_settings = load_user_settings(&config.save_dir)?;
    // The `v` toggle persists an explicit preference; absent means the [tts]
    // profile's `enabled` default decides.
    let tts_enabled = user_settings.tts_enabled.unwrap_or(config.tts.enabled);
    config.tts.enabled = tts_enabled;
    let images_enabled = user_settings.images_enabled.unwrap_or(true);
    app.set_images_enabled(images_enabled);
    let tts = Arc::new(TtsService::new(config.tts.clone()));
    {
        let status = tts.status().await;
        let images_enabled = app.images_enabled();
        let image_backend = app.image_backend_label();
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
        );
    }
    // Start-menu status line: guide an unconfigured first run, and note a
    // down server for a returning user — without blocking the menu.
    app.set_home_status(if provider_config.model.trim().is_empty() {
        Some(settings::unconfigured_home_status(
            discovery
                .as_ref()
                .unwrap_or(&settings::DiscoveryOutcome::NoServer),
        ))
    } else if health == ProviderHealth::Unavailable {
        Some(settings::server_down_home_status())
    } else {
        None
    });
    app.show_home();
    if repaired_main_model {
        user_settings.model = Some(provider_config.model.clone());
    }
    if repaired_stage_models {
        user_settings.director_model = config.director_model.clone();
        user_settings.choice_model = config.choice_model.clone();
        user_settings.writer_model = config.writer_model.clone();
    }
    if repaired_main_model || repaired_stage_models {
        save_user_settings(&config.save_dir, &user_settings)?;
    }
    let (tx, mut rx) = unbounded_channel();
    let mut cancellation: Option<(story_model::uuid::Uuid, CancellationToken)> = None;

    let mut event_stream = EventStream::new();
    let mut frame_tick = tokio::time::interval(std::time::Duration::from_millis(16));
    frame_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
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
                            dispatch(command, &mut engine, &mut app, &pack, &provider, &config, &save_store, &tts, &mut scene_image_cache, &tx, &mut cancellation).await?;
                        }
                        handle_ui_requests(
                            &mut app, &mut engine, &mut pack, &mut provider_config,
                            &mut provider, &mut user_settings, &mut config,
                            &save_store, &tts, &mut scene_image_cache, &tx, &mut cancellation, &mut resume_engine,
                        ).await?;
                        needs_redraw = true;
                    }
                    Event::Mouse(mouse) => {
                        app.handle_mouse(mouse);
                        if let Some(command) = app.take_mouse_command() {
                            dispatch(
                                command,
                                &mut engine,
                                &mut app,
                                &pack,
                                &provider,
                                &config,
                                &save_store,
                                &tts,
                                &mut scene_image_cache,
                                &tx,
                                &mut cancellation,
                            )
                            .await?;
                        }
                        handle_ui_requests(
                            &mut app, &mut engine, &mut pack, &mut provider_config,
                            &mut provider, &mut user_settings, &mut config,
                            &save_store, &tts, &mut scene_image_cache, &tx, &mut cancellation, &mut resume_engine,
                        ).await?;
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
                match message {
                    HostMessage::Generation { generation_id, model, result } => {
                        let command = match result {
                            Ok(draft) => {
                                app.record_generation_model(generation_id, model);
                                app.record_generation_duration(generation_id);
                                EngineCommand::AcceptGeneration { draft: Box::new(draft) }
                            }
                            Err(error) => {
                                diagnostics::record_provider_failure(&config.save_dir, generation_id, &error);
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
                        dispatch(command, &mut engine, &mut app, &pack, &provider, &config, &save_store, &tts, &mut scene_image_cache, &tx, &mut cancellation).await?;
                        needs_redraw = true;
                    }
                    HostMessage::Director { generation_id, result } => {
                        let command = match result {
                            Ok(plan) => EngineCommand::AcceptDirectorPlan { generation_id, plan: Box::new(plan) },
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
                                    EngineCommand::ProviderFailed { generation_id, error: provider_error_view(error) }
                                }
                            }
                        };
                        dispatch(command, &mut engine, &mut app, &pack, &provider, &config, &save_store, &tts, &mut scene_image_cache, &tx, &mut cancellation).await?;
                        needs_redraw = true;
                    }
                    HostMessage::Writer { generation_id, model, result } => {
                        let command = match result {
                            Ok(finish) => {
                                app.record_generation_model(generation_id, model);
                                app.record_generation_duration(generation_id);
                                EngineCommand::AcceptWriterFinish { finish: Box::new(finish) }
                            }
                            Err(error) => {
                                diagnostics::record_provider_failure(&config.save_dir, generation_id, &error);
                                EngineCommand::ProviderFailed { generation_id, error: provider_error_view(error) }
                            }
                        };
                        dispatch(command, &mut engine, &mut app, &pack, &provider, &config, &save_store, &tts, &mut scene_image_cache, &tx, &mut cancellation).await?;
                        needs_redraw = true;
                    }
                    HostMessage::ImportFinished { path, result } => {
                        match result {
                            Ok(outcome) => {
                                app.show_import_report(&path, outcome.lines, Some(outcome.pack_dir))
                            }
                            Err(error) => {
                                app.show_import_report(&path, vec![format!("임포트 실패: {error}")], None)
                            }
                        }
                        needs_redraw = true;
                    }
                    HostMessage::WriterChunk { generation_id, text } => {
                        app.push_writer_chunk(generation_id, &text);
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
        }
    }

    if let Some((_, token)) = cancellation.take() {
        token.cancel();
    }
    Ok(())
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
    scene_images: &mut images::SceneImageCache,
    config: &RuntimeConfig,
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
    apply_player_profile(pack, &config.player);
    *engine = EngineState::initial(pack);
    engine.use_director_writer = config.use_director_writer;
    *resume_engine = None;
    *scene_images = images::SceneImageCache::default();
    app.set_view(build_view(engine, pack, None, None));
    app.set_lists(&engine.game.history, &engine.game.memories);
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

/// Refresh the scene image cache and push its update into the view: set a new
/// image, clear it, or leave the current pane untouched.
fn refresh_scene_images(
    app: &mut App,
    pack: &StoryPack,
    engine: &EngineState,
    cache: &mut images::SceneImageCache,
) {
    match cache.refresh(pack, engine) {
        images::SceneImageUpdate::Set(image) => app.set_scene_image(Some(image)),
        images::SceneImageUpdate::Clear => app.set_scene_image(None),
        images::SceneImageUpdate::Unchanged => {}
    }
}

#[allow(clippy::too_many_arguments)] // consumes all host-owned UI requests in one place
async fn handle_ui_requests(
    app: &mut App,
    engine: &mut EngineState,
    pack: &mut StoryPack,
    provider_config: &mut story_provider::ProviderConfig,
    provider: &mut Arc<OpenAiCompatibleProvider>,
    user_settings: &mut UserSettings,
    config: &mut RuntimeConfig,
    save_store: &SaveStore,
    tts: &TtsService,
    scene_images: &mut images::SceneImageCache,
    tx: &UnboundedSender<HostMessage>,
    cancellation: &mut Option<(story_model::uuid::Uuid, CancellationToken)>,
    resume_engine: &mut Option<EngineState>,
) -> Result<(), String> {
    if app.take_settings_opened() {
        if let Some((generation_id, token)) = cancellation.take() {
            token.cancel();
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
        // no model is selected, opening settings retries auto-setup first.
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
            Some(settings::DiscoveryOutcome::Connected { .. }) => {}
            Some(other) => {
                app.set_home_status(Some(settings::unconfigured_home_status(other)));
            }
            None => settings::refresh_model_list(app, provider, provider_config, false).await,
        }
    }
    if app.take_model_reload_requested() {
        settings::refresh_model_list(app, provider, provider_config, true).await;
    }
    if let Some(name) = app.take_selected_provider() {
        // Clone the roster: the call also needs `config` mutably for the
        // stale-model repair.
        let roster = config.providers.clone();
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
            provider,
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
            provider,
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
            provider,
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
                app.set_lists(&engine.game.history, &engine.game.memories);
                app.follow_story_end();
                app.close_home();
                refresh_scene_images(app, pack, engine, scene_images);
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
                if let Some(mut restored) = resume_engine.take() {
                    restored.use_director_writer = config.use_director_writer;
                    *engine = restored;
                    app.set_view(build_view(engine, pack, None, None));
                    app.set_lists(&engine.game.history, &engine.game.memories);
                    app.follow_story_end();
                    app.set_can_continue(false);
                    refresh_scene_images(app, pack, engine, scene_images);
                }
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
        swap_pack(
            app,
            &pack_dir,
            pack,
            engine,
            scene_images,
            config,
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

fn apply_player_profile(pack: &mut StoryPack, profile: &storytui::config::PlayerProfile) {
    let name = profile
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty());
    let persona = profile
        .persona
        .as_deref()
        .map(str::trim)
        .filter(|persona| !persona.is_empty());
    if name.is_none() && persona.is_none() {
        return;
    }
    pack.prompts.system.push_str("\n\n# Player profile\n");
    if let Some(name) = name {
        pack.prompts.system.push_str(&format!(
            "플레이어의 이름은 `{name}`이다. 다른 이름이나 '이름 씨' 같은 임시 호칭을 만들지 말고, 필요한 호칭에는 `{name} 씨`를 사용한다.\n"
        ));
    }
    if let Some(persona) = persona {
        pack.prompts.system.push_str(&format!(
            "플레이어 페르소나: {persona}\n플레이어가 직접 입력하지 않은 행동·감정은 이 페르소나만으로 확정하지 않는다.\n"
        ));
    }
}

#[allow(clippy::too_many_arguments)] // host adapter keeps all effect-owned handles explicit
async fn dispatch(
    command: EngineCommand,
    engine: &mut EngineState,
    app: &mut App,
    pack: &story_model::StoryPack,
    provider: &Arc<OpenAiCompatibleProvider>,
    config: &RuntimeConfig,
    save_store: &SaveStore,
    tts: &TtsService,
    scene_images: &mut images::SceneImageCache,
    tx: &UnboundedSender<HostMessage>,
    cancellation: &mut Option<(story_model::uuid::Uuid, CancellationToken)>,
) -> Result<(), String> {
    if matches!(command, EngineCommand::CancelGeneration { .. })
        && let Some((_, token)) = cancellation.as_ref()
    {
        token.cancel();
    }
    let transition = reduce(engine, command, pack).map_err(|e| {
        let view = DomainErrorView {
            code: e.reason_code().unwrap_or("engine_error").to_string(),
            title_ko: "명령을 처리할 수 없습니다".to_string(),
            message_ko: e.to_string(),
            recoverable: true,
        };
        app.view.error = Some(view);
        e.to_string()
    })?;
    *engine = transition.state;
    app.set_lists(&engine.game.history, &engine.game.memories);

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
            // confusing provider failure.
            EngineEffect::RequestGeneration { .. }
            | EngineEffect::RequestDirector { .. }
            | EngineEffect::RequestWriter { .. }
                if provider.model_name().trim().is_empty() =>
            {
                app.view.error = Some(DomainErrorView {
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
                tracing::info!(
                    %generation_id,
                    %model,
                    choice_refresh = is_choice_refresh,
                    max_tokens = request.max_tokens,
                    "invoking generation"
                );
                tokio::spawn(async move {
                    let result = provider.generate_turn(request, token).await;
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
                tracing::info!(%generation_id, model = %model, "invoking Writer stage");
                tokio::spawn(async move {
                    let (writer_events, mut writer_events_rx) = tokio::sync::mpsc::channel(32);
                    let write = provider.write_stream(request, writer_events, token);
                    tokio::pin!(write);
                    let result = loop {
                        tokio::select! {
                            event = writer_events_rx.recv() => match event {
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
                    };
                    let _ = tx.send(HostMessage::Writer {
                        generation_id,
                        model,
                        result,
                    });
                });
            }
        }
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
    use super::{provider_error_view, provider_failure_stage};
    use story_model::ProviderError;

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
}
