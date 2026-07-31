//! `MockProvider` — a programmable, in-process fixture provider for tests
//! (ST-016).
//!
//! Holds a FIFO queue of [`MockResponse`]s and replays them one per
//! [`MockProvider::generate_turn`] call. No network, no `reqwest`. Latency is
//! modeled with `tokio::time::sleep`, and every latency-bearing response is
//! cancellable: cancelling during a `Delayed`/`Late`/`Timeout` wait returns
//! [`ProviderError::Cancelled`] immediately and discards any in-flight draft.
//!
//! This module is the backbone of the W5 vertical slice: the engine and
//! coordinator (Tranche 2) drive it exactly as they will the real W6 provider.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use story_model::{
    GeneratedTurnDraft, ProviderCapabilities, ProviderError, ProviderHealth, TurnRequest,
};
use tokio_util::sync::CancellationToken;

use crate::provider::LlmProvider;

/// A single scripted response for [`MockProvider`] to return.
///
/// `Delayed` and `Late` differ only in intent: both sleep then succeed, but
/// `Late` is meant to be cancelled mid-flight (the draft must never surface).
#[derive(Debug)]
pub enum MockResponse {
    /// Succeed immediately with this draft.
    Draft(GeneratedTurnDraft),
    /// Succeed after `ms` milliseconds (cancellable).
    Delayed {
        /// The draft to return once the latency elapses.
        draft: GeneratedTurnDraft,
        /// Simulated latency in milliseconds.
        ms: u64,
    },
    /// Return a raw malformed body (the parser is exercised separately in
    /// `recovery`; this variant surfaces an invalid-generation error with a
    /// sanitized detail and does NOT embed the raw body).
    Malformed(String),
    /// Sleep for `ms` then return [`ProviderError::Timeout`] (cancellable).
    Timeout {
        /// Simulated latency before the timeout fires.
        ms: u64,
    },
    /// Return this error immediately.
    Error(ProviderError),
    /// Complete after a long delay; intended to be cancelled so the draft is
    /// discarded (never surfaced). Treated identically to [`Self::Delayed`].
    Late {
        /// The draft that must NEVER surface when cancelled.
        draft: GeneratedTurnDraft,
        /// Simulated latency in milliseconds.
        ms: u64,
    },
}

/// An in-process, network-free provider that replays a scripted [`MockResponse`]
/// queue. `Send + Sync` via a brief-hold `std::sync::Mutex` around the queue.
#[derive(Debug)]
pub struct MockProvider {
    id: String,
    queue: Mutex<VecDeque<MockResponse>>,
    health: ProviderHealth,
    caps: ProviderCapabilities,
}

impl MockProvider {
    /// New mock with the given id, empty queue, [`ProviderHealth::Ready`], and
    /// MVP defaults (all capabilities `false`).
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            queue: Mutex::new(VecDeque::new()),
            health: ProviderHealth::Ready,
            caps: ProviderCapabilities {
                supports_structured_output: false,
                supports_director: false,
                supports_streaming: false,
            },
        }
    }

    /// Builder: enqueue a scripted response (pushed to the back of the queue).
    #[must_use]
    pub fn with(mut self, response: MockResponse) -> Self {
        self.queue.get_mut().unwrap().push_back(response);
        self
    }

    /// Builder: override the advertised health.
    #[must_use]
    pub fn with_health(mut self, health: ProviderHealth) -> Self {
        self.health = health;
        self
    }

    /// Builder: override the advertised capabilities.
    #[must_use]
    pub fn with_capabilities(mut self, caps: ProviderCapabilities) -> Self {
        self.caps = caps;
        self
    }

    /// Pop the next scripted response, or `None` if the queue is empty.
    fn pop(&self) -> Option<MockResponse> {
        self.queue.lock().expect("mock queue poisoned").pop_front()
    }
}

#[async_trait]
impl LlmProvider for MockProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn capabilities(&self) -> ProviderCapabilities {
        self.caps.clone()
    }

    async fn health(&self) -> Result<ProviderHealth, ProviderError> {
        Ok(self.health)
    }

    async fn generate_turn(
        &self,
        _request: TurnRequest,
        cancel: CancellationToken,
    ) -> Result<GeneratedTurnDraft, ProviderError> {
        let response = self.pop().ok_or_else(|| {
            ProviderError::Unavailable("mock response queue exhausted".to_string())
        })?;
        match response {
            MockResponse::Draft(draft) => Ok(draft),
            MockResponse::Delayed { draft, ms } => {
                select_sleep_or_cancel(Duration::from_millis(ms), &cancel).await?;
                Ok(draft)
            }
            MockResponse::Late { draft, ms } => {
                select_sleep_or_cancel(Duration::from_millis(ms), &cancel).await?;
                Ok(draft)
            }
            MockResponse::Timeout { ms } => {
                select_sleep_or_cancel(Duration::from_millis(ms), &cancel).await?;
                Err(ProviderError::Timeout { ms })
            }
            MockResponse::Error(err) => Err(err),
            // Sanitized detail — the raw mock body is NOT embedded in the error.
            MockResponse::Malformed(_) => Err(ProviderError::InvalidGeneration(
                "malformed mock body".to_string(),
            )),
        }
    }
}

/// Await a latency, racing it against cancellation. Returns `Ok(())` if the
/// latency elapses, or `Err(Cancelled)` if the token fires first.
async fn select_sleep_or_cancel(
    latency: Duration,
    cancel: &CancellationToken,
) -> Result<(), ProviderError> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(ProviderError::Cancelled),
        _ = tokio::time::sleep(latency) => Ok(()),
    }
}
