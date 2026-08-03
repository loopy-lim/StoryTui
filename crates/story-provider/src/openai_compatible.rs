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
use futures_util::StreamExt;
use reqwest::redirect::Policy;
use reqwest::{Client, StatusCode};
use story_model::{
    DirectorPlan, DirectorRequest, GeneratedTurnDraft, ProviderCapabilities, ProviderError,
    ProviderHealth, TokenUsage, TurnRequest, WriterEvent, WriterFinish, WriterRequest,
};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

use crate::config::{ProviderConfig, StructuredOutputMode};
use crate::envelope::ChatCompletionRequest;
use crate::recovery::parse_generated_turn;
use crate::sse::{SseEvent, SseParser};

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
    /// Bearer token resolved once from `ProviderConfig::api_key_env`.
    api_key: Option<String>,
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
        let api_key = config
            .api_key
            .clone()
            .or_else(|| {
                config
                    .api_key_env
                    .as_deref()
                    .and_then(|name| std::env::var(name).ok())
            })
            .filter(|value| !value.trim().is_empty());
        Ok(Self {
            id: config.name.clone(),
            config,
            client,
            loopback,
            structured_supported: AtomicBool::new(structured_supported),
            api_key,
        })
    }

    fn authorize(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_key {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    /// Full chat-completions URL.
    pub fn chat_url(&self) -> String {
        join_url(&self.config.base_url, &self.config.chat_path)
    }

    /// Health-check URL.
    pub fn health_url(&self) -> String {
        join_url(&self.config.base_url, &self.config.health_path)
    }

    /// OpenAI-compatible model-list URL.
    pub fn models_url(&self) -> String {
        join_url(&self.config.base_url, "/v1/models")
    }

    /// Fetch the models exposed by the configured OpenAI-compatible server.
    ///
    /// The endpoint follows OpenAI's `{ "data": [{ "id": "..." }] }`
    /// envelope. Empty identifiers are ignored and the returned identifiers
    /// are sorted and deduplicated so a TUI can render a stable chooser.
    /// This is intentionally a single GET: unlike generation, model discovery
    /// has no cancellation token or retry contract.
    pub async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        let response = self
            .authorize(self.client.get(self.models_url()))
            .send()
            .await
            .map_err(|error| map_send_error(error, self.config.request_timeout_ms))?;

        if !response.status().is_success() {
            return Err(ProviderError::Protocol(format!(
                "HTTP {}",
                response.status().as_u16()
            )));
        }

        let response: ModelListResponse = response.json().await.map_err(|error| {
            ProviderError::Protocol(format!("model-list response decode failed: {error}"))
        })?;
        let mut ids: Vec<String> = response
            .data
            .into_iter()
            .filter_map(|model| {
                let id = model.id.trim();
                (!id.is_empty()).then(|| id.to_string())
            })
            .collect();
        ids.sort_unstable();
        ids.dedup();
        Ok(ids)
    }

    /// `true` iff the configured `base_url` host is loopback (ADR-002). Surfaced
    /// for the status bar / coordinator; a non-loopback provider still
    /// functions (the warning is non-blocking).
    pub fn is_loopback(&self) -> bool {
        self.loopback
    }

    /// Model id pinned into requests created by this provider instance.
    pub fn model_name(&self) -> &str {
        &self.config.model
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
        temperature_override: Option<f32>,
    ) -> Result<GeneratedTurnDraft, ProviderError> {
        // The engine owns prompt construction, while the runtime provider
        // owns sampling.  Do not let MVP placeholder values embedded in a
        // TurnRequest silently override the operator's TOML profile.
        let mut configured_request = request.clone();
        configured_request.temperature = temperature_override.unwrap_or(self.config.temperature);
        configured_request.top_p = self.config.top_p;
        configured_request.top_k = self.config.top_k;
        configured_request.max_tokens = self.config.max_tokens;
        let bounded_request = bound_context(&configured_request, self.config.context_budget_tokens);
        let send_rf = !suppress_rf && self.structured_enabled() && request.json_schema.is_some();
        let body = ChatCompletionRequest::from_turn(&bounded_request, &self.config.model, send_rf);
        let max_attempts = self.config.max_retries.saturating_add(1);

        for attempt in 0..max_attempts {
            if cancel.is_cancelled() {
                return Err(ProviderError::Cancelled);
            }
            let send = self
                .authorize(self.client.post(self.chat_url()))
                .json(&body)
                .send();
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

            // A 2xx status does not guarantee that a local server completed
            // its body. Read it explicitly so a truncated/malformed envelope
            // is diagnosed without logging raw model output, then treat it as
            // a transient server response and use the configured retry budget.
            let envelope_body = match resp.text().await {
                Ok(body) => body,
                Err(e) => {
                    // A large/slow local model can blow the request deadline
                    // while the body is still streaming. reqwest marks that as a
                    // timeout: surface it as `Timeout` (player sees "시간 초과"
                    // and knows to raise `request_timeout_ms` or lower
                    // `max_tokens`) rather than a generic protocol error.
                    let timed_out = e.is_timeout();
                    tracing::warn!(
                        target: "story_provider::openai_compatible",
                        generation_id = %request.generation_id,
                        model = %self.config.model,
                        http_status = %status,
                        timed_out,
                        error = %e,
                        "chat-completion response body read failed"
                    );
                    if attempt + 1 < max_attempts {
                        Self::sleep_or_cancel(attempt, cancel).await?;
                        continue;
                    }
                    return Err(if timed_out {
                        ProviderError::Timeout {
                            ms: self.config.request_timeout_ms,
                        }
                    } else {
                        ProviderError::Protocol(format!("response body read failed: {e}"))
                    });
                }
            };
            let parsed: crate::envelope::ChatCompletionResponse =
                match story_model::serde_json::from_str(&envelope_body) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!(
                            target: "story_provider::openai_compatible",
                            generation_id = %request.generation_id,
                            model = %self.config.model,
                            http_status = %status,
                            response_chars = envelope_body.chars().count(),
                            error = %e,
                            "chat-completion envelope decode failed; retrying if budget remains"
                        );
                        if attempt + 1 < max_attempts {
                            Self::sleep_or_cancel(attempt, cancel).await?;
                            continue;
                        }
                        return Err(ProviderError::Protocol(format!(
                            "response decode failed: {e}"
                        )));
                    }
                };
            let content = parsed.content();
            let usage = parsed.usage();
            let finish_reason = parsed.finish_reason().unwrap_or("missing").to_string();
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
                Err(e) => {
                    tracing::warn!(
                        target: "story_provider::openai_compatible",
                        generation_id = %request.generation_id,
                        model = %self.config.model,
                        finish_reason,
                        assistant_content_chars = content.chars().count(),
                        error = %e,
                        "assistant content rejected by game response contract"
                    );
                    Err(e)
                }
            };
        }
        // Defensive: all attempts hit the transient-retry path without returning.
        Err(ProviderError::Protocol(
            "retry budget exhausted".to_string(),
        ))
    }
}

