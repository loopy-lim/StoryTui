//! AppViewModel — the pure-data view model the engine publishes and the TUI
//! renders (ST-019).
//!
//! [`AppViewModel`] is the single value that crosses the
//! `story-core` (deterministic) → `story-tui` (rendering) boundary. It is plain
//! data: no ratatui types, no behavior. The engine builds it from
//! `EngineState` + `StoryPack`; the TUI maps it into widgets. Keeping it in
//! `story-model` means neither the engine nor the TUI crate has to depend on
//! the other for the view shape, and the view stays free of UI/HTTP/FS
//! (ADR-003).
//!
//! # Serde policy
//!
//! - Struct fields serialize as snake_case (serde default).
//! - [`Screen`] uses `#[serde(rename_all = "snake_case")]`.

use serde::{Deserialize, Serialize};

use crate::uuid::Uuid;
use crate::{RiskLevel, TimeSlot};

/// The complete view the engine publishes for one render. All display strings
/// are pre-resolved (Korean allowed); the TUI only lays them out.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppViewModel {
    /// Which screen to render.
    pub screen: Screen,
    /// Story title from pack metadata.
    pub story_title: String,
    /// Current chapter id (displayed as-is; packs carry no chapter titles yet).
    pub chapter: String,
    /// Display name resolved from `rules.locations`.
    pub location_display: String,
    /// Current time-of-day bucket.
    pub time_slot: TimeSlot,
    /// Narration prose to show (latest turn's, or the opening on turn 0).
    pub narration: String,
    /// Player-facing choices offered this turn.
    pub choices: Vec<ChoiceView>,
    /// Numeric status line.
    pub status: StatusLine,
    /// Non-empty while a generation is in flight (loading overlay).
    pub loading: Option<LoadingState>,
    /// Non-empty when a recoverable error should be shown (error overlay).
    pub error: Option<DomainErrorView>,
}

/// Top-level screen identifier. Only [`Screen::Story`] is rendered in the W5
/// slice; the rest are placeholders the navigation layer (W7+) switches to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Screen {
    /// The main narration + choices view.
    Story,
    /// Free-text player input (W7).
    FreeInput,
    /// History browser.
    History,
    /// Character sheet.
    Character,
    /// Raw game-state inspector.
    GameState,
    /// Save / load manager (W8).
    SaveLoad,
    /// Settings panel.
    Settings,
    /// Detailed error view.
    ErrorDetail,
}

/// A single selectable choice in the view.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChoiceView {
    /// Pack-stable choice id (echoed back as `EngineCommand::SubmitChoice`).
    pub id: String,
    /// Short label shown in the choice list.
    pub label: String,
    /// Risk indicator surfaced to the player.
    pub risk: RiskLevel,
    /// Optional spoiler-free preview.
    pub preview: Option<String>,
}

/// Numeric status line, sourced from the primary character + game state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StatusLine {
    /// Active route id, if any.
    pub route: Option<String>,
    /// Primary character's affection, if a primary character exists.
    pub affection: Option<i16>,
    /// Primary character's trust, if a primary character exists.
    pub trust: Option<i16>,
    /// Primary character's tension, if a primary character exists.
    pub tension: Option<i16>,
    /// Current turn counter.
    pub turn: u32,
}

/// Loading overlay state, present only while a generation is pending.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoadingState {
    /// The pending generation id (matches `PendingGeneration::generation_id`).
    pub generation_id: Uuid,
    /// Korean loading message.
    pub message_ko: String,
}

/// Recoverable error overlay state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DomainErrorView {
    /// Stable machine-readable reason code (from `CoreError`).
    pub code: String,
    /// Short Korean title.
    pub title_ko: String,
    /// Korean detail message.
    pub message_ko: String,
    /// Whether the player can dismiss and continue.
    pub recoverable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_round_trips() {
        let view = AppViewModel {
            screen: Screen::Story,
            story_title: "비가 그친 뒤".to_string(),
            chapter: "chapter-1".to_string(),
            location_display: "카페 나무".to_string(),
            time_slot: TimeSlot::Evening,
            narration: "오랜만이에요.".to_string(),
            choices: vec![ChoiceView {
                id: "choice-0".to_string(),
                label: "사과한다".to_string(),
                risk: RiskLevel::Low,
                preview: Some("과거에 대해 말한다".to_string()),
            }],
            status: StatusLine {
                route: None,
                affection: Some(3),
                trust: Some(1),
                tension: Some(8),
                turn: 1,
            },
            loading: Some(LoadingState {
                generation_id: Uuid::nil(),
                message_ko: "생성 중...".to_string(),
            }),
            error: Some(DomainErrorView {
                code: "patch_unknown_flag".to_string(),
                title_ko: "생성 실패".to_string(),
                message_ko: "알 수 없는 플래그입니다.".to_string(),
                recoverable: true,
            }),
        };
        let json = serde_json::to_string(&view).expect("serialize");
        let back: AppViewModel = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, view);
    }

    #[test]
    fn screen_serializes_snake_case() {
        let s = serde_json::to_string(&Screen::FreeInput).unwrap();
        assert_eq!(s, "\"free_input\"");
        let back: Screen = serde_json::from_str("\"save_load\"").unwrap();
        assert_eq!(back, Screen::SaveLoad);
    }

    #[test]
    fn empty_view_round_trips() {
        let view = AppViewModel {
            screen: Screen::Story,
            story_title: String::new(),
            chapter: String::new(),
            location_display: String::new(),
            time_slot: TimeSlot::Morning,
            narration: String::new(),
            choices: Vec::new(),
            status: StatusLine {
                route: None,
                affection: None,
                trust: None,
                tension: None,
                turn: 0,
            },
            loading: None,
            error: None,
        };
        let json = serde_json::to_string(&view).unwrap();
        let back: AppViewModel = serde_json::from_str(&json).unwrap();
        assert_eq!(back, view);
    }
}
