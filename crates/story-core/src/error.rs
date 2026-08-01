//! Error variants and stable reason codes for the engine core (ST-007/008).
//!
//! [`CoreError`] is the single error type returned by the validator, reducer,
//! state machines, and engine. Every patch/rule rejection carries a stable
//! machine-readable `code` (one of the `PATCH_*` / `RULE_*` constants below) so
//! callers can branch on it without parsing the human-readable `detail`.

use thiserror::Error;

/// Top-level error enum for `story-core` operations.
#[derive(Debug, Error)]
pub enum CoreError {
    /// A proposed [`story_model::StatePatch`] failed structural / reference /
    /// range / permission validation. `code` is one of the `PATCH_*` consts.
    #[error("invalid patch ({code}): {detail}")]
    InvalidPatch { code: &'static str, detail: String },

    /// A story rule (constraint) was violated by a patch or action.
    #[error("rule violation ({code}): {detail}")]
    RuleViolation { code: &'static str, detail: String },

    /// A relationship or route transition was forbidden by the state machine.
    #[error("forbidden transition: {0}")]
    ForbiddenTransition(String),

    /// An [`crate::engine_cmd::EngineCommand`] was rejected before it could
    /// produce effects (e.g. unknown choice id, generation already in flight).
    /// `code` is one of the `ENGINE_*` consts.
    #[error("invalid command ({code}): {detail}")]
    InvalidCommand { code: &'static str, detail: String },

    /// A serde_json (de)serialization failure.
    #[error(transparent)]
    Json(#[from] story_model::serde_json::Error),
}

impl CoreError {
    /// Construct an `InvalidPatch` with a reason code and detail.
    pub fn invalid_patch(code: &'static str, detail: impl Into<String>) -> Self {
        Self::InvalidPatch {
            code,
            detail: detail.into(),
        }
    }

    /// Construct a `RuleViolation` with a reason code and detail.
    pub fn rule_violation(code: &'static str, detail: impl Into<String>) -> Self {
        Self::RuleViolation {
            code,
            detail: detail.into(),
        }
    }

    /// Construct an `InvalidCommand` with a reason code and detail.
    pub fn invalid_command(code: &'static str, detail: impl Into<String>) -> Self {
        Self::InvalidCommand {
            code,
            detail: detail.into(),
        }
    }

    /// Stable reason code carried by this error, if any.
    pub fn reason_code(&self) -> Option<&'static str> {
        match self {
            Self::InvalidPatch { code, .. }
            | Self::RuleViolation { code, .. }
            | Self::InvalidCommand { code, .. } => Some(code),
            Self::ForbiddenTransition(_) => Some(FORBIDDEN_TRANSITION),
            Self::Json(_) => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Reason-code constants
// ---------------------------------------------------------------------------

/// A delta targeted a character id not present in `state.characters`.
pub const PATCH_UNKNOWN_CHARACTER: &str = "patch_unknown_character";
/// A flag id in `set_flags` is not in `rules.allowed_flags`.
pub const PATCH_UNKNOWN_FLAG: &str = "patch_unknown_flag";
/// A location id is not in `rules.locations`.
pub const PATCH_UNKNOWN_LOCATION: &str = "patch_unknown_location";
/// A number variable id is not in `rules.numbers`.
pub const PATCH_UNKNOWN_NUMBER: &str = "patch_unknown_number";
/// A string variable id is not in `rules.strings`.
pub const PATCH_UNKNOWN_STRING: &str = "patch_unknown_string";
/// An item id is not in `rules.allowed_items`.
pub const PATCH_UNKNOWN_ITEM: &str = "patch_unknown_item";
/// An event id is not in `rules.events`.
pub const PATCH_UNKNOWN_EVENT: &str = "patch_unknown_event";
/// A single-turn delta exceeds `rules.delta_limits`.
pub const PATCH_DELTA_OUT_OF_RANGE: &str = "patch_delta_out_of_range";
/// A projected final stat exceeds `rules.stat_limits`.
pub const PATCH_STAT_OUT_OF_RANGE: &str = "patch_stat_out_of_range";
/// A projected inventory quantity would go negative.
pub const PATCH_NEGATIVE_INVENTORY: &str = "patch_negative_inventory";
/// A relationship transition skips more than one stage without permission.
pub const PATCH_RELATIONSHIP_SKIP: &str = "patch_relationship_skip";
/// `next_location` is not adjacent to the current location and no event
/// effect forces it.
pub const PATCH_NONADJACENT_LOCATION: &str = "patch_nonadjacent_location";
/// A string value exceeded `max_len` or was not in the `allowed` set.
pub const PATCH_STRING_VIOLATION: &str = "patch_string_violation";
/// A number value fell outside the variable's `[min, max]`.
pub const PATCH_NUMBER_OUT_OF_RANGE: &str = "patch_number_out_of_range";
/// Generic forbidden-transition code (used when none of the above apply).
pub const FORBIDDEN_TRANSITION: &str = "forbidden_transition";

/// `SubmitChoice` arrived while a generation is already pending.
pub const ENGINE_GENERATION_IN_FLIGHT: &str = "engine_generation_in_flight";
/// `SubmitChoice` referenced a choice id not in the current view.
pub const ENGINE_UNKNOWN_CHOICE: &str = "engine_unknown_choice";
/// `SubmitChoice` arrived when no choices are available yet.
pub const ENGINE_NO_CHOICES: &str = "engine_no_choices";

/// A `TurnSummary` asserted facts/changes without any `source_history_ids`
/// anchor (ST-030 source-coverage).
pub const SUMMARY_MISSING_SOURCE_HISTORY_IDS: &str = "summary_missing_source_history_ids";
/// A `TurnSummary` listed a `source_history_id` that is not present in the
/// supplied source history entries (an invented/unsourced fact, ST-030).
pub const SUMMARY_UNSOURCED_FACT: &str = "summary_unsourced_fact";

// ---------------------------------------------------------------------------
// Director / Writer reason codes (W10, ST-033/034)
// ---------------------------------------------------------------------------

/// `reveal_fact_ids` overlaps `must_not_reveal_fact_ids`, or a reveal targets a
/// character's declared `unknown_facts` (knowledge-scope violation).
pub const DIRECTOR_REVEAL_CONFLICT: &str = "director_reveal_conflict";
/// `emotion_arcs` references a character id not present in `state.characters`.
pub const DIRECTOR_UNKNOWN_CHARACTER: &str = "director_unknown_character";
/// `choices` count outside `[1, 6]`, or a choice has empty `id`/`label`/`intent`.
pub const DIRECTOR_INVALID_CHOICE: &str = "director_invalid_choice";
/// `end_condition` is empty / whitespace-only.
pub const DIRECTOR_EMPTY_END_CONDITION: &str = "director_empty_end_condition";
/// The proposed patch passed the W3 validator but failed the reducer dry-run
/// (e.g. an event effect went out of range) — the Writer is NOT called.
pub const DIRECTOR_DRY_RUN_FAILED: &str = "director_dry_run_failed";
