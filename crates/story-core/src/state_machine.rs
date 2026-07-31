//! Route, relationship, and ending state machines (ST-010).
//!
//! This module is the *only* path by which `active_route`, character
//! `relationship_stage`, and the computed `ending` change.
//!
//! - [`confirm_route`] accepts a `proposed_route` only when its
//!   `enter_condition` evaluates `true`; an unmet or unknown proposal leaves
//!   the existing route untouched.
//! - [`resolve_relationship_transition`] enforces single-step transitions on
//!   the order `Stranger → Acquaintance → Friend → CloseFriend →
//!   RomanticInterest → Partner`, with `Estranged` reachable from any stage.
//!   A skip beyond one step requires an applicable fired event with
//!   `allow_relationship_skip`.
//! - [`resolve_ending`] evaluates every `EndingDef::condition` after events
//!   fire and returns the lowest-id match. **The patch is never consulted for
//!   the ending** — endings are derived purely from state + rules.

use story_model::{EndingDef, GameState, RelationshipStage, StoryRules};

use crate::error::{CoreError, PATCH_RELATIONSHIP_SKIP};
use crate::eval::EvalContext;
use crate::eval::evaluate;

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

/// Returns `true` if the move `current → target` is always-permitted
/// regardless of skip grants (Estranged case or no-op).
fn always_permitted(current: RelationshipStage, target: RelationshipStage) -> bool {
    target == RelationshipStage::Estranged
        || current == RelationshipStage::Estranged
        || current == target
}

/// Validator-side pre-check: would the transition `current → target` be
/// permitted given that some event in the patch's `add_events` carries
/// `allow_relationship_skip` (`any_skip_candidate == true`)?
///
/// Returns [`CoreError::InvalidPatch`] with code [`PATCH_RELATIONSHIP_SKIP`] on
/// rejection so callers see the validator-stage reason code.
pub fn would_permit_relationship_transition(
    current: RelationshipStage,
    target: RelationshipStage,
    any_skip_candidate: bool,
) -> Result<(), CoreError> {
    if always_permitted(current, target) {
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

/// Authoritative reducer-side transition resolver.
///
/// Returns the resolved [`RelationshipStage`] or
/// [`CoreError::ForbiddenTransition`] when the move is not permitted. The
/// `event_skip_allowed` flag must be `true` only when an applicable event with
/// `allow_relationship_skip` actually fired during this turn.
pub fn resolve_relationship_transition(
    current: RelationshipStage,
    target: RelationshipStage,
    event_skip_allowed: bool,
) -> Result<RelationshipStage, CoreError> {
    if always_permitted(current, target) {
        return Ok(target);
    }
    let diff = step_index(target).abs_diff(step_index(current));
    if diff <= 1 || event_skip_allowed {
        Ok(target)
    } else {
        Err(CoreError::ForbiddenTransition(format!(
            "relationship transition {current:?} -> {target:?} skips {diff} stages (max 1 \
             without allow_relationship_skip)"
        )))
    }
}

/// Returns the route id the engine should consider active after a turn.
///
/// The proposed route is accepted **only** when its `enter_condition` evaluates
/// `true` against the post-events state. When the proposal is unmet (or
/// absent), the previously active route is retained unchanged.
pub fn confirm_route(
    state: &GameState,
    rules: &StoryRules,
    proposed: &Option<String>,
) -> Option<String> {
    if let Some(proposed_id) = proposed
        && let Some(def) = rules.routes.get(proposed_id)
    {
        let ctx = EvalContext { state, rules };
        if evaluate(&def.enter_condition, &ctx) {
            return Some(proposed_id.clone());
        }
        // Unmet proposal: keep the existing route.
    }
    state.active_route.clone()
}

/// Compute the ending that should trigger given the post-events state.
///
/// Evaluates every [`EndingDef::condition`]; if exactly one matches, that is
/// the ending. If multiple match, the lowest id (stable sort) wins and the
/// caller is expected to warn. If none match, returns `None`.
///
/// **The patch is never consulted** — endings are derived purely from state +
/// rules.
pub fn resolve_ending(state: &GameState, rules: &StoryRules) -> Option<String> {
    let ctx = EvalContext { state, rules };
    let mut matching: Vec<&EndingDef> = rules
        .endings
        .values()
        .filter(|e| evaluate(&e.condition, &ctx))
        .collect();
    // Stable sort by id for deterministic selection.
    matching.sort_by_key(|e| e.id.clone());
    matching.first().map(|e| e.id.clone())
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
    fn resolve_allows_single_step_blocks_skips_without_permission() {
        // Friend (2) -> CloseFriend (3): one step, ok.
        assert_eq!(
            resolve_relationship_transition(
                RelationshipStage::Friend,
                RelationshipStage::CloseFriend,
                false
            )
            .unwrap(),
            RelationshipStage::CloseFriend
        );
        // Friend (2) -> RomanticInterest (4): two steps, denied without skip.
        assert!(
            resolve_relationship_transition(
                RelationshipStage::Friend,
                RelationshipStage::RomanticInterest,
                false
            )
            .is_err()
        );
        // Same transition permitted with event skip.
        assert_eq!(
            resolve_relationship_transition(
                RelationshipStage::Friend,
                RelationshipStage::RomanticInterest,
                true
            )
            .unwrap(),
            RelationshipStage::RomanticInterest
        );
    }

    #[test]
    fn estranged_is_reachable_from_any_stage() {
        for from in [
            RelationshipStage::Stranger,
            RelationshipStage::Acquaintance,
            RelationshipStage::Friend,
            RelationshipStage::CloseFriend,
            RelationshipStage::RomanticInterest,
            RelationshipStage::Partner,
            RelationshipStage::Estranged,
        ] {
            assert_eq!(
                resolve_relationship_transition(from, RelationshipStage::Estranged, false).unwrap(),
                RelationshipStage::Estranged,
                "Estranged must be reachable from {from:?}"
            );
        }
    }

    #[test]
    fn would_permit_allows_single_step_blocks_skips() {
        would_permit_relationship_transition(
            RelationshipStage::Friend,
            RelationshipStage::CloseFriend,
            false,
        )
        .unwrap();
        would_permit_relationship_transition(
            RelationshipStage::Friend,
            RelationshipStage::RomanticInterest,
            false,
        )
        .unwrap_err();
        would_permit_relationship_transition(
            RelationshipStage::Friend,
            RelationshipStage::RomanticInterest,
            true,
        )
        .unwrap();
    }
}
