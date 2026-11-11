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
use crate::{CharacterId, Emotion, RelationshipStage, RiskLevel, TimeSlot};

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
    /// Checkpoint the player can rewind to with one keypress (the newest
    /// committed state older than the current turn). `None` when there is
    /// nothing to undo (fresh game, save restore, or mid-generation).
    #[serde(default)]
    pub undo_checkpoint_id: Option<Uuid>,
    /// Variant-cycle badge for the displayed narration, e.g. `"변형 2/3"`.
    /// `Some` only while the last committed turn has more than one stored
    /// variant; `None` otherwise (plain turns, loading, after undo).
    #[serde(default)]
    pub variant_label: Option<String>,
    /// Live status of every cast member (id-key order): the `p` roster
    /// overlay's rows and the free-input `@mention` target pool. Absent in
    /// views serialized before the cast roster shipped; `serde(default)`
    /// keeps those loadable, exactly like `variant_label`.
    #[serde(default)]
    pub cast: Vec<CharacterStatusView>,
    /// Truncation state of the displayed turn's prose, derived from the
    /// Writer stream's `finish_reason == "length"` (max_tokens cut). The host
    /// decides the reaction: `Auto` allows ONE automatic `ContinueNarration`,
    /// `ManualOnly` forbids it (the per-turn defensive re-call budget was
    /// spent). Cleared on the next regular turn request/commit.
    #[serde(default)]
    pub truncation: TruncationStatus,
    /// `true` when the last side-generation (continue / suggested action /
    /// group scene) came back empty and therefore applied nothing. The host
    /// may re-dispatch the same side command once and/or toast; the engine
    /// never re-dispatches by itself. A non-empty side accept, a regular
    /// turn request/commit, or a rewind clears it.
    #[serde(default)]
    pub empty_side_result: bool,
}

/// Truncation state of the currently displayed turn's prose (AC-8: at most
/// one defensive re-call per turn).
///
/// The engine derives it from the Writer stream's `finish_reason`; `"length"`
/// means the output token cap cut the prose mid-sentence. Pure
/// dispatch/display data — the engine never auto-continues on its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TruncationStatus {
    /// No reported truncation for the displayed turn (single-pass turns,
    /// `"stop"`, unreported finish, or already retired).
    #[default]
    None,
    /// The displayed prose was cut mid-sentence and this turn has NOT spent
    /// its defensive re-call (no narration repair ran): the host may
    /// auto-continue once.
    Auto,
    /// The displayed prose was cut, but this turn already spent its
    /// defensive re-call (a narration repair ran, or an auto-continue
    /// already appended): the host must NOT auto-continue — manual only.
    ManualOnly,
}

/// One cast member's live status, joined from `pack.characters` (display
/// name) and `game.characters` (mutable state). Pure display data — the TUI
/// formats it and matches `@mentions` against it; the engine stays
/// authoritative for the underlying `CharacterState`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CharacterStatusView {
    /// Pack-stable character id (echoed as the free-action `target`).
    pub id: CharacterId,
    /// Pack display name (falls back to the id string for state-only
    /// characters a pack no longer declares).
    pub name: String,
    /// Current relationship stage bucket (unchanged by scene side-generations).
    pub relationship_stage: RelationshipStage,
    /// Current dominant emotion.
    pub current_emotion: Emotion,
    /// Affection score (same domain as `CharacterState.affection`).
    pub affection: i16,
    /// Trust score.
    pub trust: i16,
    /// Tension score.
    pub tension: i16,
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
    /// Latest turn's affection delta for the primary character, if any.
    #[serde(default)]
    pub affection_delta: Option<i16>,
    /// Latest turn's trust delta for the primary character, if any.
    #[serde(default)]
    pub trust_delta: Option<i16>,
    /// Latest turn's tension delta for the primary character, if any.
    #[serde(default)]
    pub tension_delta: Option<i16>,
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
                affection_delta: Some(1),
                trust_delta: None,
                tension_delta: Some(-1),
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
            undo_checkpoint_id: Some(Uuid::nil()),
            variant_label: None,
            cast: vec![CharacterStatusView {
                id: CharacterId("c-1".to_string()),
                name: "유나".to_string(),
                relationship_stage: crate::RelationshipStage::Acquaintance,
                current_emotion: Emotion::Neutral,
                affection: 3,
                trust: 1,
                tension: 8,
            }],
            truncation: TruncationStatus::Auto,
            empty_side_result: false,
        };
        let json = serde_json::to_string(&view).expect("serialize");
        let back: AppViewModel = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, view);
        assert!(
            json.contains("\"truncation\":\"auto\""),
            "TruncationStatus serializes snake_case: {json}"
        );
    }

    #[test]
    fn view_undo_roundtrip() {
        // Views serialized before undo shipped carry no `undo_checkpoint_id`;
        // the field must deserialize to None instead of failing.
        let json = r#"{
            "screen": "story",
            "story_title": "비",
            "chapter": "chapter-1",
            "location_display": "카페",
            "time_slot": "evening",
            "narration": "내레이션",
            "choices": [],
            "status": {"route": null, "affection": null, "trust": null, "tension": null, "turn": 2},
            "loading": null,
            "error": null
        }"#;
        let back: AppViewModel = serde_json::from_str(json).expect("deserialize legacy view");
        assert_eq!(back.undo_checkpoint_id, None);
        assert_eq!(back.variant_label, None);
        assert!(
            back.cast.is_empty(),
            "views serialized before the cast roster shipped deserialize to an empty cast"
        );
        assert_eq!(back.truncation, TruncationStatus::None);
        assert!(!back.empty_side_result);
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
                affection_delta: None,
                trust_delta: None,
                tension_delta: None,
            },
            loading: None,
            error: None,
            undo_checkpoint_id: None,
            variant_label: None,
            cast: Vec::new(),
            truncation: TruncationStatus::None,
            empty_side_result: false,
        };
        let json = serde_json::to_string(&view).unwrap();
        let back: AppViewModel = serde_json::from_str(&json).unwrap();
        assert_eq!(back, view);
    }
}
