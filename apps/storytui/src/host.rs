use std::io::{self, Stdout};
use std::sync::Arc;

use crossterm::event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyEventKind};
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

use storytui::config::{RuntimeConfig, UserSettings, load_user_settings};

mod diagnostics;
mod saves;
mod settings;

type UiTerminal = Terminal<CrosstermBackend<Stdout>>;

enum HostMessage {
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
    let mut provider = Arc::new(
        OpenAiCompatibleProvider::new(provider_config.clone())
            .map_err(|e| format!("provider를 만들지 못했습니다: {e}"))?,
    );
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
    let mut resume_engine = saves::restore_autosave(&save_store, &pack);
    let mut engine = EngineState::initial(&pack);
    engine.use_director_writer = config.use_director_writer;
    if let Some(restored) = resume_engine.as_mut() {
        restored.use_director_writer = config.use_director_writer;
    }
    let mut app = App::new(build_view(&engine, &pack, None, None));
    app.set_lists(&engine.game.history, &engine.game.memories);
    app.set_can_continue(resume_engine.is_some());
    settings::refresh_view(&mut app, &provider_config, health, config.director_model.clone(), config.writer_model.clone());
    app.show_home();
    let mut user_settings = load_user_settings(&config.save_dir)?;
    let (tx, mut rx) = unbounded_channel();
    let mut cancellation: Option<(story_model::uuid::Uuid, CancellationToken)> = None;

    let mut event_stream = EventStream::new();
    let mut frame_tick = tokio::time::interval(std::time::Duration::from_millis(16));
    frame_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut needs_redraw = true;

    loop {
        tokio::select! {
            Some(event_res) = event_stream.next() => {
                let event = event_res.map_err(|e| format!("입력 이벤트를 읽지 못했습니다: {e}"))?;
                match event {
                    Event::Key(key) if key.kind != KeyEventKind::Release => {
                        let pending = app.view.loading.as_ref().map(|v| v.generation_id);
                        if let Some(command) = app.handle_key(key, pending) {
                            dispatch(command, &mut engine, &mut app, &pack, &provider, &config, &save_store, &tx, &mut cancellation).await?;
                        }
                        handle_ui_requests(
                            &mut app, &mut engine, &pack, &mut provider_config,
                            &mut provider, &mut user_settings, &mut config,
                            &save_store, &tx, &mut cancellation, &mut resume_engine,
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
                                &tx,
                                &mut cancellation,
                            )
                            .await?;
                        }
                        handle_ui_requests(
                            &mut app, &mut engine, &pack, &mut provider_config,
                            &mut provider, &mut user_settings, &mut config,
                            &save_store, &tx, &mut cancellation, &mut resume_engine,
                        ).await?;
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
                        dispatch(command, &mut engine, &mut app, &pack, &provider, &config, &save_store, &tx, &mut cancellation).await?;
                        needs_redraw = true;
                    }
                    HostMessage::Director { generation_id, result } => {
                        let command = match result {
                            Ok(plan) => EngineCommand::AcceptDirectorPlan { generation_id, plan: Box::new(plan) },
                            Err(error) => {
                                diagnostics::record_provider_failure(&config.save_dir, generation_id, &error);
                                EngineCommand::ProviderFailed { generation_id, error: provider_error_view(error) }
                            }
                        };
                        dispatch(command, &mut engine, &mut app, &pack, &provider, &config, &save_store, &tx, &mut cancellation).await?;
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
                        dispatch(command, &mut engine, &mut app, &pack, &provider, &config, &save_store, &tx, &mut cancellation).await?;
                        needs_redraw = true;
                    }
                    HostMessage::WriterChunk { generation_id, text } => {
                        app.push_writer_chunk(generation_id, &text);
                        needs_redraw = true;
                    }
                }
            }
            _ = frame_tick.tick(), if needs_redraw || app.view.loading.is_some() => {
                terminal
                    .draw(|frame| app.render(frame, frame.area()))
                    .map_err(|e| format!("화면을 그리지 못했습니다: {e}"))?;
                needs_redraw = false;
            }
        }
    }

    if let Some((_, token)) = cancellation.take() {
        token.cancel();
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)] // consumes all host-owned UI requests in one place
async fn handle_ui_requests(
    app: &mut App,
    engine: &mut EngineState,
    pack: &StoryPack,
    provider_config: &mut story_provider::ProviderConfig,
    provider: &mut Arc<OpenAiCompatibleProvider>,
    user_settings: &mut UserSettings,
    config: &mut RuntimeConfig,
    save_store: &SaveStore,
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
                tx,
                cancellation,
            )
            .await?;
        }
        settings::refresh_model_list(app, provider, provider_config, false).await;
    }
    if app.take_model_reload_requested() {
        settings::refresh_model_list(app, provider, provider_config, true).await;
    }
    if let Some(model) = app.take_selected_model() {
        settings::apply_model(
            app,
            model,
            provider_config,
            provider,
            user_settings,
            &config.save_dir,
        )
        .await?;
    }
    if let Some(model) = app.take_selected_director_model() {
        settings::apply_director_model(
            app,
            model,
            config,
            provider_config,
            user_settings,
        )?;
    }
    if let Some(model) = app.take_selected_writer_model() {
        settings::apply_writer_model(
            app,
            model,
            config,
            provider_config,
            user_settings,
        )?;
    }
    if let Some(request) = app.take_home_request() {
        match request {
            HomeRequest::StartNew => {
                let mut new_engine = EngineState::initial(pack);
                new_engine.use_director_writer = config.use_director_writer;
                *engine = new_engine;
                app.set_view(build_view(engine, pack, None, None));
                app.set_lists(&engine.game.history, &engine.game.memories);
                app.follow_story_end();
                app.close_home();
                dispatch(
                    EngineCommand::StartNewGame,
                    engine,
                    app,
                    pack,
                    provider,
                    config,
                    save_store,
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
                }
            }
            HomeRequest::ReturnToGame => app.close_home(),
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

#[allow(clippy::too_many_arguments)] // host adapter keeps all effect-owned handles safe
async fn dispatch(
    command: EngineCommand,
    engine: &mut EngineState,
    app: &mut App,
    pack: &story_model::StoryPack,
    provider: &Arc<OpenAiCompatibleProvider>,
    config: &RuntimeConfig,
    save_store: &SaveStore,
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
            EngineEffect::PublishView(view) => app.set_view(*view),
            EngineEffect::ShowRecoverableError(error) => app.view.error = Some(error),
            EngineEffect::RequestGeneration {
                generation_id,
                request,
            } => {
                if let Some((_, token)) = cancellation.take() {
                    token.cancel();
                }
                let token = CancellationToken::new();
                *cancellation = Some((generation_id, token.clone()));
                let tx = tx.clone();
                let provider = Arc::clone(provider);
                let model = provider.model_name().to_string();
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
                let token = cancellation
                    .as_ref()
                    .map(|(_, token)| token.clone())
                    .unwrap_or_default();
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
