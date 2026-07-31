#![warn(clippy::all)]
//! Terminal UI for StoryTUI: renders the engine's [`story_model::AppViewModel`]
//! into ratatui widgets and maps keys to [`story_core::engine_cmd::EngineCommand`]s.
//! Owns all terminal interaction (`ratatui` + `crossterm`) so that [`story-core`]
//! stays IO-free (ADR-003).

pub mod app;
pub mod event;
pub mod screens;

pub use app::App;
pub use event::{KeyAction, SELECT_SENTINEL, map_key, parse_direct_select_index};
pub use screens::story::{MIN_HEIGHT, MIN_WIDTH, render_story};
