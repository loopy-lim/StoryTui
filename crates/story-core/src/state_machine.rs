//! Route, relationship, and ending state machines (ST-010).
//!
//! This module is the *only* path by which `active_route`, character
//! `relationship_stage`, and the computed `ending` change. It is populated
//! incrementally: W3.3 exposes [`step_index`] and [`would_permit_relationship_transition`]
//! (needed by the validator's relationship-skip check); W3.4 adds the full
//! route/relationship/ending state-machine entry points used by the reducer.

use story_model::RelationshipStage;

use crate::error::{CoreError, PATCH_RELATIONSHIP_SKIP};

/// Order of progression along the relationship track. `Estranged` is special:
/// it is reachable from any other stage (and from itself), but is never a
/// "step" along the main progression, so it returns a sentinel `-1`.
pub fn step_index(stage: RelationshipStage) -> i8 {
    match stage {
        RelationshipStage::Stranger => 0,
        RelationshipStage::Acquaintance => 1,
        RelationshipStage::Friend => 2,
        RelationshipStage::CloseFriend => 3,
        RelationshipStage::RomanticInterest => 4,
        RelationshipStage::Partner => 5,
        RelationshipStage::Estranged => -1,
    }
}

/// Validator-side pre-check: would the transition `current → target` be
/// permitted given that some event in the patch's `add_events` carries
/// `allow_relationship_skip` (`any_skip_candidate == true`)?
///
/// Rules mirror [`crate::reducer`] application: transitions to `Estranged`,
/// from `Estranged`, or no-op (same stage) are always permitted; otherwise the
/// absolute step difference must be ≤ 1, or the skip must be granted by a
/// candidate event.
pub fn would_permit_relationship_transition(
    current: RelationshipStage,
    target: RelationshipStage,
    any_skip_candidate: bool,
) -> Result<(), CoreError> {
    if target == RelationshipStage::Estranged
        || current == RelationshipStage::Estranged
        || current == target
    {
        return Ok(());
    }
    let diff = step_index(target).abs_diff(step_index(current));
    if diff <= 1 || any_skip_candidate {
        Ok(())
    } else {
        Err(CoreError::invalid_patch(
            PATCH_RELATIONSHIP_SKIP,
            format!(
                "relationship transition {current:?} -> {target:?} skips {diff} stages; no event \
                 with allow_relationship_skip is present in add_events"
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_index_orders_progression() {
        assert!(
            step_index(RelationshipStage::Stranger) < step_index(RelationshipStage::Acquaintance)
        );
        assert!(
            step_index(RelationshipStage::Acquaintance) < step_index(RelationshipStage::Friend)
        );
        assert!(step_index(RelationshipStage::Friend) < step_index(RelationshipStage::CloseFriend));
        assert!(
            step_index(RelationshipStage::CloseFriend)
                < step_index(RelationshipStage::RomanticInterest)
        );
        assert!(
            step_index(RelationshipStage::RomanticInterest)
                < step_index(RelationshipStage::Partner)
        );
        assert_eq!(step_index(RelationshipStage::Estranged), -1);
    }

    #[test]
    fn would_permit_allows_single_step_blocks_skips() {
        // Friend (2) -> CloseFriend (3): one step, ok.
        would_permit_relationship_transition(
            RelationshipStage::Friend,
            RelationshipStage::CloseFriend,
            false,
        )
        .unwrap();
        // Friend (2) -> RomanticInterest (4): two steps, denied without skip.
        would_permit_relationship_transition(
            RelationshipStage::Friend,
            RelationshipStage::RomanticInterest,
            false,
        )
        .unwrap_err();
        // Same, permitted with event skip candidate.
        would_permit_relationship_transition(
            RelationshipStage::Friend,
            RelationshipStage::RomanticInterest,
            true,
        )
        .unwrap();
    }

    #[test]
    fn would_permit_estranged_always_allowed() {
        for from in [
            RelationshipStage::Stranger,
            RelationshipStage::Friend,
            RelationshipStage::Partner,
        ] {
            would_permit_relationship_transition(from, RelationshipStage::Estranged, false)
                .unwrap();
        }
    }
}
