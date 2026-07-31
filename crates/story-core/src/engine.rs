//! Top-level engine orchestration.
//!
//! [`commit_turn`] is the high-level "advance the game by one turn" entry
//! point: it validates a generated patch, atomically applies it, fires any
//! resulting events, resolves the route/relationship/ending state machines,
//! and returns the new state plus an audit trail.
//!
//! This module intentionally exposes only the patch-commit primitive. The
//! full `EngineCommand` / `Transition` / `EngineEffect` boundary (parsing,
//! memory selection, prompt context, provider I/O) is W5; here we keep the
//! surface minimal so the deterministic core can be tested with zero LLM
//! input.

use story_model::uuid::Uuid;
use story_model::{GameState, StatePatch, StoryRules};

use crate::error::CoreError;
use crate::reducer::{ApplyResult, apply_patch};

/// Validate + atomically apply a generated patch for one turn.
///
/// Delegates to [`apply_patch`]; bumped `revision` lives on the returned
/// [`ApplyResult::state`]. The `turn` field is **not** advanced here — the
/// caller (engine layer / W5) is responsible for tick semantics.
pub fn commit_turn(
    state: &GameState,
    rules: &StoryRules,
    patch: &StatePatch,
    generation_id: Uuid,
) -> Result<ApplyResult, CoreError> {
    apply_patch(state, rules, patch, generation_id)
}
