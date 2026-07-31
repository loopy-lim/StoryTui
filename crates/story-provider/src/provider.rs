//! The [`LlmProvider`] trait surface (ST-016).
//!
//! This is the single async boundary the deterministic `story-core` engine
//! talks to when it needs a model. All network interaction (`reqwest`) lives
//! behind implementations of this trait — the trait itself depends only on
//! cross-boundary DTOs from `story-model` plus a tokio cancellation token.
//!
//! MVP scope: `direct` (director planner) and `write_stream` (token streaming)
//! are placeholder methods that return [`ProviderError::UnsupportedCapability`]
//! by default. Real implementations land in W6 (HTTP) / W10 (streaming).

use async_trait::async_trait;
use story_model::{
    GeneratedTurnDraft, ProviderCapabilities, ProviderError, ProviderHealth, TurnRequest,
};
use tokio_util::sync::CancellationToken;

/// Async LLM boundary for StoryTUI.
///
/// Implementations MUST be `Send + Sync` (the engine holds them behind an
/// `Arc<dyn LlmProvider>`). The four methods are intentionally minimal:
/// identity, advertised capabilities, health probe, and the single
/// generation entry point.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Stable provider id (e.g. `"mock"`, `"openai-http"`), surfaced in logs.
    fn id(&self) -> &str;

    /// Capabilities advertised by this provider.
    fn capabilities(&self) -> ProviderCapabilities;

    /// Current health, used by the status bar / coordinator (FR-037).
    async fn health(&self) -> Result<ProviderHealth, ProviderError>;

    /// Generate one turn. Honors `cancel`: if cancelled during a latency wait,
    /// returns [`ProviderError::Cancelled`] and discards any in-flight draft.
    async fn generate_turn(
        &self,
        request: TurnRequest,
        cancel: CancellationToken,
    ) -> Result<GeneratedTurnDraft, ProviderError>;

    /// Director-planner path (MVP: unsupported). Real impl in W6.
    async fn direct(&self, _req: (), _cancel: CancellationToken) -> Result<(), ProviderError> {
        Err(ProviderError::UnsupportedCapability("director".into()))
    }

    /// Token-streaming path (MVP: unsupported). Real impl in W10.
    async fn write_stream(
        &self,
        _req: (),
        _sink: (),
        _cancel: CancellationToken,
    ) -> Result<(), ProviderError> {
        Err(ProviderError::UnsupportedCapability("streaming".into()))
    }
}
