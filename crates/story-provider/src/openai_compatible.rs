//! [`OpenAiCompatibleProvider`] — the real local-LLM HTTP adapter (ST-017).
//!
//! Talks to an OpenAI-compatible server (MLX-VLM / MLX-LM) at `127.0.0.1`.
//! Builds the chat-completions request envelope from a [`TurnRequest`], POSTs
//! to `/v1/chat/completions`, extracts the assistant message body, and runs it
//! through the existing [`recovery::parse_generated_turn`] (parsing is NOT
//! reimplemented). ADR-002: loopback default; non-loopback warned (NOT
//! blocked); no redirects; no telemetry.
//!
//! Retry policy: HTTP **4xx MUST NOT be retried** (returned immediately as
//! [`ProviderError::Protocol`]); connection errors / 5xx / timeout are retried
//! up to `max_retries` with bounded exponential backoff; the [`CancellationToken`]
//! is honored before every attempt AND raced against the in-flight request.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use reqwest::redirect::Policy;
use reqwest::{Client, StatusCode};
use story_model::{
    GeneratedTurnDraft, ProviderCapabilities, ProviderError, ProviderHealth, TurnRequest,
};
use tokio_util::sync::CancellationToken;

use crate::config::{ProviderConfig, StructuredOutputMode};
use crate::envelope::ChatCompletionRequest;
use crate::recovery::parse_generated_turn;

/// Maximum backoff cap. The exponential backoff starts at `BASE_BACKOFF_MS`.
const MAX_BACKOFF: Duration = Duration::from_millis(1_000);
const BASE_BACKOFF_MS: u64 = 20;

/// Returns `true` when the host part of `url` is a loopback address
/// (`127.0.0.0/8`, `localhost`, `::1`). Used to decide the ADR-002 non-loopback
/// warning — no external URL crate is pulled in (ADR-003: keep deps minimal).
pub fn is_loopback_base_url(url: &str) -> bool {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    // Pull the host: bracketed IPv6 (`[::1]:port`) vs. the more common
    // `host:port` / `host/path` shapes.
    let host = if after_scheme.starts_with('[') {
        // Bracketed IPv6 (`[::1]:port`); include the closing bracket.
        match after_scheme.find(']') {
            Some(end) => &after_scheme[..=end],
            None => after_scheme,
        }
    } else {
        let host_end = after_scheme.find([':', '/']).unwrap_or(after_scheme.len());
        &after_scheme[..host_end]
    };
    is_loopback_host(host)
}

fn is_loopback_host(host: &str) -> bool {
    let h = host.trim();
    h == "127.0.0.1" || h == "localhost" || h == "::1" || h == "[::1]" || h.starts_with("127.")
}

fn join_url(base: &str, path: &str) -> String {
    let trimmed_base = base.trim_end_matches('/');
    let normalized_path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    format!("{trimmed_base}{normalized_path}")
}

/// OpenAI-compatible HTTP provider for a local MLX server.
///
/// Holds a `reqwest::Client` configured with request/connect timeouts and
/// `redirect(Policy::none())` (no redirects by default — ADR-002). A non-loopback
/// `base_url` triggers a one-shot `tracing::warn!` in [`OpenAiCompatibleProvider::new`]
/// and is recorded in [`OpenAiCompatibleProvider::is_loopback`] for UI surfacing.
pub struct OpenAiCompatibleProvider {
    id: String,
    config: ProviderConfig,
    client: Client,
    loopback: bool,
    /// Resolved structured-output capability. For [`StructuredOutputMode::Auto`]
    /// this starts `true` and is disabled for the session if a parse failure
    /// triggers the no-`response_format` fallback retry.
    structured_supported: AtomicBool,
}

impl OpenAiCompatibleProvider {
    /// Build the provider: reqwest client (timeouts, no redirects), loopback
    /// warning if needed. Does NOT touch the network.
    pub fn new(config: ProviderConfig) -> Result<Self, ProviderError> {
        let loopback = is_loopback_base_url(&config.base_url);
        if !loopback {
            tracing::warn!(
                target: "story_provider::openai_compatible",
                base_url = %config.base_url,
                "non-loopback provider URL — 외부 전송 가능성"
            );
        }
        let client = Client::builder()
            .timeout(Duration::from_millis(config.request_timeout_ms))
            .connect_timeout(Duration::from_millis(config.connect_timeout_ms))
            .redirect(Policy::none())
            .build()
            .map_err(|e| ProviderError::Protocol(format!("client build: {e}")))?;
        let structured_supported = match config.structured_output {
            StructuredOutputMode::On | StructuredOutputMode::Auto => true,
            StructuredOutputMode::Off => false,
        };
        Ok(Self {
            id: config.name.clone(),
            config,
            client,
            loopback,
            structured_supported: AtomicBool::new(structured_supported),
        })
    }

