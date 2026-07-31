//! W8.3 tests: migration + corruption diagnosis.
//!
//! These exercise the `SaveStore::load` path end-to-end:
//! - A v0 fixture is migrated to v1 in-memory; the on-disk file is unchanged.
//! - A future-major (`save_schema_version = 99`) envelope is rejected.
//! - A deliberately corrupted file is diagnosed as TruncatedOrCorrupt.
//! - A `migrate_payload` direct call rejects a future major.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use story_model::{
    CharacterId, CharacterState, Emotion, GameState, LocationId, RelationshipStage, SaveEnvelope,
    SavePayload, TimeSlot,
};
use story_persistence::{SaveError, SaveStore, migrate_payload};
use tempfile::tempdir;

fn state(version: u32, turn: u32) -> GameState {
    let mut characters = BTreeMap::new();
    characters.insert(
        CharacterId("c-namu".to_string()),
        CharacterState {
            affection: 3,
            trust: 1,
            tension: 8 + (turn as i16),
            relationship_stage: RelationshipStage::Acquaintance,
            current_emotion: Emotion::Neutral,
            known_facts: BTreeSet::new(),
            address_form_overrides: BTreeMap::new(),
        },
    );
    GameState {
        save_schema_version: version,
        story_id: "rainy-reunion".to_string(),
        story_version: semver::Version::new(0, 1, 0),
        pack_schema_version: semver::Version::new(1, 0, 0),
        session_id: uuid::Uuid::nil(),
        current_chapter: "chapter-1".to_string(),
        current_scene_id: "cafe-namu".to_string(),
        turn,
        location: LocationId("cafe-namu".to_string()),
        time_slot: TimeSlot::Evening,
        active_route: None,
        characters,
        flags: BTreeMap::new(),
        numbers: BTreeMap::new(),
        strings: BTreeMap::new(),
        inventory: BTreeMap::new(),
        occurred_events: BTreeSet::new(),
        recent_summaries: VecDeque::new(),
        memories: Vec::new(),
        history: Vec::new(),
        last_choice: None,
        revision: turn as u64,
        stable_generation_id: None,
    }
}

fn payload(version: u32, turn: u32) -> SavePayload {
    let game = state(version, turn);
    let memories = game.memories.clone();
    let history = game.history.clone();
    SavePayload {
        stable_state: game,
        memories,
        history,
        audit_tail: Vec::new(),
    }
}

