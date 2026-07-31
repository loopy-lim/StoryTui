//! Audit trail of every proposed change to a [`crate::GameState`].
//!
//! Each [`StateChangeLog`] entry captures a single field-level decision: what
//! the value was before, what was proposed, and what (if anything) was
//! applied. The reducer populates these on both the rejected (validator) and
//! applied (reducer commit) paths.
//!
//! # Hard rule
//!
//! **Narration text MUST NEVER appear in a [`StateChangeLog`] entry.** The
//! change log describes machine-readable state mutations only — narration lives
//! on [`crate::GeneratedTurn`] and [`crate::HistoryEntry`].

use serde::{Deserialize, Serialize};

/// Originator of a proposed state change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeSource {
    /// A patch emitted by the generation LLM.
    LlmPatch,
    /// An event rule firing.
    EventRule,
    /// The deterministic reducer itself.
    Reducer,
    /// A schema migration.
    Migration,
    /// A direct player action.
    PlayerAction,
}

/// Outcome of evaluating a single proposed change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeDecision {
    /// The proposed value was applied verbatim.
    Applied,
    /// The proposed value was rejected outright.
    Rejected,
    /// The proposed value exceeded limits and was clamped before application.
    Clamped,
    /// The proposed value was deferred (e.g. cosmetic issue).
    Deferred,
}

/// A single audited field-level change against a [`crate::GameState`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StateChangeLog {
    /// Unique id of this log entry.
    pub id: uuid::Uuid,
    /// Generation this entry belongs to.
    pub generation_id: uuid::Uuid,
    /// Turn at which the change was proposed.
    pub turn: u32,
    /// Who proposed the change.
    pub source: ChangeSource,
    /// Dotted path of the affected field (e.g. `characters.c-namu.affection`).
    pub field_path: String,
    /// Serialized value before the change.
    pub before: serde_json::Value,
    /// Value originally proposed.
    pub proposed: serde_json::Value,
    /// Value that was actually applied, if any.
    pub applied: Option<serde_json::Value>,
    /// Decision reached for this proposal.
    pub decision: ChangeDecision,
    /// Rule id that drove the decision, if any.
    pub rule_id: Option<String>,
    /// Stable machine-readable reason code (e.g. `patch_delta_out_of_range`).
    pub reason_code: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn change_source_serializes_snake_case() {
        let s = serde_json::to_string(&ChangeSource::LlmPatch).unwrap();
        assert_eq!(s, "\"llm_patch\"");
        let s = serde_json::to_string(&ChangeSource::EventRule).unwrap();
        assert_eq!(s, "\"event_rule\"");
    }

    #[test]
    fn change_decision_serializes_snake_case() {
        let s = serde_json::to_string(&ChangeDecision::Applied).unwrap();
        assert_eq!(s, "\"applied\"");
        let s = serde_json::to_string(&ChangeDecision::Rejected).unwrap();
        assert_eq!(s, "\"rejected\"");
    }
}
