#![warn(clippy::all)]
//! Terminal UI for StoryTUI: renders the engine's [`story_model::AppViewModel`]
//! into ratatui widgets and maps keys to [`story_core::engine_cmd::EngineCommand`]s.
//! Owns all terminal interaction (`ratatui` + `crossterm`) so that [`story-core`]
//! stays IO-free (ADR-003).

pub mod app;
pub mod event;
pub mod input;
pub mod screens;
pub mod theme;

pub use app::{
    App, HomeRequest, SaveLoadMode, SaveLoadRequest, TtsDisplay, classify_emotion,
    emotion_timeline, location_timeline,
};
pub use event::{KeyAction, SELECT_SENTINEL, map_key, parse_direct_select_index};
pub use screens::save_load::{
    MIN_HEIGHT as SAVE_LOAD_MIN_HEIGHT, MIN_WIDTH as SAVE_LOAD_MIN_WIDTH, render_save_load,
};
pub use screens::story::{
    ListMode, MIN_HEIGHT, MIN_WIDTH, StoryFooterAction, StoryStackEntry, footer_action_at,
    render_list, render_story,
};
