//! Persistent history of past turns.

use serde::{Deserialize, Serialize};

use crate::{Choice, ParsedAction, StatePatch};

/// A full record of a single played turn, persisted for save/replay/recall.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Unique id of this entry.
    pub id: uuid::Uuid,
    /// Turn number this entry records.
    pub turn: u32,
    /// Scene in which the turn took place.
    pub scene_id: String,
    /// Wall-clock time the entry was created.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Narration that was shown to the player.
    pub narration: String,
    /// The player's parsed action for this turn, if any.
    pub player_action: Option<ParsedAction>,
    /// Choices that were offered.
    pub choices: Vec<Choice>,
    /// Patch that was actually applied for this turn.
    pub applied_patch: StatePatch,
    /// Generation id associated with this turn's content.
    pub generation_id: uuid::Uuid,
}
