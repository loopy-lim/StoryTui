#![warn(clippy::all)]
//! Domain model for StoryTUI: story, scene, choice, memory, state, and metadata
//! value types. Pure data with `serde` (de)serialization and no behavior that
//! touches the terminal, network, or filesystem. This is the shared vocabulary
//! every other crate speaks; it must stay free of UI, HTTP, and IO dependencies
//! (ADR-003).