    /// Full chat-completions URL.
    pub fn chat_url(&self) -> String {
        join_url(&self.config.base_url, &self.config.chat_path)
    }

    /// Health-check URL.
    pub fn health_url(&self) -> String {
        join_url(&self.config.base_url, &self.config.health_path)
    }

    /// `true` iff the configured `base_url` host is loopback (ADR-002). Surfaced
    /// for the status bar / coordinator; a non-loopback provider still
    /// functions (the warning is non-blocking).
    pub fn is_loopback(&self) -> bool {
        self.loopback
    }

    fn structured_enabled(&self) -> bool {
        self.structured_supported.load(Ordering::Relaxed)
    }

    /// Disable structured output for the rest of this provider's lifetime
    /// (Auto-fallback after a parse failure).
    fn disable_structured(&self) {
        self.structured_supported.store(false, Ordering::Relaxed);
    }

    /// Bounded exponential backoff between retry attempts.
    fn backoff(attempt: u32) -> Duration {
        let exp = attempt.min(5);
        let ms = BASE_BACKOFF_MS.saturating_mul(1u64 << exp);
        Duration::from_millis(ms).min(MAX_BACKOFF)
    }

    /// Sleep for the attempt's backoff, racing against cancellation.
    async fn sleep_or_cancel(
        attempt: u32,
        cancel: &CancellationToken,
    ) -> Result<(), ProviderError> {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(ProviderError::Cancelled),
            _ = tokio::time::sleep(Self::backoff(attempt)) => Ok(()),
        }
    }

    /// One full pass through the transient-retry loop. Returns the parsed draft
    /// on success, or an error. `suppress_rf` is set on the Auto-fallback pass
    /// so `response_format` is omitted even though the capability flag is
    /// already disabled for the session.
    async fn do_generate(
        &self,
        request: &TurnRequest,
        suppress_rf: bool,
        cancel: &CancellationToken,
    ) -> Result<GeneratedTurnDraft, ProviderError> {
        let send_rf = !suppress_rf && self.structured_enabled() && request.json_schema.is_some();
        let body = ChatCompletionRequest::from_turn(request, &self.config.model, send_rf);
        let max_attempts = self.config.max_retries.saturating_add(1);

        for attempt in 0..max_attempts {
            if cancel.is_cancelled() {
                return Err(ProviderError::Cancelled);
            }
            let send = self.client.post(self.chat_url()).json(&body).send();
            let result = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
                r = send => r,
            };

            let resp = match result {
                Ok(r) => r,
                Err(e) => {
                    // Transient (connect / timeout / pool) -> retry, else fail.
                    if attempt + 1 < max_attempts {
                        Self::sleep_or_cancel(attempt, cancel).await?;
                        continue;
                    }
                    return Err(map_send_error(e, self.config.request_timeout_ms));
                }
            };

            let status = resp.status();
            if status.is_client_error() {
                // 4xx MUST NOT be retried (ADR-002).
                return Err(ProviderError::Protocol(format!("HTTP {}", status.as_u16())));
            }
            if !status.is_success() {
                // 5xx / other -> retry.
                if attempt + 1 < max_attempts {
                    Self::sleep_or_cancel(attempt, cancel).await?;
                    continue;
                }
                return Err(ProviderError::Protocol(format!("HTTP {}", status.as_u16())));
            }

            // 2xx: decode the envelope (serde-tolerant).
            let parsed: crate::envelope::ChatCompletionResponse = match resp.json().await {
                Ok(p) => p,
                Err(e) => {
                    return Err(ProviderError::Protocol(format!(
                        "response decode failed: {e}"
                    )));
                }
            };
            let content = parsed.content();
            let usage = parsed.usage();
            return match parse_generated_turn(&content, request.generation_id) {
                Ok(outcome) => {
                    let mut draft = outcome.draft;
                    // Prefer the model's reported usage; fall back to the
                    // envelope's token counts when the body omitted them.
                    if draft.usage.is_none() {
                        draft.usage = usage;
                    }
                    Ok(draft)
                }
                Err(e) => Err(e),
            };
        }
        // Defensive: all attempts hit the transient-retry path without returning.
        Err(ProviderError::Protocol(
            "retry budget exhausted".to_string(),
        ))
    }
}

