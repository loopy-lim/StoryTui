//! Domain model for StoryTUI: story, scene, choice, memory, state, and metadata
//! value types. Pure data with `serde` (de)serialization and no behavior that
//! touches the terminal, network, or filesystem. This is the shared vocabulary
//! every other crate speaks; it must stay free of UI, HTTP, and IO dependencies
//! (ADR-003).
//!
//! # Serde / version policy
//!
//! - Newtype IDs are `#[serde(transparent)]`: `"c-namu"` round-trips, not
//!   `{"0":"c-namu"}`.
//! - Enums use `#[serde(rename_all = "snake_case")]`:
//!   `RelationshipStage::CloseFriend` <-> `"close_friend"`.
//! - [`StatePatch`] is `#[serde(deny_unknown_fields)]` because it is the model
//!   output contract — unknown keys are a hard error.
//! - [`GameState`] is forward-tolerant: it carries
//!   [`SAVE_SCHEMA_VERSION`] and intentionally does *not* deny unknown fields,
//!   so future optional fields do not break loading of older saves.
//! - [`ConditionExpr`] is internally tagged via `#[serde(tag = "kind")]`.

#![warn(clippy::all)]

pub mod choice;
pub mod condition;
pub mod enums;
pub mod id;
pub mod state;

/// Schema version this build of `story-model` writes and understands.
///
/// Bumped on every incompatible change to [`crate::GameState`] or its nested
/// types. Forward-tolerance covers additive optional changes; a bumped version
/// signals a migration is required.
pub const SAVE_SCHEMA_VERSION: u32 = 1;

pub use choice::Choice;
pub use condition::ConditionExpr;
pub use enums::{
    ActionType, CompareOp, Emotion, MemoryKind, RelationshipStage, RiskLevel, TimeSlot,
};
pub use id::{CharacterId, EventId, FlagId, ItemId, LocationId};
pub use state::{CharacterState, ChoiceRef};
