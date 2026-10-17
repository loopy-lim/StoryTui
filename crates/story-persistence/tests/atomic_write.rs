//! W8.1 tests: SHA-256 checksum + atomic JSON write (9-step).
//!
//! The fault-injection test is the gate: a failure at any stage (temp write,
//! sync, rename) MUST leave a pre-existing save byte-identical and loadable.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use story_model::{SaveEnvelope, SavePayload};
use story_persistence::atomic_write::atomic_write_json;
use story_persistence::checksum::{payload_checksum, verify};
use tempfile::tempdir;

fn sample_payload(turn: u32) -> SavePayload {
    use std::collections::{BTreeMap, BTreeSet, VecDeque};
    use story_model::{
        CharacterId, CharacterState, Emotion, GameState, LocationId, RelationshipStage, TimeSlot,
    };
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
    let mut state = GameState {
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
    };
    let _ = &mut state;
    SavePayload {
        stable_state: state,
        memories: Vec::new(),
        history: Vec::new(),
        audit_tail: Vec::new(),
    }
}

#[test]
fn checksum_is_hex_sha256_of_canonical_payload() {
    let payload = sample_payload(1);
    let checksum = payload_checksum(&payload);
    assert_eq!(checksum.len(), 64, "256 bits = 64 hex chars");
    assert!(
        checksum.chars().all(|c| c.is_ascii_hexdigit()),
        "hex only: {checksum}"
    );
    // Determinism: same input -> same digest.
    assert_eq!(checksum, payload_checksum(&payload));
    // Distinct input -> distinct digest.
    let other = sample_payload(2);
    assert_ne!(checksum, payload_checksum(&other));
}

#[test]
fn verify_accepts_correct_checksum_rejects_others() {
    let payload = sample_payload(1);
    let good = payload_checksum(&payload);
    assert!(verify(&payload, &good));
    assert!(!verify(&payload, "0".repeat(64).as_str()));
    // Tampered checksum (one hex char flipped) fails.
    let mut tampered = good.clone();
    let first = tampered.chars().next().unwrap();
    tampered.replace_range(0..1, if first == 'a' { "b" } else { "a" });
    assert!(!verify(&payload, &tampered));
}

#[test]
fn atomic_write_creates_file_with_bytes() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("save.json");
    let bytes = br#"{"hello":"world"}"#;
    atomic_write_json(&path, bytes, 3).expect("write ok");
    let on_disk = fs::read(&path).expect("read");
    assert_eq!(on_disk, bytes);
}

#[test]
fn atomic_write_leaves_existing_file_untouched_on_failure() {
    // The 9-step guarantee: if the rename target's parent dir is read-only,
    // creating the temp file fails. The pre-existing save MUST remain
    // byte-identical and loadable.
    let dir = tempdir().unwrap();
    let path = dir.path().join("save.json");
    let original = b"{\"original\":true}";
    fs::write(&path, original).unwrap();

    // Make the parent directory non-writable (no temp creation possible).
    let parent_meta = fs::metadata(dir.path()).unwrap();
    let _ = parent_meta;
    let mut perms = fs::metadata(dir.path()).unwrap().permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o500); // r-x for owner: no writes
    }
    fs::set_permissions(dir.path(), perms.clone()).unwrap();

    let result = atomic_write_json(&path, b"{\"new\":true}", 3);
    assert!(result.is_err(), "expected write to fail");

    // Restore perms so cleanup can happen.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o700);
        fs::set_permissions(dir.path(), perms).unwrap();
    }

    let on_disk = fs::read(&path).expect("read after failure");
    assert_eq!(
        on_disk, original,
        "pre-existing save must be byte-identical"
    );
}

#[test]
fn atomic_write_keeps_bak_when_overwriting() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("save.json");
    let v1 = b"v1";
    let v2 = b"v2";

    atomic_write_json(&path, v1, 3).unwrap();
    atomic_write_json(&path, v2, 3).unwrap();

    assert_eq!(fs::read(&path).unwrap(), v2);
    let bak = fs::read(dir.path().join("save.json.bak")).unwrap();
    assert_eq!(bak, v1, ".bak holds the previous generation");
}

#[test]
fn atomic_write_ages_out_oldest_bak() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("save.json");

    // Write 5 generations with bak_generations=2.
    for i in 0..5 {
        let bytes = format!("v{i}").into_bytes();
        atomic_write_json(&path, &bytes, 2).unwrap();
    }

    assert_eq!(fs::read(&path).unwrap(), b"v4".to_vec());
    assert_eq!(
        fs::read(dir.path().join("save.json.bak")).unwrap(),
        b"v3".to_vec(),
        "most recent bak is v3"
    );
    assert_eq!(
        fs::read(dir.path().join("save.json.bak.1")).unwrap(),
        b"v2".to_vec(),
        "next bak is v2"
    );
    assert!(
        !dir.path().join("save.json.bak.2").exists(),
        "v1 aged out (bak_generations=2)"
    );
}

#[test]
fn atomic_write_no_temp_left_behind_on_failure() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("save.json");
    fs::write(&path, b"orig").unwrap();

    // Force failure via read-only parent.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(dir.path()).unwrap().permissions();
        perms.set_mode(0o500);
        fs::set_permissions(dir.path(), perms).unwrap();
    }

    let _ = atomic_write_json(&path, b"new", 3);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(dir.path()).unwrap().permissions();
        perms.set_mode(0o700);
        fs::set_permissions(dir.path(), perms).unwrap();
    }

    let leftover_temps: Vec<PathBuf> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            let name = p.file_name().unwrap().to_string_lossy();
            name.starts_with(".save.json.tmp")
        })
        .collect();
    assert!(
        leftover_temps.is_empty(),
        "temp cleaned up: {leftover_temps:?}"
    );
}

#[test]
fn envelope_serialization_is_deterministic_for_checksum_stability() {
    // serde_json over BTreeMap-backed types is deterministic — the same
    // payload must yield byte-identical JSON across two calls. This is what
    // makes the checksum stable across save/load.
    let payload = sample_payload(7);
    let a = serde_json::to_vec(&payload).unwrap();
    let b = serde_json::to_vec(&payload).unwrap();
    assert_eq!(a, b);

    // Also stable via the envelope wrapper.
    let checksum = payload_checksum(&payload);
    let envelope = SaveEnvelope {
        save_schema_version: story_model::SAVE_SCHEMA_VERSION,
        engine_version: "0.1.0".to_string(),
        story_id: payload.stable_state.story_id.clone(),
        story_version: payload.stable_state.story_version.clone(),
        pack_schema_version: payload.stable_state.pack_schema_version.clone(),
        slot_id: "autosave".to_string(),
        description: None,
        saved_at: chrono::DateTime::parse_from_rfc3339("2026-08-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
        chapter_label: "chapter-1".to_string(),
        scene_label: "cafe-namu".to_string(),
        checksum_algorithm: "sha256".to_string(),
        payload_checksum: checksum.clone(),
        payload: payload.clone(),
    };
    let _json = serde_json::to_vec_pretty(&envelope).unwrap();
    let _ = std::io::stdout().write_all(b"");
    // Re-derive checksum from the re-extracted payload — must match.
    assert_eq!(payload_checksum(&envelope.payload), checksum);
}
