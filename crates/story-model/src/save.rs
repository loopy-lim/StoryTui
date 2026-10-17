//! Save-file envelope and payload types (W8, ST-025).
//!
//! These are the on-disk shapes a `SaveStore` writes and reads. They live in
//! `story-model` so both `story-persistence` (the writer/reader) and
//! `story-core` (which assembles a [`SavePayload`] from its state) can share
//! them without taking a dependency on each other. The types are plain data:
//! no FS / UI / HTTP (ADR-003).
//!
//! # Checksum contract
//!
//! `payload_checksum` is the hex SHA-256 of the canonical JSON serialization
//! of [`SavePayload`] (`serde_json::to_vec`, deterministic via `BTreeMap`
//! ordering). The writer computes it; the loader verifies it before use and
//! refuses on mismatch (nothing mutated).
//!
//! # Forbidden contents
//!
//! Saves MUST NOT carry provider prompts, API keys, or partial Writer
//! buffers. [`SavePayload`] only holds stable state + memories + history +
//! audit tail — none of those fields are secrets.

use serde::{Deserialize, Serialize};

use crate::{GameState, HistoryEntry, MemoryEntry, StateChangeLog};

/// The save payload: stable game state + the recall/history/audit tails that
/// are needed to resume a session. No prompts, no API key, no streaming
/// buffer (see the [`crate::save`] module docs).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavePayload {
    /// Committed stable game state (the save-file keystone).
    pub stable_state: GameState,
    /// Long-term memories accumulated over the session. Mirrored from
    /// `stable_state.memories` for forward-compatibility with the W9 memory
    /// selector; the writer fills it from the same source.
    pub memories: Vec<MemoryEntry>,
    /// Full per-turn history. Mirrored from `stable_state.history`.
    pub history: Vec<HistoryEntry>,
    /// Tail of the field-level audit log (most recent entries). Bounded by
    /// the engine to keep saves compact.
    pub audit_tail: Vec<StateChangeLog>,
}

/// The on-disk envelope wrapping a [`SavePayload`] with the metadata the
/// loader needs to verify integrity and route migration.
///
/// Field order follows the W8 plan / Technical Spec §13.1 verbatim. The
/// `checksum_algorithm` is currently always `"sha256"`; the loader rejects
/// any other value.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SaveEnvelope {
    /// Save schema revision this envelope was written with.
    pub save_schema_version: u32,
    /// Engine build that wrote this save (`env!("CARGO_PKG_VERSION")` of the
    /// host app). Diagnostic only — does not drive migration logic.
    pub engine_version: String,
    /// Pack id this save belongs to.
    pub story_id: String,
    /// Semantic version of the story pack this save was created with.
    pub story_version: semver::Version,
    /// Schema version of the story pack format (independent of save schema).
    pub pack_schema_version: semver::Version,
    /// Slot id this envelope was written to (e.g. `"autosave"`, `"slot-1"`).
    pub slot_id: String,
    /// Optional human-readable description (player-facing label).
    pub description: Option<String>,
    /// Wall-clock time the save was written.
    pub saved_at: chrono::DateTime<chrono::Utc>,
    /// Display label for the current chapter (diagnostic).
    pub chapter_label: String,
    /// Display label for the current scene (diagnostic).
    pub scene_label: String,
    /// Checksum algorithm in use. Currently always `"sha256"`.
    pub checksum_algorithm: String,
    /// Hex SHA-256 of the canonical JSON serialization of `payload`.
    pub payload_checksum: String,
    /// The payload itself.
    pub payload: SavePayload,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CharacterState, Emotion, LocationId, RelationshipStage, TimeSlot};
    use std::collections::{BTreeMap, BTreeSet, VecDeque};

    fn sample_state() -> GameState {
        GameState {
            save_schema_version: crate::SAVE_SCHEMA_VERSION,
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
            characters: BTreeMap::new(),
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
            lorebook_activations: Vec::new(),
        }
        .with_primary_character()
    }

    trait WithPrimary {
        fn with_primary_character(self) -> Self;
    }
    impl WithPrimary for GameState {
        fn with_primary_character(self) -> Self {
            let mut s = self;
            s.characters.insert(
                crate::CharacterId("c-namu".to_string()),
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
            s
        }
    }

    #[test]
    fn envelope_round_trips() {
        let state = sample_state();
        let payload = SavePayload {
            stable_state: state,
            memories: Vec::new(),
            history: Vec::new(),
            audit_tail: Vec::new(),
        };
        let envelope = SaveEnvelope {
            save_schema_version: crate::SAVE_SCHEMA_VERSION,
            engine_version: "0.1.0".to_string(),
            story_id: "rainy-reunion".to_string(),
            story_version: semver::Version::new(0, 1, 0),
            pack_schema_version: semver::Version::new(1, 0, 0),
            slot_id: "autosave".to_string(),
            description: Some("자동 저장".to_string()),
            saved_at: chrono::DateTime::parse_from_rfc3339("2026-08-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            chapter_label: "chapter-1".to_string(),
            scene_label: "cafe-namu".to_string(),
            checksum_algorithm: "sha256".to_string(),
            payload_checksum: "deadbeef".to_string(),
            payload,
        };
        let json = serde_json::to_string(&envelope).expect("serialize");
        let back: SaveEnvelope = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.slot_id, envelope.slot_id);
        assert_eq!(back.payload.stable_state.story_id, "rainy-reunion");
    }
}
