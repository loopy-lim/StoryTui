//! Types produced by the generation pipeline for a single turn.

use serde::{Deserialize, Serialize};

use crate::{ActionType, CharacterId, Choice, StatePatch};

/// The player's action for a turn, parsed from free-text input.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParsedAction {
    /// Coarse classification of the action.
    pub action_type: ActionType,
    /// Character the action targets, if any.
    pub target: Option<CharacterId>,
    /// Normalized intent string.
    pub intent: Option<String>,
    /// Optional tone hint.
    pub tone: Option<String>,
    /// Cleaned-up content the player entered.
    pub content: String,
    /// Original raw text the player typed.
    pub original_text: String,
    /// Parser confidence in 0..=1, if known.
    pub confidence: Option<f32>,
}

/// Structured summary of a turn, persisted for later recall.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TurnSummary {
    // --- MVP wire fields (always populated) ---
    /// Salient facts established this turn.
    pub facts: Vec<String>,
    /// Open threads left unresolved.
    pub unresolved: Vec<String>,
    /// Human-readable relationship deltas.
    pub relationship_changes: Vec<String>,
    /// Human-readable knowledge deltas.
    pub knowledge_changes: Vec<String>,
    // --- Extended fields (optional, populated from M6+) ---
    /// Optional natural-language interpretation of the turn.
    pub interpretation: Option<String>,
    /// Address-form change notes (e.g. switched honorific).
    pub address_form_changes: Vec<String>,
    /// Player actions flagged as important for recall.
    pub important_player_actions: Vec<String>,
    /// History entries this summary was derived from.
    pub source_history_ids: Vec<uuid::Uuid>,
}

/// Non-fatal warning emitted during generation (e.g. fallback used).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GenerationWarning {
    /// Stable machine-readable code (e.g. `"retry_budget_exhausted"`).
    pub code: String,
    /// Human-readable detail.
    pub detail: String,
}

/// Token accounting for a single generation call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Tokens consumed by the prompt.
    pub prompt_tokens: u32,
    /// Tokens consumed by the completion.
    pub completion_tokens: u32,
    /// Total tokens (`prompt_tokens + completion_tokens`).
    pub total_tokens: u32,
}

/// The full output of generating one turn: narration, choices, and the patch.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GeneratedTurn {
    /// Unique id for this generation; stable across retries of the same turn.
    pub generation_id: uuid::Uuid,
    /// Narration prose shown to the player.
    pub narration: String,
    /// Choices offered this turn.
    pub choices: Vec<Choice>,
    /// Patch the model proposed (subject to validation).
    pub proposed_patch: StatePatch,
    /// Patch that survived validation and was actually applied.
    pub accepted_patch: StatePatch,
    /// Structured summary for memory/recall.
    pub summary: TurnSummary,
    /// Non-fatal warnings.
    pub warnings: Vec<GenerationWarning>,
    /// Token usage, if the provider reported it.
    pub usage: Option<TokenUsage>,
}
