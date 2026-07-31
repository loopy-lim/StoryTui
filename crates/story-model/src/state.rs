//! Persistent story state.
//!
//! [`CharacterState`] is the per-character relationship/emotion snapshot.
//! [`ChoiceRef`] is a compact pointer to the last choice the player made.
//! The full [`GameState`] keystone is added once its memory/history dependencies
//! exist (see W2.4).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{CharacterId, Emotion, RelationshipStage};

/// Per-character mutable state tracked by the engine.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CharacterState {
    /// Affection score, typically -100..=100.
    pub affection: i16,
    /// Trust score, typically -100..=100.
    pub trust: i16,
    /// Tension score, typically 0..=100.
    pub tension: i16,
    /// Current relationship stage bucket.
    pub relationship_stage: RelationshipStage,
    /// Current dominant emotion.
    pub current_emotion: Emotion,
    /// Facts this character knows about the world/player.
    pub known_facts: BTreeSet<String>,
    /// Per-character address-form overrides (e.g. name + honorific).
    pub address_form_overrides: BTreeMap<CharacterId, String>,
}

/// Compact reference to the most recent choice, stored on [`GameState`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChoiceRef {
    /// Id of the chosen [`crate::Choice`].
    pub choice_id: String,
    /// Label snapshot at the time of selection.
    pub label: String,
    /// Turn number at which the choice was made.
    pub turn: u32,
}