/// Build a fully-formed envelope with the given on-disk schema version. The
/// checksum is computed over the actual payload, so the file verifies clean.
fn envelope(slot: &str, schema: u32, turn: u32) -> SaveEnvelope {
    let p = payload(schema, turn);
    let checksum = story_persistence::payload_checksum(&p);
    SaveEnvelope {
        save_schema_version: schema,
        engine_version: "0.1.0".to_string(),
        story_id: p.stable_state.story_id.clone(),
        story_version: p.stable_state.story_version.clone(),
        pack_schema_version: p.stable_state.pack_schema_version.clone(),
        slot_id: slot.to_string(),
        description: Some(format!("turn {turn}")),
        saved_at: chrono::DateTime::parse_from_rfc3339("2026-08-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
        chapter_label: "chapter-1".to_string(),
        scene_label: "cafe-namu".to_string(),
        checksum_algorithm: "sha256".to_string(),
        payload_checksum: checksum,
        payload: p,
    }
}

#[test]
fn migrate_payload_v0_to_v1_in_memory() {
    let p = payload(0, 1);
    let migrated = migrate_payload(p.clone(), 0, 1).expect("migrate");
    assert_eq!(migrated.stable_state.save_schema_version, 1);
    // Pure transform: other fields untouched.
    assert_eq!(migrated.stable_state.turn, p.stable_state.turn);
    assert_eq!(migrated.stable_state.story_id, p.stable_state.story_id);
}

#[test]
fn migrate_payload_rejects_future_major() {
    let p = payload(99, 1);
    match migrate_payload(p, 99, 1) {
        Err(SaveError::UnsupportedVersion { found, supported }) => {
            assert_eq!(found, 99);
            assert_eq!(supported, 1);
        }
        other => panic!("expected UnsupportedVersion, got {other:?}"),
    }
}

#[test]
fn load_v0_save_migrates_in_memory_and_leaves_disk_untouched() {
    let dir = tempdir().unwrap();
    let store = SaveStore::open(dir.path()).unwrap();

    // Write a v0 envelope by hand to the slot path (store.save would
    // re-stamp the schema version under SAVE_SCHEMA_VERSION; we want v0 on
    // disk to exercise the load-time migration path).
    let env = envelope("slot-1", 0, 1);
    let bytes = serde_json::to_vec_pretty(&env).unwrap();
    std::fs::write(store.slot_path("slot-1"), &bytes).unwrap();

    // Capture the on-disk bytes — they must not change after load.
    let before = std::fs::read(store.slot_path("slot-1")).unwrap();

    let loaded = store.load("slot-1").expect("migrate then load");
    assert_eq!(
        loaded.save_schema_version,
        story_persistence::SAVE_SCHEMA_VERSION,
        "loaded envelope reports the supported version"
    );
    assert_eq!(
        loaded.payload.stable_state.save_schema_version,
        story_persistence::SAVE_SCHEMA_VERSION,
        "payload's embedded schema version was migrated"
    );

    // The on-disk file is byte-identical — migration is in-memory only.
    let after = std::fs::read(store.slot_path("slot-1")).unwrap();
    assert_eq!(before, after, "migration MUST NOT mutate the original file");
}

#[test]
fn load_future_major_save_is_unsupported_version() {
    let dir = tempdir().unwrap();
    let store = SaveStore::open(dir.path()).unwrap();

    let env = envelope("slot-1", 99, 1);
    let bytes = serde_json::to_vec_pretty(&env).unwrap();
    std::fs::write(store.slot_path("slot-1"), &bytes).unwrap();

    match store.load("slot-1") {
        Err(SaveError::UnsupportedVersion { found, supported }) => {
            assert_eq!(found, 99);
            assert_eq!(supported, story_persistence::SAVE_SCHEMA_VERSION);
        }
        other => panic!("expected UnsupportedVersion, got {other:?}"),
    }
}

#[test]
fn load_truncated_file_is_truncated_or_corrupt() {
    let dir = tempdir().unwrap();
    let store = SaveStore::open(dir.path()).unwrap();
    store.save(envelope("slot-1", 1, 1)).unwrap();

    let path = store.slot_path("slot-1");
    let original = std::fs::read(&path).unwrap();
    // Slice off the last 30% — guaranteed invalid JSON.
    let cut = (original.len() * 7) / 10;
    std::fs::write(&path, &original[..cut]).unwrap();

    match store.load("slot-1") {
        Err(SaveError::TruncatedOrCorrupt(_)) => (),
        other => panic!("expected TruncatedOrCorrupt, got {other:?}"),
    }
}

#[test]
fn load_garbage_file_is_truncated_or_corrupt() {
    let dir = tempdir().unwrap();
    let store = SaveStore::open(dir.path()).unwrap();
    std::fs::write(store.slot_path("slot-1"), b"not json at all {{{{").unwrap();
    match store.load("slot-1") {
        Err(SaveError::TruncatedOrCorrupt(_)) => (),
        other => panic!("expected TruncatedOrCorrupt, got {other:?}"),
    }
}

#[test]
fn load_current_version_is_no_migration() {
    // Sanity: a current-version save does not route through migration and
    // loads with the schema version intact.
    let dir = tempdir().unwrap();
    let store = SaveStore::open(dir.path()).unwrap();
    store
        .save(envelope(
            "slot-1",
            story_persistence::SAVE_SCHEMA_VERSION,
            3,
        ))
        .unwrap();
    let loaded = store.load("slot-1").unwrap();
    assert_eq!(
        loaded.save_schema_version,
        story_persistence::SAVE_SCHEMA_VERSION
    );
    assert_eq!(loaded.payload.stable_state.turn, 3);
}
