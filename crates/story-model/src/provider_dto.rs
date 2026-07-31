//! Cross-boundary DTOs for the LLM provider layer (ST-016).
//!
//! These pure-data types are the vocabulary spoken across the
//! `story-provider` (async, network, reqwest) <-> `story-core` (deterministic,
//! IO-free) boundary. Keeping them in `story-model` means neither crate has to
//! depend on the other for the wire shape, and the DTOs stay free of UI/HTTP/FS
//! (ADR-003).
//!
//! # Serde policy
//!
//! - Struct fields serialize as snake_case (serde default).
//! - [`ProviderHealth`] uses `#[serde(rename_all = "snake_case")]`.
//! - [`ProviderError`] uses the default externally-tagged enum representation
//!   (`{"Timeout":{"ms":7}}`, `"Cancelled"`) so it round-trips with `serde_json`
//!   while ALSO implementing `thiserror::Error` for ergonomic `?` propagation on
//!   the provider side.

use serde::{Deserialize, Serialize};

use crate::{Choice, StatePatch, TokenUsage, TurnSummary};

// ---------------------------------------------------------------------------
// Chat / request
// ---------------------------------------------------------------------------

/// A single chat message in a provider request.
///
/// `role` is one of `"system"`, `"user"`, `"assistant"`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Conversation role: `"system"`, `"user"`, or `"assistant"`.
    pub role: String,
    /// Verbatim message content.
    pub content: String,
}

/// A single generation request handed to an [`crate::LlmProvider`]-style trait
/// in `story-provider`.
///
/// `json_schema` is populated only when the provider's
/// [`ProviderCapabilities::supports_structured_output`] is confirmed; otherwise
/// the engine falls back to free-form JSON with a field contract.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TurnRequest {
    /// Unique id for this generation; stable across retries of the same turn.
    pub generation_id: uuid::Uuid,
    /// Ordered chat messages forming the prompt.
    pub messages: Vec<ChatMessage>,
    /// Provider-specific model id to target.
    pub model: String,
    /// Sampling temperature.
    pub temperature: f32,
    /// Nucleus sampling probability mass.
    pub top_p: f32,
    /// Top-k sampling cutoff (0 = provider default / disabled).
    pub top_k: u32,
    /// Maximum output tokens for this turn.
    pub max_tokens: u32,
    /// Optional JSON Schema constraining the response, included only when the
    /// provider confirmed structured-output support.
    pub json_schema: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Generation result (pre-validation)
// ---------------------------------------------------------------------------

/// The raw output of one generation call, BEFORE the deterministic engine
/// validates / clamps the patch.
///
/// `story-provider` produces this (potentially via the [`crate`] recovery
/// parser); `story-core` consumes the patch/choices/summary and emits a fully
/// validated `GeneratedTurn`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GeneratedTurnDraft {
    /// Generation id echoed back from the request.
    pub generation_id: uuid::Uuid,
    /// Narration prose shown to the player.
    pub narration: String,
    /// Player-facing choices offered this turn.
    pub choices: Vec<Choice>,
    /// Patch the model proposed (subject to validator clamping).
    pub proposed_patch: StatePatch,
    /// Structured summary for memory/recall.
    pub summary: TurnSummary,
    /// Token usage, if the provider reported it.
    pub usage: Option<TokenUsage>,
}

// ---------------------------------------------------------------------------
// Health / capabilities
// ---------------------------------------------------------------------------

/// Coarse health of a provider, surfaced to the UI/status bar (FR-037).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderHealth {
    /// Ready to serve requests.
    Ready,
    /// Warming up (model load / cold start).
    Loading,
    /// Cannot serve at all (e.g. process down).
    Unavailable,
    /// Serving but degraded (retries, slow, partial).
    Degraded,
    /// Configuration error (missing key, bad endpoint).
    Misconfigured,
}

/// Capability flags advertised by a provider. The engine uses these to decide
/// whether to use the director/streaming paths or fall back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    /// Supports `response_format` JSON Schema (structured output).
    pub supports_structured_output: bool,
    /// Supports the separate director planner (MVP: `false`).
    pub supports_director: bool,
    /// Supports token streaming (MVP: `false`).
    pub supports_streaming: bool,
}

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Errors a provider can return. Crosses the boundary as data (it derives
/// `Serialize`/`Deserialize`) and ALSO implements `thiserror::Error` so the
/// provider crate can use `?` and `Result<_, ProviderError>` ergonomically.
///
/// Note: the `Timeout` variant's error message interpolates the `ms` field
/// directly (`timeout after {ms}ms`); `thiserror` references named struct
/// fields by name.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Serialize, Deserialize)]
pub enum ProviderError {
    /// Provider reachable but unusable right now.
    #[error("provider unavailable: {0}")]
    Unavailable(String),
    /// Provider is still warming up / loading.
    #[error("provider loading: {0}")]
    Loading(String),
    /// Wire/protocol error (bad JSON, bad status, envelope mismatch).
    #[error("protocol error: {0}")]
    Protocol(String),
    /// Request exceeded the per-turn deadline.
    #[error("timeout after {ms}ms")]
    Timeout { ms: u64 },
    /// Request was cancelled via the `CancellationToken`.
    #[error("cancelled")]
    Cancelled,
    /// An unsupported capability path was invoked (director/streaming in MVP).
    #[error("unsupported capability: {0}")]
    UnsupportedCapability(String),
    /// Model output present but failed strict parsing / recovery.
    #[error("invalid generation: {0}")]
    InvalidGeneration(String),
}
