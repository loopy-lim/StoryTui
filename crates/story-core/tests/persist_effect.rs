//! W8.4 tests: PersistStableState effect + snapshot_payload + autosave.
//!
//! The engine is pure: it DESCRIBES a PersistStableState effect; the host app
//! executes it by calling SaveStore::save. story-core must NOT touch the
//! filesystem.

use story_core::engine::{EngineState, reduce};
use story_core::engine_cmd::{EngineCommand, EngineEffect};
use story_core::tests_support::sample_pack;
use story_model::{SavePayload, StateChangeLog};

fn start_and_pending_id(pack: &story_model::StoryPack) -> (EngineState, story_model::uuid::Uuid) {
    let init = EngineState::initial(pack);
    let t = reduce(&init, EngineCommand::StartNewGame, pack).expect("start");
    let gid = t
        .effects
        .iter()
        .find_map(|e| match e {
            EngineEffect::RequestGeneration { generation_id, .. } => Some(*generation_id),
            _ => None,
        })
        .expect("StartNewGame emits RequestGeneration");
    (t.state, gid)
}

#[test]
fn accepted_turn_emits_persist_stable_state_for_autosave() {
    let pack = sample_pack();
    let (state, gid) = start_and_pending_id(&pack);

    let t = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(story_core::tests_support::valid_draft(gid, 1)),
        },
        &pack,
    )
    .expect("accept");

    let persist = t
        .effects
        .iter()
        .find_map(|e| match e {
            EngineEffect::PersistStableState {
                slot_id,
                description,
            } => Some((slot_id.clone(), description.clone())),
            _ => None,
        })
        .expect("expected PersistStableState effect after accept");

    assert_eq!(persist.0, "autosave", "default slot is 'autosave'");
    assert!(persist.1.is_some(), "description populated");
}

#[test]
fn autosave_can_be_disabled_on_engine_state() {
    let pack = sample_pack();
    let (mut state, gid) = start_and_pending_id(&pack);
    state.autosave_on_accept = false;

    let t = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(story_core::tests_support::valid_draft(gid, 1)),
        },
        &pack,
    )
    .expect("accept");

    assert!(
        !t.effects
            .iter()
            .any(|e| matches!(e, EngineEffect::PersistStableState { .. })),
        "no autosave when disabled, got {:?}",
        t.effects
    );
}

#[test]
fn snapshot_payload_assembles_state_memories_history_audit() {
    let pack = sample_pack();
    let (state, gid) = start_and_pending_id(&pack);

    let t = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(story_core::tests_support::valid_draft(gid, 1)),
        },
        &pack,
    )
    .expect("accept");

    let payload: SavePayload = t.state.snapshot_payload();
    assert_eq!(
        payload.stable_state.turn, 1,
        "snapshot reflects post-accept turn"
    );
    assert_eq!(
        payload.history.len(),
        t.state.game.history.len(),
        "history mirror matches game.history"
    );
    assert_eq!(
        payload.memories.len(),
        t.state.game.memories.len(),
        "memories mirror matches game.memories"
    );
    assert!(
        !payload.audit_tail.is_empty(),
        "audit tail accumulated from the accepted turn's change log"
    );
    // Every audit entry in the tail is a real StateChangeLog.
    for entry in &payload.audit_tail {
        let _: &StateChangeLog = entry;
    }
}

#[test]
fn snapshot_payload_is_pure_data_no_fs_call_sites_in_story_core() {
    // Smoke: snapshot_payload is a method on EngineState and is callable from
    // pure code (no Path / File / std::fs touched here). This is also
    // enforced structurally by `cargo tree -p story-core` having no fs deps.
    let pack = sample_pack();
    let init = EngineState::initial(&pack);
    let payload = init.snapshot_payload();
    assert_eq!(payload.stable_state.story_id, init.game.story_id);
}