/// Only envelope-level failures indicate that the server ignored constrained
/// decoding. Contract-level failures should retain `response_format` for the
/// bounded repair retry.
fn is_json_envelope_failure(detail: &str) -> bool {
    detail.starts_with("json_extract:") || detail.starts_with("json_syntax:")
}

fn narration_char_count(draft: &GeneratedTurnDraft) -> usize {
    draft.narration.chars().count()
}

fn length_repair_request(
    request: &TurnRequest,
    first: &GeneratedTurnDraft,
    min_chars: usize,
) -> TurnRequest {
    let mut retry = request.clone();
    // Anchor the rewrite on the model's own first answer so the expansion
    // deepens the SAME scene — same events, choices, and state_patch — instead
    // of regenerating a fresh, possibly earlier or divergent, scene. Without
    // this assistant turn, a weaker model sees only two consecutive user
    // messages and tends to drift or repeat a prior beat.
    let anchor = story_model::serde_json::json!({
        "narration": first.narration,
        "choices": first.choices,
        "state_patch": first.proposed_patch,
        "summary": first.summary,
    });
    retry.messages.push(story_model::ChatMessage {
        role: "assistant".to_string(),
        content: story_model::serde_json::to_string(&anchor)
            .unwrap_or_else(|_| first.narration.clone()),
    });
    retry.messages.push(story_model::ChatMessage {
        role: "user".to_string(),
        content: format!(
            "위 응답은 JSON 계약을 지켰지만 장면 본문(narration)이 짧습니다. 같은 사건과 같은 choices·state_patch를 유지하면서 narration만 최소 {min_chars}자의 풍부한 한국어 본문으로 다시 씁니다. 감각 묘사·대화·행동·감정 변화를 여러 문단으로 포함하고, 동일한 구조의 JSON 객체 하나만 반환하세요."
        ),
    });
    retry
}

#[derive(serde::Deserialize)]
struct ModelListResponse {
    data: Vec<ModelListItem>,
}

#[derive(serde::Deserialize)]
struct ModelListItem {
    id: String,
}

