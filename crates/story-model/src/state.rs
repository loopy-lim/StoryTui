//! Persistent story state.
//!
//! [`CharacterState`] is the per-character relationship/emotion snapshot.
//! [`ChoiceRef`] is a compact pointer to the last choice the player made.
//! [`GameState`] is the top-level save-state keystone.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::{
    CharacterId, Emotion, HistoryEntry, LocationId, MemoryEntry, RelationshipStage, TimeSlot,
};

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

/// The full, serializable game state — the save-file root.
///
/// This struct is intentionally forward-tolerant: it does **not** carry
/// `#[serde(deny_unknown_fields)]`, so future optional fields do not break
/// loading of older saves. The `save_schema_version` field records which schema
/// revision a given save was written with; loaders compare it against
/// [`crate::SAVE_SCHEMA_VERSION`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GameState {
    /// Schema revision this save was written with.
    pub save_schema_version: u32,
    /// Pack id this save belongs to.
    pub story_id: String,
    /// Semantic version of the story pack this save was created with.
    pub story_version: semver::Version,
    /// Schema version of the story pack format (independent of save schema).
    pub pack_schema_version: semver::Version,
    /// Unique id of this play session.
    pub session_id: uuid::Uuid,
    /// Current chapter id within the pack.
    pub current_chapter: String,
    /// Current scene id within the chapter.
    pub current_scene_id: String,
    /// Monotonic turn counter.
    pub turn: u32,
    /// Current location.
    pub location: LocationId,
    /// Current time-of-day bucket.
    pub time_slot: TimeSlot,
    /// Active route name, if any.
    pub active_route: Option<String>,
    /// Per-character state.
    pub characters: BTreeMap<CharacterId, CharacterState>,
    /// Boolean flag store.
    pub flags: BTreeMap<String, bool>,
    /// Numeric counter store.
    pub numbers: BTreeMap<String, i64>,
    /// String store.
    pub strings: BTreeMap<String, String>,
    /// Inventory item quantities.
    pub inventory: BTreeMap<String, u32>,
    /// Events that have occurred at least once.
    pub occurred_events: BTreeSet<String>,
    /// Sliding window of recent turn summaries (oldest first).
    pub recent_summaries: VecDeque<String>,
    /// Long-term memories built up over the session.
    pub memories: Vec<MemoryEntry>,
    /// Full per-turn history.
    pub history: Vec<HistoryEntry>,
    /// The most recent choice the player made, if any.
    pub last_choice: Option<ChoiceRef>,
    /// Monotonic revision counter, bumped on every applied mutation.
    pub revision: u64,
    /// Generation id of the last stable/accepted generation, if any.
    pub stable_generation_id: Option<uuid::Uuid>,
    /// Lorebook activation ledger — drives sticky/cooldown timers. Absent in
    /// older saves (serde default keeps them loadable).
    #[serde(default)]
    pub lorebook_activations: Vec<crate::LorebookActivation>,
}
