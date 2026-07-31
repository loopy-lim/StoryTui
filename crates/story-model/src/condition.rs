//! Condition DSL used by story packs and choices to gate visibility and
//! outcomes.
//!
//! [`ConditionExpr`] is an internally-tagged enum (`#[serde(tag = "kind")]`) so
//! that JSON is self-describing and stable across schema additions:
//!
//! ```json
//! { "kind": "all", "children": [ ... ] }
//! { "kind": "flag_equals", "id": "met_namu", "value": true }
//! ```
//!
//! Evaluation is implemented in W3 (rules engine); this module defines only the
//! data shape and its serde contract.

use serde::{Deserialize, Serialize};

use crate::{CharacterId, CompareOp, LocationId};

/// Boolean expression tree over story state. Variant tags are snake_cased via
/// the `kind` tag; fields are named so the JSON is self-describing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConditionExpr {
    /// Logical conjunction of `children`.
    All { children: Vec<ConditionExpr> },
    /// Logical disjunction of `children`.
    Any { children: Vec<ConditionExpr> },
    /// Logical negation of `child`.
    Not { child: Box<ConditionExpr> },
    /// A flag equals the given boolean value.
    FlagEquals { id: String, value: bool },
    /// A number satisfies `op` against `value`.
    NumberCompare {
        id: String,
        op: CompareOp,
        value: i64,
    },
    /// A character's affection is at least `value`.
    AffectionAtLeast { character: CharacterId, value: i16 },
    /// A character's trust is at least `value`.
    TrustAtLeast { character: CharacterId, value: i16 },
    /// An event has occurred at some point.
    EventOccurred { id: String },
    /// The current location equals `location`.
    AtLocation { location: LocationId },
    /// The current turn number is at least `value`.
    TurnAtLeast { value: u32 },
}
