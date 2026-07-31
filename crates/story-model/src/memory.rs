//! Memory subsystem value types: structured facts and recalled memory entries.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{CharacterId, LocationId, MemoryKind};

/// A single `(subject, predicate, object)` fact with attribution and certainty.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryFact {
    /// The fact's subject (e.g. `"player"`).
    pub subject: String,
    /// Relationship/verb (e.g. `"promised"`).
    pub predicate: String,
    /// The fact's object (e.g. `"to keep the secret"`).
    pub object: String,
    /// Characters who know this fact.
    pub known_by: BTreeSet<CharacterId>,
    /// Certainty in 0..=1.
    pub certainty: f32,
}

/// A unit of recalled context built from one or more history entries.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Unique id of this memory.
    pub id: uuid::Uuid,
    /// Categorical kind, used by recall/retrieval.
    pub kind: MemoryKind,
    /// Structured facts captured by this memory.
    pub facts: Vec<MemoryFact>,
    /// Optional natural-language interpretation.
    pub interpretation: Option<String>,
    /// Characters participating in the memory.
    pub participants: BTreeSet<CharacterId>,
    /// Where the memory was formed, if known.
    pub location: Option<LocationId>,
    /// Engine-side importance weight (higher = more salient).
    pub importance: u8,
    /// Whether this memory is still an open/unresolved thread.
    pub unresolved: bool,
    /// History entries this memory was synthesized from.
    pub source_history_ids: Vec<uuid::Uuid>,
    /// Turn number when the memory was created.
    pub created_turn: u32,
    /// Turn number when this memory was last recalled, if ever.
    pub last_recalled_turn: Option<u32>,
}