/// Keep the input side of a request within the configured soft context budget.
/// The output reservation is `request.max_tokens`; the remaining budget is
/// applied conservatively as a character count, which is intentionally safer
/// for Korean text than assuming one token per whitespace-delimited word.
fn bound_context(request: &TurnRequest, context_budget_tokens: u32) -> TurnRequest {
    let mut bounded = request.clone();
    let output_reserve = request
        .max_tokens
        .min(context_budget_tokens.saturating_sub(1));
    let input_budget = context_budget_tokens.saturating_sub(output_reserve).max(1) as usize;
    let used = bounded
        .messages
        .iter()
        .map(|message| message.content.chars().count())
        .sum::<usize>();
    if used <= input_budget {
        return bounded;
    }

    let system_chars = bounded
        .messages
        .iter()
        .take(bounded.messages.len().saturating_sub(1))
        .map(|message| message.content.chars().count())
        .sum::<usize>();
    let user_budget = input_budget.saturating_sub(system_chars);
    if let Some(last) = bounded.messages.last_mut() {
        last.content = truncate_context(&last.content, user_budget);
    }
    bounded
}

fn truncate_context(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    const MARKER: &str = "\n...[중략: context budget]...\n";
    if max_chars <= MARKER.chars().count() + 2 {
        return input.chars().take(max_chars).collect();
    }
    let available = max_chars - MARKER.chars().count();
    let head = available / 2;
    let tail = available - head;
    let prefix: String = input.chars().take(head).collect();
    let suffix: String = input
        .chars()
        .rev()
        .take(tail)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("{prefix}{MARKER}{suffix}")
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
        let result = self
            .authorize(self.client.get(self.health_url()))
            .send()
            .await;
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
        match self.do_generate(&request, false, &cancel, None).await {
            Ok(draft)
                if self.config.min_narration_chars > 0
                    && narration_char_count(&draft) < self.config.min_narration_chars =>
            {
                let first_len = narration_char_count(&draft);
                tracing::info!(
                    target: "story_provider::openai_compatible",
                    generation_id = %request.generation_id,
                    model = %self.config.model,
                    first_narration_chars = first_len,
                    min_narration_chars = self.config.min_narration_chars,
                    "short valid scene; requesting one best-effort expansion"
                );
                let retry = length_repair_request(&request, &draft, self.config.min_narration_chars);
                match self
                    .do_generate(&retry, false, &cancel, Some(self.config.temperature))
                    .await
                {
                    Ok(expanded) if narration_char_count(&expanded) >= first_len => Ok(expanded),
                    Ok(_) => Ok(draft),
                    Err(error) => {
                        tracing::warn!(
                            target: "story_provider::openai_compatible",
                            generation_id = %request.generation_id,
                            error = %error,
                            "length expansion failed; keeping the first valid scene"
                        );
                        Ok(draft)
                    }
                }
            }
            Ok(draft) => Ok(draft),
            // Auto-fallback is only for a server that ignored or could not
            // produce the JSON envelope at all. A `schema:`/`validation:`
            // failure means structured output did run, but the model produced
            // a value that violates the game contract; dropping the schema for
            // that retry makes the next answer less reliable.
            Err(ProviderError::InvalidGeneration(detail))
                if self.config.structured_output == StructuredOutputMode::Auto
                    && request.json_schema.is_some()
                    && self.structured_enabled()
                    && is_json_envelope_failure(&detail) =>
            {
                tracing::warn!(
                    target: "story_provider::openai_compatible",
                    "structured-output parse failure; retrying without response_format"
                );
                self.disable_structured();
                self.do_generate(&request, true, &cancel, Some(0.2)).await
            }
            // Keep constrained decoding for a contract-level retry. The
            // bounded retry runs at the deterministic lower temperature and
            // cannot recurse through this outer `generate_turn` match.
            Err(ProviderError::InvalidGeneration(detail))
                if self.config.structured_output == StructuredOutputMode::Auto
                    && request.json_schema.is_some()
                    && self.structured_enabled() =>
            {
                tracing::warn!(
                    target: "story_provider::openai_compatible",
                    error = %detail,
                    "structured-output contract failure; retrying with response_format"
                );
                self.do_generate(&request, false, &cancel, Some(0.2)).await
            }
            Err(ProviderError::InvalidGeneration(_))
                if self.config.structured_output == StructuredOutputMode::On =>
            {
                // Free-form JSON is still stochastic on local Qwen/MLX
                // servers. Retry once at a deterministic temperature before
                // surfacing a player-facing format error.
                self.do_generate(&request, true, &cancel, Some(0.2)).await
            }
            Err(ProviderError::InvalidGeneration(detail))
                if detail.contains("duplicate choice labels") =>
            {
                tracing::warn!(
                    target: "story_provider::openai_compatible",
                    "duplicate choices; retrying once with an safe variation reminder"
                );
                let mut repair_request = request.clone();
                repair_request.messages.push(story_model::ChatMessage {
                    role: "user".to_string(),
                    content: "중요: 선택지 3개는 문장과 전략이 서로 달라야 합니다. 같은 문장을 반복하지 말고, 정면 대응·신중한 탐색·유보 중 서로 다른 방향으로 다시 JSON만 반환하세요.".to_string(),
                });
                self.do_generate(&repair_request, true, &cancel, Some(0.3))
                    .await
            }
            Err(other) => Err(other),
        }
    }

    /// Director planner (W10, ST-033): non-streaming POST + strict parse into
    /// [`DirectorPlan`]. 4xx is NOT retried; 5xx / connect / timeout retry up
    /// to `max_retries` with bounded exponential backoff; `cancel` is honored
    /// before every attempt AND raced against the in-flight request — same
    /// contract as [`Self::generate_turn`].
    async fn direct(
        &self,
        request: DirectorRequest,
        cancel: CancellationToken,
    ) -> Result<DirectorPlan, ProviderError> {
        let model_name = if request.model.is_empty() || request.model == "mock" {
            &self.config.model
        } else {
            &request.model
        };
        let body = ChatCompletionRequest::from_director(&request, model_name);
        let max_attempts = self.config.max_retries.saturating_add(1);

        for attempt in 0..max_attempts {
            if cancel.is_cancelled() {
                return Err(ProviderError::Cancelled);
            }
            let send = self
                .authorize(self.client.post(self.chat_url()))
                .json(&body)
                .send();
            let result = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
                r = send => r,
            };

            let resp = match result {
                Ok(r) => r,
                Err(e) => {
                    if attempt + 1 < max_attempts {
                        Self::sleep_or_cancel(attempt, &cancel).await?;
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
                if attempt + 1 < max_attempts {
                    Self::sleep_or_cancel(attempt, &cancel).await?;
                    continue;
                }
                return Err(ProviderError::Protocol(format!("HTTP {}", status.as_u16())));
            }

            let parsed: crate::envelope::ChatCompletionResponse = match resp.json().await {
                Ok(p) => p,
                Err(e) => {
                    return Err(ProviderError::Protocol(format!(
                        "response decode failed: {e}"
                    )));
                }
            };
            let content = parsed.content();
            let plan = crate::recovery::parse_director_plan(&content)?;
            return Ok(plan);
        }
        Err(ProviderError::Protocol(
            "retry budget exhausted".to_string(),
        ))
    }

    /// Streaming Writer (W10, ST-034): POST with `stream:true`, parse the SSE
    /// byte stream via [`SseParser`], forward each delta as a
    /// [`WriterEvent::Chunk`] to `sink`, coalesce the full text, and return a
    /// [`WriterFinish`] on `[DONE]`. On cancel / disconnect / late chunk the
    /// method stops and discards any pending text (returns `Cancelled` /
    /// `Protocol`). `cancel` is honored via `tokio::select!`.
    async fn write_stream(
        &self,
        request: WriterRequest,
        sink: Sender<WriterEvent>,
        cancel: CancellationToken,
    ) -> Result<WriterFinish, ProviderError> {
        let model_name = if request.model.is_empty() || request.model == "mock" {
            &self.config.model
        } else {
            &request.model
        };
        let body = ChatCompletionRequest::from_writer(&request, model_name);
        let generation_id = request.generation_id;

        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }

        // Single attempt — once the stream has started we cannot transparently
        // retry (chunks may have already been surfaced to the UI). Pre-stream
        // errors (connect / 4xx / 5xx) are surfaced immediately.
        let send = self
            .authorize(self.client.post(self.chat_url()))
            .json(&body)
            .send();
        let resp = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
            r = send => r.map_err(|e| map_send_error(e, self.config.request_timeout_ms))?,
        };

        let status = resp.status();
        if !status.is_success() {
            return Err(ProviderError::Protocol(format!("HTTP {}", status.as_u16())));
        }

        let mut stream = resp.bytes_stream();
        let mut parser = SseParser::new();
        let mut full_text = String::new();
        let mut usage: Option<TokenUsage> = None;
        let mut saw_done = false;

        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    // Cancel discards any buffered / in-flight text.
                    return Err(ProviderError::Cancelled);
                }
                chunk = stream.next() => {
                    match chunk {
                        None => break,
                        Some(Err(e)) => {
                            return Err(map_send_error(e, self.config.request_timeout_ms));
                        }
                        Some(Ok(bytes)) => {
                            for ev in parser.feed(bytes.as_ref()) {
                                match ev {
                                    SseEvent::Data(payload) => {
                                        if let Some(parsed) = parse_stream_delta(&payload) {
                                            if let Some(text) = parsed.text
                                                && !text.is_empty()
                                            {
                                                // Forward to the sink; ignore a full
                                                // channel (the host may have dropped the
                                                // receiver). Coalesce regardless.
                                                let _ = sink.try_send(WriterEvent::Chunk {
                                                    text: text.clone(),
                                                });
                                                full_text.push_str(&text);
                                            }
                                            if let Some(u) = parsed.usage {
                                                usage = Some(u);
                                            }
                                        }
                                        // Unknown payloads (keep-alive comments,
                                        // unparseable deltas) are ignored.
                                    }
                                    SseEvent::Done => {
                                        saw_done = true;
                                        break;
                                    }
                                    SseEvent::Incomplete => {}
                                }
                            }
                            if saw_done {
                                break;
                            }
                        }
                    }
                }
            }
        }

        if !saw_done {
            // Stream ended without `[DONE]` — treat as a protocol error so the
            // host surfaces a recoverable error rather than partial text.
            return Err(ProviderError::Protocol(
                "stream ended without [DONE]".to_string(),
            ));
        }

        // Best-effort final notification; a dropped receiver is not an error.
        let _ = sink.try_send(WriterEvent::Done {
            usage: usage.clone(),
        });

        Ok(WriterFinish {
            generation_id,
            full_text,
            usage,
        })
    }
}

