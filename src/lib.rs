#![warn(clippy::all)]
//! Terminal UI for StoryTUI: renders the engine's [`AppViewModel`] into ratatui
//! widgets and maps keys to [`EngineCommand`]s. Owns all terminal interaction
//! (`ratatui` + `crossterm`) so that [`story-core`] stays IO-free (ADR-003).

pub mod app;
pub mod event;
pub mod screens;

pub use app::App;
pub use event::{KeyAction, map_key, parse_direct_select_index, SELECT_SENTINEL};
pub use screens::story::{MIN_HEIGHT, MIN_WIDTH, render_story};
