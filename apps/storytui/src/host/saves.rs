//! Save/resume and manual-slot operations for the interactive host.

use story_core::engine::{AUTOSAVE_SLOT, EngineState, build_view};
use story_model::{DomainErrorView, SAVE_SCHEMA_VERSION, SaveEnvelope, StoryPack};
use story_persistence::{CHECKSUM_ALGORITHM, SaveError, SaveStore, SlotMeta};
use story_tui::{App, SaveLoadMode, SaveLoadRequest};
use tokio_util::sync::CancellationToken;

/// Resume only the verified autosave belonging to the loaded Pack.
pub fn restore_autosave(save_store: &SaveStore, pack: &StoryPack) -> Option<EngineState> {
    let envelope = match save_store.load(AUTOSAVE_SLOT) {
        Ok(envelope) => envelope,
        Err(SaveError::MissingSlot(_)) => return None,
        Err(error) => {
            tracing::warn!(%error, "autosave was not resumed");
            return None;
        }
    };
    match engine_from_envelope(pack, envelope) {
        Ok(engine) => {
            tracing::info!(
                turn = engine.game.turn,
                history_count = engine.game.history.len(),
                "autosave restored"
            );
            Some(engine)
        }
        Err(error) => {
            tracing::warn!(%error, "autosave was not resumed");
            None
        }
    }
}

fn engine_from_envelope(pack: &StoryPack, envelope: SaveEnvelope) -> Result<EngineState, String> {
    let metadata = &pack.metadata;
    if envelope.story_id != metadata.story_id
        || envelope.story_version != metadata.story_version
        || envelope.pack_schema_version != metadata.schema_version
    {
        return Err("현재 Story Pack과 다른 버전의 저장입니다".to_string());
    }
    Ok(EngineState::from_snapshot_payload(pack, envelope.payload))
}

const MANUAL_SAVE_SLOTS: [&str; 3] = ["slot-1", "slot-2", "slot-3"];

fn selectable_slots(save_store: &SaveStore, pack: &StoryPack, mode: SaveLoadMode) -> Vec<SlotMeta> {
    let mut existing = save_store
        .list_slots()
        .into_iter()
        .filter(|slot| slot.story_id == pack.metadata.story_id)
        .collect::<Vec<_>>();
    existing.sort_by(|a, b| a.slot_id.cmp(&b.slot_id));
    if mode == SaveLoadMode::Load {
        return existing;
    }
    MANUAL_SAVE_SLOTS
        .iter()
        .map(|id| {
            existing
                .iter()
                .find(|slot| slot.slot_id == *id)
                .cloned()
                .unwrap_or_else(|| SlotMeta {
                    slot_id: (*id).to_string(),
                    story_id: String::new(),
                    saved_at: story_model::chrono::Utc::now(),
                    chapter_label: String::new(),
                    scene_label: String::new(),
                    description: None,
                })
        })
        .collect()
}

pub fn handle_request(
    request: SaveLoadRequest,
    engine: &mut EngineState,
    app: &mut App,
    pack: &StoryPack,
    save_store: &SaveStore,
    cancellation: &mut Option<(story_model::uuid::Uuid, CancellationToken)>,
) {
    match request {
        SaveLoadRequest::Open(mode) => {
            app.open_save_load(mode, selectable_slots(save_store, pack, mode))
        }
        SaveLoadRequest::Save { slot_id } => {
            let description = Some(format!(
                "턴 {} — {}",
                engine.game.turn, engine.game.current_scene_id
            ));
            match save_engine_snapshot(save_store, engine, slot_id, description) {
                Ok(()) => app.close_save_load(),
                Err(error) => {
                    app.close_save_load();
                    app.view.error = Some(save_error_view("수동 저장 실패", error));
                }
            }
        }
        SaveLoadRequest::Load { slot_id } => {
            let restored = save_store
                .load(&slot_id)
                .map_err(|error| error.to_string())
                .and_then(|envelope| engine_from_envelope(pack, envelope));
            match restored {
                Ok(restored) => {
                    if let Some((_, token)) = cancellation.take() {
                        token.cancel();
                    }
                    *engine = restored;
                    app.close_save_load();
                    app.set_lists(&engine.game.history, &engine.game.memories);
                    app.set_view(build_view(engine, pack, None, None));
                    app.follow_story_end();
                }
                Err(error) => {
                    app.close_save_load();
                    app.view.error = Some(save_error_view("저장 불러오기 실패", error));
                }
            }
        }
    }
}

pub fn save_engine_snapshot(
    save_store: &SaveStore,
    engine: &EngineState,
    slot_id: String,
    description: Option<String>,
) -> Result<(), String> {
    let game = &engine.game;
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
        payload: engine.snapshot_payload(),
    };
    save_store.save(envelope).map_err(|error| error.to_string())
}

pub fn save_error_view(title: &str, message: String) -> DomainErrorView {
    DomainErrorView {
        code: "save_failed".to_string(),
        title_ko: title.to_string(),
        message_ko: message,
        recoverable: true,
    }
}