// ---------------------------------------------------------------------------
// OpenAI streaming-delta payload parsing
// ---------------------------------------------------------------------------

/// Decoded payload from one OpenAI streaming chunk: the delta text (if any) and
/// the optional per-chunk / final usage. `None` if the payload is unparseable
/// or carries neither (e.g. a keep-alive chunk).
struct ParsedDelta {
    text: Option<String>,
    usage: Option<TokenUsage>,
}

/// Serde-tolerant decoder for one OpenAI streaming `data:` payload. Unknown
/// fields are ignored; missing `choices` / `delta` / `content` default to
/// empty.
fn parse_stream_delta(payload: &str) -> Option<ParsedDelta> {
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct StreamChunk {
        #[serde(default)]
        choices: Vec<StreamChoice>,
        #[serde(default)]
        usage: Option<StreamUsage>,
    }
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct StreamChoice {
        #[serde(default)]
        delta: Option<StreamDelta>,
    }
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct StreamDelta {
        #[serde(default)]
        content: Option<String>,
    }
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct StreamUsage {
        #[serde(default)]
        prompt_tokens: Option<u32>,
        #[serde(default)]
        completion_tokens: Option<u32>,
        #[serde(default)]
        total_tokens: Option<u32>,
    }
    impl StreamUsage {
        fn to_token_usage(&self) -> Option<TokenUsage> {
            let prompt = self.prompt_tokens?;
            let completion = self.completion_tokens.unwrap_or(0);
            let total = self.total_tokens.unwrap_or(prompt + completion);
            Some(TokenUsage {
                prompt_tokens: prompt,
                completion_tokens: completion,
                total_tokens: total,
            })
        }
    }

    let parsed: StreamChunk = serde_json::from_str(payload).ok()?;
    let text = parsed
        .choices
        .first()
        .and_then(|c| c.delta.as_ref())
        .and_then(|d| d.content.clone());
    let usage = parsed.usage.and_then(|u| u.to_token_usage());
    Some(ParsedDelta { text, usage })
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

    #[test]
    fn context_budget_truncates_only_when_input_exceeds_budget() {
        let request = TurnRequest {
            generation_id: story_model::uuid::Uuid::nil(),
            messages: vec![
                story_model::ChatMessage {
                    role: "system".to_string(),
                    content: "system".to_string(),
                },
                story_model::ChatMessage {
                    role: "user".to_string(),
                    content: "가나다라마바사아자차카타파하가나다라마바사아자차카타파하가나다라마바사아자차카타파하가나다라마바사아자차카타파하".to_string(),
                },
            ],
            model: "test".to_string(),
            temperature: 0.2,
            top_p: 0.9,
            top_k: 0,
            max_tokens: 2,
            json_schema: None,
        };
        let unchanged = bound_context(&request, 100);
        assert_eq!(unchanged.messages[1].content, request.messages[1].content);
        let bounded = bound_context(&request, 50);
        assert!(bounded.messages[1].content.chars().count() <= 48);
        assert!(bounded.messages[1].content.contains("중략"));
    }
}
