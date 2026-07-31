#![warn(clippy::all)]
//! Persistence layer for StoryTUI: story-pack loading, content-addressed
//! caching (`sha2`), atomic save-file writes, schema migrations keyed by
//! `semver`, and the JSON save store. All filesystem interaction is isolated
//! here so the deterministic core never touches disk (ADR-003).

pub mod atomic_write;
pub mod checksum;
pub mod error;
pub mod json_store;
pub mod migration;
pub mod pack;

pub use atomic_write::atomic_write_json;
pub use checksum::{CHECKSUM_ALGORITHM, payload_checksum, verify};
pub use error::{PackError, SAVE_SCHEMA_VERSION, SaveError};
pub use json_store::{DEFAULT_BAK_GENERATIONS, SaveStore, SlotMeta};
pub use migration::migrate_payload;
pub use pack::PackLoader;