fn map_send_error(e: reqwest::Error, request_timeout_ms: u64) -> ProviderError {
    if e.is_timeout() {
        ProviderError::Timeout {
            ms: request_timeout_ms,
        }
    } else if e.is_connect() {
        ProviderError::Unavailable(format!("connect: {e}"))
    } else {
        ProviderError::Protocol(format!("send: {e}"))
    }
}

#[async_trait]
impl crate::provider::LlmProvider for OpenAiCompatibleProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_structured_output: self.structured_enabled(),
            supports_director: false,
            supports_streaming: false,
        }
    }

    async fn health(&self) -> Result<ProviderHealth, ProviderError> {
        let result = self.client.get(self.health_url()).send().await;
        match result {
            Err(e) => {
                if e.is_timeout() {
                    Ok(ProviderHealth::Degraded)
                } else {
                    Ok(ProviderHealth::Unavailable)
                }
            }
            Ok(resp) => {
                let s = resp.status();
                if s == StatusCode::OK {
                    Ok(ProviderHealth::Ready)
                } else if s.is_server_error() {
                    Ok(ProviderHealth::Degraded)
                } else {
                    Ok(ProviderHealth::Unavailable)
                }
            }
        }
    }

    async fn generate_turn(
        &self,
        request: TurnRequest,
        cancel: CancellationToken,
    ) -> Result<GeneratedTurnDraft, ProviderError> {
        match self.do_generate(&request, false, &cancel).await {
            Ok(draft) => Ok(draft),
            // Auto-fallback: if structured output was on and the body still
            // failed to parse, disable the capability for the session and retry
            // once WITHOUT response_format. The store(false) makes future turns
            // skip response_format too.
            Err(ProviderError::InvalidGeneration(_detail))
                if self.config.structured_output == StructuredOutputMode::Auto
                    && request.json_schema.is_some()
                    && self.structured_enabled() =>
            {
                tracing::warn!(
                    target: "story_provider::openai_compatible",
                    "structured-output parse failure; retrying without response_format"
                );
                self.disable_structured();
                self.do_generate(&request, true, &cancel).await
            }
            Err(other) => Err(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_predicate_classifies_common_hosts() {
        assert!(is_loopback_base_url("http://127.0.0.1:8080"));
        assert!(is_loopback_base_url("http://localhost:8080"));
        assert!(is_loopback_base_url("http://127.1.2.3"));
        assert!(is_loopback_base_url("http://[::1]:8080"));
        assert!(is_loopback_base_url("http://localhost"));
        assert!(!is_loopback_base_url("http://192.168.0.5:8080"));
        assert!(!is_loopback_base_url("http://10.0.0.1"));
        assert!(!is_loopback_base_url("http://example.com"));
    }

    #[test]
    fn join_url_handles_trailing_slash_and_missing_leading_slash() {
        assert_eq!(
            join_url("http://127.0.0.1:8080", "/v1/chat/completions"),
            "http://127.0.0.1:8080/v1/chat/completions"
        );
        assert_eq!(
            join_url("http://127.0.0.1:8080/", "v1/chat/completions"),
            "http://127.0.0.1:8080/v1/chat/completions"
        );
    }

    #[test]
    fn backoff_is_bounded_and_monotonic() {
        let b0 = OpenAiCompatibleProvider::backoff(0);
        let b3 = OpenAiCompatibleProvider::backoff(3);
        let b_huge = OpenAiCompatibleProvider::backoff(99);
        assert!(b3 > b0, "backoff should grow: {b0:?} vs {b3:?}");
        assert!(
            b_huge <= MAX_BACKOFF,
            "backoff must be capped: {b_huge:?} > {MAX_BACKOFF:?}"
        );
    }
}
