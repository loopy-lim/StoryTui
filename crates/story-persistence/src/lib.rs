#![warn(clippy::all)]
//! Persistence layer for StoryTUI: story-pack loading, content-addressed
//! caching (`sha2`), atomic save-file writes, and schema migrations keyed by
//! `semver`. All filesystem interaction is isolated here so the deterministic
//! core never touches disk.
