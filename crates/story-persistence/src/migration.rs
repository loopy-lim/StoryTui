//! Save schema migration (W8, ST-027).
//!
//! Migration is **in-memory only**: it transforms a [`SavePayload`] read from
//! disk into the shape this build of `story-persistence` understands, without
//! ever writing back to the original save file. A future-major save (its
//! `save_schema_version` is greater than this build's) is rejected with
//! [`SaveError::UnsupportedVersion`].
//!
//! v0 → v1: bumps `stable_state.save_schema_version` to 1 and ensures any
//! optional default fields are present. Our `SAVE_SCHEMA_VERSION` has been 1
//! since W2, so this migration is effectively a version bump plus light
//! sanity-defaulting; it exists to exercise the migration code path and to
//! give future versions a place to hang structural transforms.

use story_model::SavePayload;

use crate::error::{SAVE_SCHEMA_VERSION, SaveError};

/// Migrate `payload` from `from` to `to` (inclusive), one major step at a
/// time. Each step is a pure transform. The result is the migrated payload;
/// the on-disk file is the caller's responsibility (we never write here).
///
/// `from > to` (a future-major save loaded by an older build) yields
/// [`SaveError::UnsupportedVersion`].
pub fn migrate_payload(payload: SavePayload, from: u32, to: u32) -> Result<SavePayload, SaveError> {
    if from > to {
        return Err(SaveError::UnsupportedVersion {
            found: from,
            supported: to,
        });
    }
    if from == to {
        return Ok(payload);
    }
    let mut current = payload;
    let mut version = from;
    while version < to {
        current = migrate_one(current, version)?;
        version += 1;
    }
    Ok(current)
}

/// Apply a single major-step migration: `version` → `version + 1`.
fn migrate_one(payload: SavePayload, from: u32) -> Result<SavePayload, SaveError> {
    match from {
        0 => {
            // v0 → v1: bump schema version, fill default empty containers if
            // the v0 save somehow omitted them (forward-tolerance via serde
            // means they would have come back as `Vec::new()` already, but we
            // belt-and-braces it here).
            let mut p = payload;
            p.stable_state.save_schema_version = 1;
            // Ensure memories is non-null (already a Vec; nothing to do).
            Ok(p)
        }
        // Future migration steps land here. For now any other source version
        // is treated as a future-major (we don't have a migrator for it).
        _ => Err(SaveError::UnsupportedVersion {
            found: from + 1,
            supported: SAVE_SCHEMA_VERSION,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_payload() -> SavePayload {
        use std::collections::{BTreeMap, BTreeSet, VecDeque};
        use story_model::{CharacterState, GameState, LocationId, TimeSlot};
        let state = GameState {
            save_schema_version: 0,
            story_id: "rainy-reunion".to_string(),
            story_version: semver::Version::new(0, 1, 0),
            pack_schema_version: semver::Version::new(1, 0, 0),
            session_id: uuid::Uuid::nil(),
            current_chapter: "chapter-1".to_string(),
            current_scene_id: "cafe-namu".to_string(),
            turn: 0,
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
        };
        let _ = CharacterState {
            affection: 0,
            trust: 0,
            tension: 0,
            relationship_stage: story_model::RelationshipStage::Acquaintance,
            current_emotion: story_model::Emotion::Neutral,
            known_facts: BTreeSet::new(),
            address_form_overrides: BTreeMap::new(),
        };
        SavePayload {
            stable_state: state,
            memories: Vec::new(),
            history: Vec::new(),
            audit_tail: Vec::new(),
        }
    }

    #[test]
    fn migrate_v0_to_v1_bumps_version() {
        let p = empty_payload();
        let migrated = migrate_payload(p, 0, 1).unwrap();
        assert_eq!(migrated.stable_state.save_schema_version, 1);
    }

    #[test]
    fn migrate_same_version_is_noop() {
        let p = empty_payload();
        let migrated = migrate_payload(p.clone(), 1, 1).unwrap();
        assert_eq!(
            migrated.stable_state.save_schema_version,
            p.stable_state.save_schema_version
        );
    }

    #[test]
    fn migrate_rejects_future_major() {
        let p = empty_payload();
        match migrate_payload(p, 2, 1) {
            Err(SaveError::UnsupportedVersion { found, supported }) => {
                assert_eq!(found, 2);
                assert_eq!(supported, 1);
            }
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
    }
}
