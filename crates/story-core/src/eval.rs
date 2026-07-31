//! Evaluation of the [`ConditionExpr`] DSL against a [`GameState`] + [`StoryRules`]
//! (ST-009).
//!
//! Evaluation is total and deterministic: every variant reduces to a `bool`.
//! Missing values have defensive defaults (absent flag = `false`, missing
//! number variable = the rule default then `0`, missing character = affection
//! `0` / trust `0`) so that evaluation never panics on a slightly-out-of-sync
//! state. Pack-time validation (W4) is expected to catch unknown ids earlier.

use story_model::{CharacterId, CompareOp, ConditionExpr, GameState, LocationId, StoryRules};

/// Immutable view used to evaluate a [`ConditionExpr`].
pub struct EvalContext<'a> {
    /// The state to read fields from.
    pub state: &'a GameState,
    /// The rules used to resolve defaults (e.g. number-var defaults).
    pub rules: &'a StoryRules,
}

/// Evaluate `cond` against `ctx`, returning a `bool`.
pub fn evaluate(cond: &ConditionExpr, ctx: &EvalContext<'_>) -> bool {
    match cond {
        ConditionExpr::All { children } => children.iter().all(|c| evaluate(c, ctx)),
        ConditionExpr::Any { children } => children.iter().any(|c| evaluate(c, ctx)),
        ConditionExpr::Not { child } => !evaluate(child, ctx),
        ConditionExpr::FlagEquals { id, value } => {
            let actual = ctx.state.flags.get(id).copied().unwrap_or(false);
            actual == *value
        }
        ConditionExpr::NumberCompare { id, op, value } => {
            let lhs = ctx
                .state
                .numbers
                .get(id)
                .copied()
                .or_else(|| ctx.rules.numbers.get(id).map(|d| d.default))
                .unwrap_or(0);
            apply_compare(*op, lhs, *value)
        }
        ConditionExpr::AffectionAtLeast { character, value } => {
            let actual = affection_of(ctx.state, character);
            actual >= *value
        }
        ConditionExpr::TrustAtLeast { character, value } => {
            let actual = trust_of(ctx.state, character);
            actual >= *value
        }
        ConditionExpr::EventOccurred { id } => ctx.state.occurred_events.contains(id),
        ConditionExpr::AtLocation { location } => at_location(ctx.state, location),
        ConditionExpr::TurnAtLeast { value } => ctx.state.turn >= *value,
    }
}

fn apply_compare(op: CompareOp, lhs: i64, rhs: i64) -> bool {
    match op {
        CompareOp::Eq => lhs == rhs,
        CompareOp::Ne => lhs != rhs,
        CompareOp::Lt => lhs < rhs,
        CompareOp::Le => lhs <= rhs,
        CompareOp::Gt => lhs > rhs,
        CompareOp::Ge => lhs >= rhs,
    }
}

fn affection_of(state: &GameState, character: &CharacterId) -> i16 {
    state
        .characters
        .get(character)
        .map(|c| c.affection)
        .unwrap_or(0)
}

fn trust_of(state: &GameState, character: &CharacterId) -> i16 {
    state
        .characters
        .get(character)
        .map(|c| c.trust)
        .unwrap_or(0)
}

fn at_location(state: &GameState, location: &LocationId) -> bool {
    state.location == *location
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_compare_truth_table() {
        assert!(apply_compare(CompareOp::Eq, 5, 5));
        assert!(!apply_compare(CompareOp::Eq, 5, 6));
        assert!(apply_compare(CompareOp::Ne, 5, 6));
        assert!(apply_compare(CompareOp::Lt, 5, 6));
        assert!(apply_compare(CompareOp::Le, 5, 5));
        assert!(apply_compare(CompareOp::Gt, 6, 5));
        assert!(apply_compare(CompareOp::Ge, 5, 5));
    }
}
