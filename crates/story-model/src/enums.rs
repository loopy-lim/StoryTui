//! Core enumerated vocabularies shared across the engine.
//!
//! Every enum derives `Copy` (they are small discriminant sets) and serializes
//! using `#[serde(rename_all = "snake_case")]` so the on-disk / over-the-wire
//! representation is stable and human-readable
//! (e.g. `RelationshipStage::CloseFriend` <-> `"close_friend"`).

use serde::{Deserialize, Serialize};

/// Discrete stages of an NPC relationship, ordered loosely from distant to
/// intimate. Stored on [`crate::CharacterState`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipStage {
    Stranger,
    Acquaintance,
    Friend,
    CloseFriend,
    RomanticInterest,
    Partner,
    Estranged,
}

/// Dominant emotional register of a character for the current turn.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Emotion {
    Neutral,
    Happy,
    Sad,
    Angry,
    Anxious,
    Surprised,
    Affectionate,
    Cold,
    Embarrassed,
    Guilty,
}

/// Coarse time-of-day bucket driving scene tone and pacing.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeSlot {
    Dawn,
    Morning,
    Afternoon,
    Evening,
    Night,
}

/// Risk classification attached to a [`crate::Choice`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    #[default]
    Safe,
    Low,
    Medium,
    High,
    Severe,
}

/// Binary comparison operator used by [`crate::ConditionExpr::NumberCompare`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompareOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Coarse classification of a player-initiated action, captured in
/// [`crate::ParsedAction`] and history entries.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionType {
    Choose,
    Dialogue,
    Action,
    Move,
    GeneralAction,
}

/// Kind tag for a [`crate::MemoryEntry`], used by recall/retrieval logic.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Promise,
    SecretReveal,
    RelationshipChange,
    KeyItem,
    FirstMeeting,
    Confession,
    Betrayal,
    Other,
}
