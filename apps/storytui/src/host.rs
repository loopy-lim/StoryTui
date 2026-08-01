use std::io::{self, Stdout};
use std::sync::Arc;

use crossterm::event::{Event, EventStream, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures_util::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use story_core::engine::{EngineState, build_view, reduce};
use story_core::engine_cmd::{EngineCommand, EngineEffect};
use story_model::{
    DomainErrorView, ProviderError, ProviderHealth, SAVE_SCHEMA_VERSION, SaveEnvelope,
};
use story_persistence::{CHECKSUM_ALGORITHM, PackLoader, SaveStore};
use story_provider::{LlmProvider, OpenAiCompatibleProvider};
use story_tui::App;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio_util::sync::CancellationToken;

use storytui::config::RuntimeConfig;

type UiTerminal = Terminal<CrosstermBackend<Stdout>>;

enum HostMessage {
    Generation {
        generation_id: story_model::uuid::Uuid,
        result: Result<story_model::GeneratedTurnDraft, ProviderError>,
    },
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<(Self, UiTerminal)> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, crossterm::cursor::Hide)?;
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
        let _ = execute!(stdout, LeaveAlternateScreen, crossterm::cursor::Show);
    }
}

pub async fn run(config: RuntimeConfig) -> Result<(), String> {
    let pack = PackLoader::load(&config.pack_dir)
        .map_err(|e| format!("Story Pack을 불러오지 못했습니다: {e}"))?;
    let provider = Arc::new(
        OpenAiCompatibleProvider::new(config.provider.clone())
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
        model = %config.provider.model,
        base_url = %config.provider.base_url,
        ?health,
        "provider health probe"
    );

    let (_guard, mut terminal) =
        TerminalGuard::enter().map_err(|e| format!("터미널을 초기화하지 못했습니다: {e}"))?;
    let mut engine = EngineState::initial(&pack);
    let mut app = App::new(build_view(&engine, &pack, None, None));
    let (tx, mut rx) = unbounded_channel();
    let mut cancellation: Option<(story_model::uuid::Uuid, CancellationToken)> = None;

    dispatch(
        EngineCommand::StartNewGame,
        &mut engine,
        &mut app,
        &pack,
        &provider,
        &save_store,
        &tx,
        &mut cancellation,
    )
    .await?;

    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(100));

    loop {
        terminal
            .draw(|frame| app.render(frame, frame.area()))
            .map_err(|e| format!("화면을 그리지 못했습니다: {e}"))?;

        tokio::select! {
            maybe_event = events.next() => {
                let Some(Ok(event)) = maybe_event else { break };
                if let Event::Key(key) = event
                    && key.kind != KeyEventKind::Release
                {
                    let pending = app.view.loading.as_ref().map(|v| v.generation_id);
                    if let Some(command) = app.handle_key(key, pending) {
                        dispatch(command, &mut engine, &mut app, &pack, &provider, &save_store, &tx, &mut cancellation).await?;
                    }
                }
                if app.should_quit { break; }
            }
            Some(message) = rx.recv() => {
                match message {
                    HostMessage::Generation { generation_id, result } => {
                        let command = match result {
                            Ok(draft) => EngineCommand::AcceptGeneration { draft: Box::new(draft) },
                            Err(error) => EngineCommand::ProviderFailed {
                                generation_id,
                                error: provider_error_view(error),
                            },
                        };
                        dispatch(command, &mut engine, &mut app, &pack, &provider, &save_store, &tx, &mut cancellation).await?;
                    }
                }
            }
            _ = tick.tick() => {}
        }
    }

    if let Some((_, token)) = cancellation.take() {
        token.cancel();
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)] // host adapter keeps all effect-owned handles safe
async fn dispatch(
    command: EngineCommand,
    engine: &mut EngineState,
    app: &mut App,
    pack: &story_model::StoryPack,
    provider: &Arc<OpenAiCompatibleProvider>,
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
                tokio::spawn(async move {
                    let result = provider.generate_turn(request, token).await;
                    let _ = tx.send(HostMessage::Generation {
                        generation_id,
                        result,
                    });
                });
            }
            EngineEffect::PersistStableState {
                slot_id,
                description,
            } => {
                let game = &engine.game;
                let payload = engine.snapshot_payload();
                let envelope = SaveEnvelope {
                    save_schema_version: SAVE_SCHEMA_VERSION,
                    engine_version: env!("CARGO_PKG_VERSION").to_string(),
                    story_id: game.story_id.clone(),
                    story_version: game.story_version.clone(),
                    pack_schema_version: game.pack_schema_version.clone(),
                    slot_id,
                    description,
                    saved_at: story_model::chrono::Utc::now(),
                    chapter_label: game.current_chapter.clone(),
                    scene_label: game.current_scene_id.clone(),
                    checksum_algorithm: CHECKSUM_ALGORITHM.to_string(),
                    payload_checksum: String::new(),
                    payload,
                };
                if let Err(error) = save_store.save(envelope) {
                    tracing::error!(%error, "stable state autosave failed");
                    app.view.error = Some(DomainErrorView {
                        code: "save_failed".to_string(),
                        title_ko: "자동 저장 실패".to_string(),
                        message_ko: format!("세이브를 저장하지 못했습니다: {error}"),
                        recoverable: true,
                    });
                }
            }
            EngineEffect::RequestDirector { .. } | EngineEffect::RequestWriter { .. } => {
                app.view.error = Some(DomainErrorView {
                    code: "unsupported_runtime_path".to_string(),
                    title_ko: "지원하지 않는 실행 경로".to_string(),
                    message_ko: "현재 호스트는 MVP 단일 호출 경로만 실행합니다.".to_string(),
                    recoverable: true,
                });
            }
        }
    }
    Ok(())
}

fn provider_error_view(error: ProviderError) -> DomainErrorView {
    let player_message = if matches!(&error, ProviderError::InvalidGeneration(_)) {
        "모델 응답을 이해하지 못했습니다. 다시 시도해 주세요".to_string()
    } else {
        error.to_string()
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
        ProviderError::InvalidGeneration(_) => (
            "invalid_generation",
            "모델 응답을 이해하지 못했습니다. 다시 시도해 주세요",
        ),
    };
    DomainErrorView {
        code: code.to_string(),
        title_ko: title.to_string(),
        message_ko: player_message,
        recoverable: true,
    }
}

#[cfg(test)]
mod tests {
    use super::provider_error_view;
    use story_model::ProviderError;

    #[test]
    fn invalid_generation_hides_parser_internals_from_player() {
        let view = provider_error_view(ProviderError::InvalidGeneration(
            "key must be a string at line 1 column 2".to_string(),
        ));
        assert_eq!(view.code, "invalid_generation");
        assert!(!view.message_ko.contains("key must be a string"));
        assert_eq!(
            view.message_ko,
            "모델 응답을 이해하지 못했습니다. 다시 시도해 주세요"
        );
    }
}
