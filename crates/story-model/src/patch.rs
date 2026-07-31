//! The model-output patch contract.
//!
//! [`StatePatch`] is the strict, closed set of mutations a generated turn
//! proposes/applies. It carries `#[serde(deny_unknown_fields)]` because it is
//! the contract the model emits: an unknown key is a hard error rather than a
//! silently-ignored mistake. Validation/semantics live in W3; this module is
//! data + serde only.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{CharacterId, LocationId, RelationshipStage, TimeSlot};

/// A closed set of mutations to apply to [`crate::GameState`].
///
/// All maps are keyed by the relevant id; delta fields are additive. Two
/// sentinel fields (`next_location`, `next_time_slot`) carry replacement
/// values rather than deltas. `proposed_route` switches the active route.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StatePatch {
    /// Additive affection change per character.
    pub affection_delta: BTreeMap<CharacterId, i16>,
    /// Additive trust change per character.
    pub trust_delta: BTreeMap<CharacterId, i16>,
    /// Additive tension change per character.
    pub tension_delta: BTreeMap<CharacterId, i16>,
    /// Forced relationship-stage transition per character.
    pub relationship_transitions: BTreeMap<CharacterId, RelationshipStage>,
    /// Flag assignments (set True/False).
    pub set_flags: BTreeMap<String, bool>,
    /// Additive number changes.
    pub number_delta: BTreeMap<String, i64>,
    /// String assignments (replace).
    pub set_strings: BTreeMap<String, String>,
    /// Inventory quantity deltas (may be negative).
    pub inventory_delta: BTreeMap<String, i32>,
    /// Events to mark as occurred.
    pub add_events: BTreeSet<String>,
    /// If set, the new location after the turn.
    pub next_location: Option<LocationId>,
    /// If set, the new time-of-day after the turn.
    pub next_time_slot: Option<TimeSlot>,
    /// If set, switch the active route.
    pub proposed_route: Option<String>,
}

impl StatePatch {
    /// Returns `true` when the patch carries no mutations whatsoever.
    ///
    /// Useful for generated turns that change nothing other than narration.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::StatePatch;

    #[test]
    fn default_is_empty() {
        assert!(StatePatch::default().is_empty());
    }
}
