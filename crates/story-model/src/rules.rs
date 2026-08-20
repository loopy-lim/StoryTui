//! Canonical rule structures consumed by the `story-core` engine.
//!
//! These types are the *input contract* for the deterministic engine: a W4
//! story-pack loader will later produce a [`StoryRules`] instance from
//! `rules.toml`. Everything here is pure data with a stable serde shape — no
//! behavior that touches the terminal, network, or filesystem (ADR-003).
//!
//! # Serde / version policy
//!
//! - Struct fields serialize as snake_case (serde default).
//! - Enums use `#[serde(rename_all = "snake_case")]`.
//! - [`StoryRules`] does **not** carry `deny_unknown_fields`: the W4 loader is
//!   allowed to round-trip exploratory fields it does not yet understand.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{CharacterId, ConditionExpr, EventId, LocationId, RelationshipStage};

// ---------------------------------------------------------------------------
// Defaults & range helper
// ---------------------------------------------------------------------------

/// Default lower bound for a single-turn affection/trust/tension delta.
pub const DEFAULT_DELTA_MIN: i16 = -3;
/// Default upper bound for a single-turn affection/trust/tension delta.
pub const DEFAULT_DELTA_MAX: i16 = 3;
/// Default lower bound for a final affection/trust/tension score.
pub const DEFAULT_STAT_MIN: i16 = -100;
/// Default upper bound for a final affection/trust/tension score.
pub const DEFAULT_STAT_MAX: i16 = 100;

/// Inclusive `i16` range used by delta/stat limit configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Int16Range {
    /// Inclusive minimum.
    pub min: i16,
    /// Inclusive maximum.
    pub max: i16,
}

impl Int16Range {
    /// Returns `true` when `v` lies within `[min, max]`.
    pub fn contains(&self, v: i16) -> bool {
        v >= self.min && v <= self.max
    }
}

impl Default for Int16Range {
    fn default() -> Self {
        Self {
            min: DEFAULT_DELTA_MIN,
            max: DEFAULT_DELTA_MAX,
        }
    }
}

/// Per-axis delta limits applied by the validator to single-turn deltas.
///
/// Defaults to `(-3..=3)` on every axis (see [`Int16Range::default`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DeltaLimits {
    /// Affection delta range.
    pub affection: Int16Range,
    /// Trust delta range.
    pub trust: Int16Range,
    /// Tension delta range.
    pub tension: Int16Range,
}

// ---------------------------------------------------------------------------
// Rule structs
// ---------------------------------------------------------------------------

/// Top-level rule set consumed by the engine. Produced from a story pack by
/// the W4 loader; hand-built in `story-core` unit tests.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct StoryRules {
    /// Known locations and their adjacency.
    pub locations: BTreeMap<LocationId, LocationDef>,
    /// Allow-list of flag ids the engine may set/unset.
    pub allowed_flags: BTreeSet<String>,
    /// Numeric variable definitions.
    pub numbers: BTreeMap<String, NumberVarDef>,
    /// String variable definitions.
    pub strings: BTreeMap<String, StringVarDef>,
    /// Allow-list of item ids the engine may add/remove.
    pub allowed_items: BTreeSet<String>,
    /// Defined events that may fire during patch application.
    pub events: BTreeMap<EventId, EventDef>,
    /// Defined routes the model may propose.
    pub routes: BTreeMap<String, RouteDef>,
    /// Defined endings evaluated after events fire.
    pub endings: BTreeMap<String, EndingDef>,
    /// Per-axis single-turn delta limits.
    pub delta_limits: DeltaLimits,
    /// Final affection/trust/tension bounds (default `-100..=100`).
    pub stat_limits: Int16Range,
    /// Optional per-axis final bounds. Older canonical data falls back to the
    /// shared `stat_limits` field for backward compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stat_limits_by_axis: Option<DeltaLimits>,
    /// Default number of choices offered per turn (MVP = 3).
    pub default_choice_count: u8,
}

impl StoryRules {
    /// A minimal but valid rule set: empty maps, default limits, three choices.
    pub fn minimal() -> Self {
        Self {
            delta_limits: DeltaLimits::default(),
            stat_limits: Int16Range {
                min: DEFAULT_STAT_MIN,
                max: DEFAULT_STAT_MAX,
            },
            stat_limits_by_axis: None,
            default_choice_count: 3,
            ..Self::default()
        }
    }

    pub fn affection_stat_limits(&self) -> &Int16Range {
        self.stat_limits_by_axis
            .as_ref()
            .map(|limits| &limits.affection)
            .unwrap_or(&self.stat_limits)
    }

    pub fn trust_stat_limits(&self) -> &Int16Range {
        self.stat_limits_by_axis
            .as_ref()
            .map(|limits| &limits.trust)
            .unwrap_or(&self.stat_limits)
    }

