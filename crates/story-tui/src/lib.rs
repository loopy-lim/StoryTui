#![warn(clippy::all)]
//! Terminal UI rendering and input loop for StoryTUI, built on `ratatui` and
//! `crossterm`. Widgets, layout, Korean text shaping helpers, and the async
//! event pump. Depends on [`story-model`] and [`story-core`] only — never on the
//! provider or persistence crates directly.
