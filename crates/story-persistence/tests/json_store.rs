//! W8.2 tests: SaveStore save/load/list/delete + checksum-verified load.
//!
//! `list_slots` reads only envelope headers (no full payload materialization),
//! and a slot MUST NOT appear in `list_slots` until its write fully succeeds.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use story_model::{
    CharacterId, CharacterState, Emotion, GameState, LocationId, RelationshipStage, SaveEnvelope,
    SavePayload, TimeSlot,
};
use story_persistence::{SaveError, SaveStore};
use tempfile::tempdir;

fn state(turn: u32) -> GameState {
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
        save_schema_version: story_model::SAVE_SCHEMA_VERSION,
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
        lorebook_activations: Vec::new(),
    }
}

fn payload(turn: u32) -> SavePayload {
    let game = state(turn);
    let memories = game.memories.clone();
    let history = game.history.clone();
    SavePayload {
        stable_state: game,
        memories,
        history,
        audit_tail: Vec::new(),
    }
}

fn envelope(slot: &str, turn: u32) -> SaveEnvelope {
    let p = payload(turn);
    let checksum = story_persistence::payload_checksum(&p);
    SaveEnvelope {
        save_schema_version: story_model::SAVE_SCHEMA_VERSION,
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
fn save_and_load_round_trip() {
    let dir = tempdir().unwrap();
    let store = SaveStore::open(dir.path()).unwrap();
    let env = envelope("slot-1", 1);
    store.save(env.clone()).expect("save");

    let loaded = store.load("slot-1").expect("load");
    assert_eq!(loaded.slot_id, "slot-1");
    assert_eq!(loaded.payload.stable_state.turn, 1);
    assert_eq!(
        loaded.payload_checksum,
        story_persistence::payload_checksum(&loaded.payload),
        "loaded payload still matches its checksum"
    );
}

#[test]
fn list_slots_returns_meta_after_save() {
    let dir = tempdir().unwrap();
    let store = SaveStore::open(dir.path()).unwrap();
    assert!(store.list_slots().is_empty(), "fresh store has no slots");

    store.save(envelope("slot-1", 3)).unwrap();
    store.save(envelope("slot-2", 7)).unwrap();
    let mut slots = store.list_slots();
    slots.sort_by(|a, b| a.slot_id.cmp(&b.slot_id));

    assert_eq!(slots.len(), 2);
    assert_eq!(slots[0].slot_id, "slot-1");
    assert_eq!(slots[0].story_id, "rainy-reunion");
    assert_eq!(slots[0].description.as_deref(), Some("turn 3"));
    assert_eq!(slots[0].chapter_label, "chapter-1");
    assert_eq!(slots[1].slot_id, "slot-2");
}

#[test]
fn load_missing_slot_returns_missing_slot_error() {
    let dir = tempdir().unwrap();
    let store = SaveStore::open(dir.path()).unwrap();
    match store.load("nope") {
        Err(SaveError::MissingSlot(_)) => (),
        other => panic!("expected MissingSlot, got {other:?}"),
    }
}

#[test]
fn delete_removes_slot_from_list() {
    let dir = tempdir().unwrap();
    let store = SaveStore::open(dir.path()).unwrap();
    store.save(envelope("slot-1", 1)).unwrap();
    assert_eq!(store.list_slots().len(), 1);

    store.delete("slot-1").unwrap();
    assert!(store.list_slots().is_empty());
    assert!(matches!(
        store.load("slot-1"),
        Err(SaveError::MissingSlot(_))
    ));
}

#[test]
fn delete_missing_slot_is_ok() {
    let dir = tempdir().unwrap();
    let store = SaveStore::open(dir.path()).unwrap();
    // Deleting a non-existent slot is a no-op (idempotent).
    store.delete("never").unwrap();
}

#[test]
fn slot_path_is_root_slash_slot_id_json() {
    let dir = tempdir().unwrap();
    let store = SaveStore::open(dir.path()).unwrap();
    let p = store.slot_path("autosave").unwrap();
    assert_eq!(p.file_name().unwrap(), "autosave.json");
    assert_eq!(p.parent().unwrap(), dir.path());
}

#[test]
fn overwriting_slot_replaces_payload() {
    let dir = tempdir().unwrap();
    let store = SaveStore::open(dir.path()).unwrap();
    store.save(envelope("slot-1", 1)).unwrap();
    store.save(envelope("slot-1", 5)).unwrap();

    let loaded = store.load("slot-1").unwrap();
    assert_eq!(loaded.payload.stable_state.turn, 5);
}

#[test]
fn load_diagnoses_truncated_json() {
    let dir = tempdir().unwrap();
    let store = SaveStore::open(dir.path()).unwrap();
    store.save(envelope("slot-1", 1)).unwrap();

    // Truncate the file.
    let path = store.slot_path("slot-1").unwrap();
    let original = std::fs::read(&path).unwrap();
    let half = &original[..original.len() / 2];
    std::fs::write(&path, half).unwrap();

    match store.load("slot-1") {
        Err(SaveError::TruncatedOrCorrupt(_)) => (),
        other => panic!("expected TruncatedOrCorrupt, got {other:?}"),
    }
}

#[test]
fn load_diagnoses_checksum_mismatch_and_leaves_file_untouched() {
    let dir = tempdir().unwrap();
    let store = SaveStore::open(dir.path()).unwrap();
    store.save(envelope("slot-1", 1)).unwrap();

    // Tamper with the on-disk JSON: flip one character in the narration-ish
    // payload region. Find a digit in the JSON and bump it.
    let path = store.slot_path("slot-1").unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    // Find a numeric character in the payload section (after "payload").
    let payload_idx = bytes
        .windows(7)
        .position(|w| w == b"payload")
        .expect("payload key present");
    let flip_at = bytes[payload_idx..]
        .iter()
        .position(|b| b.is_ascii_digit())
        .map(|i| i + payload_idx)
        .expect("digit in payload");
    bytes[flip_at] = if bytes[flip_at] == b'9' {
        b'0'
    } else {
        bytes[flip_at] + 1
    };
    std::fs::write(&path, &bytes).unwrap();

    match store.load("slot-1") {
        Err(SaveError::ChecksumMismatch { slot }) => assert_eq!(slot, "slot-1"),
        other => panic!("expected ChecksumMismatch, got {other:?}"),
    }

    // The on-disk file is unchanged from our tampered write (load never
    // mutates). Re-tampering back would still mismatch by construction.
    let still_tampered = std::fs::read(&path).unwrap();
    assert_eq!(still_tampered, bytes);
}

#[test]
fn unsafe_slot_ids_are_rejected_before_filesystem_access() {
    let dir = tempdir().unwrap();
    let store = SaveStore::open(dir.path()).unwrap();
    for invalid in ["", "../outside", "nested/slot", "slot.json", "슬롯"] {
        assert!(store.slot_path(invalid).is_err(), "must reject {invalid:?}");
        assert!(matches!(
            store.load(invalid),
            Err(SaveError::InvalidSlotId(id)) if id == invalid
        ));
        assert!(store.delete(invalid).is_err());
        assert!(store.save(envelope(invalid, 1)).is_err());
    }
    let too_long = "a".repeat(story_persistence::json_store::MAX_SLOT_ID_LEN + 1);
    assert!(store.slot_path(&too_long).is_err());
    assert!(dir.path().read_dir().unwrap().next().is_none());
}
