//! The [`LlmProvider`] trait surface (ST-016).
//!
//! This is the single async boundary the deterministic `story-core` engine
//! talks to when it needs a model. All network interaction (`reqwest`) lives
//! behind implementations of this trait — the trait itself depends only on
//! cross-boundary DTOs from `story-model` plus a tokio cancellation token.
//!
//! The default `direct`/`write_stream` bodies still return
//! [`ProviderError::UnsupportedCapability`] so providers that do not implement
//! the optional Director/Writer split (e.g. [`crate::MockProvider`]) compile
//! unchanged; [`crate::OpenAiCompatibleProvider`] overrides both with real
//! HTTP + SSE impls (W10).

use async_trait::async_trait;
use story_model::{
    DirectorPlan, DirectorRequest, GeneratedTurnDraft, ProviderCapabilities, ProviderError,
    ProviderHealth, TurnRequest, WriterEvent, WriterFinish, WriterRequest,
};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

/// Async LLM boundary for StoryTUI.
///
/// Implementations MUST be `Send + Sync` (the engine holds them behind an
/// `Arc<dyn LlmProvider>`). The four methods are intentionally minimal:
/// identity, advertised capabilities, health probe, and the single
/// generation entry point.
// Newer clippy (1.99+) fires `double_must_use` inside async_trait's
// expansion: the macro emits a bare `#[must_use]` for methods that already
// return `Result` (must_use by type). The attribute is the macro's, not
// ours — silence it at the trait boundary.
#[allow(clippy::double_must_use)]
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

    /// Director-planner path (non-streaming). Real impl on
    /// [`crate::OpenAiCompatibleProvider`]; the default returns
    /// [`ProviderError::UnsupportedCapability`] so providers that never
    /// advertise `supports_director` (e.g. [`crate::MockProvider`]) compile
    /// unchanged.
    async fn direct(
        &self,
        _req: DirectorRequest,
        _cancel: CancellationToken,
    ) -> Result<DirectorPlan, ProviderError> {
        Err(ProviderError::UnsupportedCapability("director".into()))
    }

    /// Token-streaming Writer path. `sink` receives [`WriterEvent::Chunk`]s as
    /// each SSE data line lands; the method returns the final
    /// [`WriterFinish`] once `[DONE]` is observed. Real impl on
    /// [`crate::OpenAiCompatibleProvider`]; the default returns
    /// [`ProviderError::UnsupportedCapability`].
    async fn write_stream(
        &self,
        _req: WriterRequest,
        _sink: Sender<WriterEvent>,
        _cancel: CancellationToken,
    ) -> Result<WriterFinish, ProviderError> {
        Err(ProviderError::UnsupportedCapability("streaming".into()))
    }
}
