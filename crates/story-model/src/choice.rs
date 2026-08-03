//! The player-facing [`Choice`] value type.

use serde::{Deserialize, Serialize};

use crate::{CharacterId, ConditionExpr, RiskLevel};

/// A single selectable choice offered to the player in a generated turn.
///
/// `hidden_requirements` is a conjunction: the choice is visible/available only
/// when every condition holds. Evaluation lives in W3.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    /// Pack-stable id of this choice within its turn.
    pub id: String,
    /// Short human-readable label shown in the choice list.
    pub label: String,
    /// Free-form description of the player's intent (drives generation).
    #[serde(default)]
    pub intent: String,
    /// Optional tone hint (e.g. "playful", "cold").
    pub tone: Option<String>,
    /// Character this choice is directed at, if any.
    pub target: Option<CharacterId>,
    /// Risk classification surfaced to the player.
    #[serde(default)]
    pub risk: RiskLevel,
    /// Conditions that must hold for the choice to be selectable.
    #[serde(default)]
    pub hidden_requirements: Vec<ConditionExpr>,
    /// Optional spoiler-free preview text shown before selecting.
    pub preview: Option<String>,
}
