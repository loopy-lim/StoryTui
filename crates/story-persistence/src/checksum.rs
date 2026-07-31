//! SHA-256 checksum over the canonical serialization of a [`SavePayload`] (W8, ST-025).
//!
//! The checksum is the integrity gate for a save file: the writer computes it
//! before writing; the loader verifies it before use and refuses on mismatch
//! (the on-disk file is never mutated). Because [`SavePayload`] is composed of
//! `BTreeMap`-backed types, `serde_json::to_vec` yields byte-deterministic
//! output, so the checksum is stable across runs and across hosts.

use sha2::{Digest, Sha256};
use story_model::SavePayload;

/// Algorithm id stored in `SaveEnvelope::checksum_algorithm`.
pub const CHECKSUM_ALGORITHM: &str = "sha256";

/// Compute the hex SHA-256 of the canonical JSON serialization of `payload`.
///
/// Canonical = `serde_json::to_vec(payload)`. Determinism relies on the
/// `BTreeMap` ordering of every map inside [`SavePayload`] / [`GameState`]
/// (see `story-model`); `HashMap` would break this contract.
pub fn payload_checksum(payload: &SavePayload) -> String {
    let bytes = serde_json::to_vec(payload).expect("SavePayload serializes");
    hex_sha256(&bytes)
}

/// Verify `payload` against `checksum` using a constant-time-ish comparison.
///
/// Returns `false` (never panics) on length mismatch or hex mismatch.
pub fn verify(payload: &SavePayload, checksum: &str) -> bool {
    let actual = payload_checksum(payload);
    constant_time_eq(actual.as_bytes(), checksum.as_bytes())
}

/// Compute the hex SHA-256 digest of `bytes`.
pub fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Length-checked byte equality. Not a true constant-time implementation, but
/// avoids the early-exit timing leak of `==` on the digest.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_payload() -> SavePayload {
        use std::collections::{BTreeMap, BTreeSet, VecDeque};
        use story_model::{
            CharacterId, CharacterState, Emotion, GameState, LocationId, RelationshipStage,
            TimeSlot,
        };
        let mut characters = BTreeMap::new();
        characters.insert(
            CharacterId("c-namu".to_string()),
            CharacterState {
                affection: 3,
                trust: 1,
                tension: 8,
                relationship_stage: RelationshipStage::Acquaintance,
                current_emotion: Emotion::Neutral,
                known_facts: BTreeSet::new(),
                address_form_overrides: BTreeMap::new(),
            },
        );
        let state = GameState {
            save_schema_version: story_model::SAVE_SCHEMA_VERSION,
            story_id: "rainy-reunion".to_string(),
            story_version: semver::Version::new(0, 1, 0),
            pack_schema_version: semver::Version::new(1, 0, 0),
            session_id: uuid::Uuid::nil(),
            current_chapter: "chapter-1".to_string(),
            current_scene_id: "cafe-namu".to_string(),
            turn: 1,
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
            revision: 0,
            stable_generation_id: None,
        };
        SavePayload {
            stable_state: state,
            memories: Vec::new(),
            history: Vec::new(),
            audit_tail: Vec::new(),
        }
    }

    #[test]
    fn checksum_stable_across_calls() {
        let p = sample_payload();
        assert_eq!(payload_checksum(&p), payload_checksum(&p));
    }

    #[test]
    fn verify_rejects_wrong_checksum() {
        let p = sample_payload();
        assert!(!verify(&p, "00".repeat(32).as_str()));
    }
}