    pub fn tension_stat_limits(&self) -> &Int16Range {
        self.stat_limits_by_axis
            .as_ref()
            .map(|limits| &limits.tension)
            .unwrap_or(&self.stat_limits)
    }
}

/// A location and the locations the player may move to from it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocationDef {
    /// Identifier of this location (matches the map key).
    pub id: LocationId,
    /// Display name shown to the player.
    pub display_name: String,
    /// Locations reachable from this one in a single turn.
    pub adjacent: BTreeSet<LocationId>,
}

/// Definition of a numeric counter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NumberVarDef {
    /// Identifier of this variable.
    pub id: String,
    /// Inclusive minimum value.
    pub min: i64,
    /// Inclusive maximum value.
    pub max: i64,
    /// Value used when the variable is absent in [`crate::GameState::numbers`].
    pub default: i64,
}

/// Definition of a string variable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StringVarDef {
    /// Identifier of this variable.
    pub id: String,
    /// Maximum string length.
    pub max_len: u32,
    /// When set, the value must be in this allow-list.
    pub allowed: Option<BTreeSet<String>>,
}

/// Definition of an event that may fire during patch application.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventDef {
    /// Identifier of this event.
    pub id: EventId,
    /// Condition under which the event fires.
    pub trigger: ConditionExpr,
    /// Higher values fire first when multiple events match.
    pub priority: u32,
    /// Effects applied atomically when the event fires.
    pub effects: EventEffects,
    /// When `true`, an applicable firing of this event permits a
    /// relationship transition that skips more than one stage.
    pub allow_relationship_skip: bool,
    /// When `Some`, the event only fires when `active_route` matches.
    pub route_lock: Option<String>,
}

/// Effects applied when an [`EventDef`] fires.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct EventEffects {
    /// Additive affection change per character.
    pub affection_delta: BTreeMap<CharacterId, i16>,
    /// Additive trust change per character.
    pub trust_delta: BTreeMap<CharacterId, i16>,
    /// Additive tension change per character.
    pub tension_delta: BTreeMap<CharacterId, i16>,
    /// Flag assignments.
    pub set_flags: BTreeMap<String, bool>,
    /// Additive number changes.
    pub number_delta: BTreeMap<String, i64>,
    /// Events to mark as occurred.
    pub add_events: BTreeSet<String>,
    /// Force the player's location after the turn.
    pub force_location: Option<LocationId>,
    /// Force a character's relationship stage.
    pub force_relationship: BTreeMap<CharacterId, RelationshipStage>,
    /// Force an ending id (still re-checked against ending conditions).
    pub ending: Option<String>,
}

/// A route the model may propose, gated by an enter condition.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RouteDef {
    /// Identifier of this route.
    pub id: String,
    /// Condition that must evaluate true for the route to be entered.
    pub enter_condition: ConditionExpr,
    /// Premise text shown when the route becomes active.
    pub premise: String,
}

/// An ending evaluated after events fire.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EndingDef {
    /// Identifier of this ending.
    pub id: String,
    /// Condition under which this ending triggers.
    pub condition: ConditionExpr,
    /// Human-readable label.
    pub label: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int16range_contains_is_inclusive() {
        let r = Int16Range { min: -3, max: 3 };
        assert!(r.contains(-3));
        assert!(r.contains(0));
        assert!(r.contains(3));
        assert!(!r.contains(-4));
        assert!(!r.contains(4));
    }

    #[test]
    fn default_range_matches_const() {
        let r = Int16Range::default();
        assert_eq!(r.min, DEFAULT_DELTA_MIN);
        assert_eq!(r.max, DEFAULT_DELTA_MAX);
    }

    #[test]
    fn minimal_rules_have_default_limits() {
        let r = StoryRules::minimal();
        assert_eq!(r.stat_limits.min, DEFAULT_STAT_MIN);
        assert_eq!(r.stat_limits.max, DEFAULT_STAT_MAX);
        assert_eq!(r.default_choice_count, 3);
    }

    #[test]
    fn event_effects_default_has_empty_typed_deltas() {
        let e = EventEffects::default();
        assert!(e.affection_delta.is_empty());
        assert!(e.trust_delta.is_empty());
        assert!(e.tension_delta.is_empty());
    }

    #[test]
    fn event_effects_round_trips_typed_deltas() {
        let mut e = EventEffects::default();
        e.affection_delta.insert(CharacterId("c-namu".into()), 2);
        e.trust_delta.insert(CharacterId("c-namu".into()), 3);
        e.tension_delta.insert(CharacterId("c-namu".into()), -1);
        let json = serde_json::to_string(&e).expect("serialize");
        let back: EventEffects = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(e, back);
        // Trust/tension serialize as their own typed keys — NOT under number_delta.
        assert!(
            json.contains("\"trust_delta\""),
            "trust_delta key missing from JSON: {json}"
        );
        assert!(
            json.contains("\"tension_delta\""),
            "tension_delta key missing from JSON: {json}"
        );
    }
}
