# W6 Real MLX Provider Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement `OpenAiCompatibleProvider` — the real local-LLM adapter that talks to an OpenAI-compatible HTTP server (MLX-VLM / MLX-LM) at `127.0.0.1`. Code-complete with HTTP-fixture contract tests. ST-017. **Real-server runtime verification (10-turn play, server-command recording, terminal checks) is explicitly OUT of scope here — it needs a live MLX server on Apple Silicon and is recorded as a separate runtime gate, not closed by code/tests alone.**

**Architecture:** `story-provider` gains `OpenAiCompatibleProvider` implementing the existing `LlmProvider` trait (W5 Tranche 1). It builds the OpenAI request envelope from a `TurnRequest`, POSTs to `/v1/chat/completions`, extracts the assistant message body, and runs it through the existing W5.3 `recovery::parse_generated_turn`. Config comes from a `[provider]` TOML block. ADR-002: loopback default, non-loopback warned (not blocked), no telemetry.

**Tech Stack:** reqwest 0.13.4 (rustls), tokio, serde/serde_json, toml, tracing (for the non-loopback warning + redacted diagnostics), an HTTP mock lib as dev-dep for contract tests.

---

## Part A — provider config (story-provider, new `config.rs`)

```rust
pub struct ProviderConfig {
    pub kind: String,                 // "openai_compatible"
    pub name: String,                 // "local-mlx-vlm"
    pub base_url: String,             // "http://127.0.0.1:8080"
    pub chat_path: String,            // "/v1/chat/completions"
    pub health_path: String,          // "/health"
    pub model: String,
    pub temperature: f32,
    pub top_p: f32,
    pub top_k: u32,
    pub max_tokens: u32,
    pub request_timeout_ms: u64,
    pub connect_timeout_ms: u64,
    pub max_retries: u32,
    pub context_budget_tokens: u32,
    pub structured_output: StructuredOutputMode,  // Auto | On | Off
}
#[serde(rename_all="lowercase")] pub enum StructuredOutputMode { Auto, On, Off }
```
Deserialize from the `[provider]` TOML table (Technical Spec §10.2). Provide `ProviderConfig::defaults()` = the spec example (127.0.0.1:8080, temp 0.85, top_p 0.92, top_k 40, max_tokens 1800, timeouts 180000/3000, retries 2, budget 24576, structured_output auto). Serde round-trip test.

## Part B — OpenAI envelope (story-provider, flesh out `envelope.rs`)

Request body (Technical Spec §10.3):
```rust
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub temperature: f32,
    pub top_p: f32,
    pub top_k: u32,                    // some servers reject top_k — send only if >0
    pub max_tokens: u32,
    pub stream: bool,                  // false for MVP
    pub response_format: Option<ResponseFormat>,
}
pub struct ResponseFormat { pub #[serde(rename="type")] kind: String, pub json_schema: Option<JsonSchemaSpec> }
pub struct JsonSchemaSpec { pub name: String, pub strict: bool, pub schema: serde_json::Value }
```
Response extraction: parse `ChatCompletionResponse { choices: [{ message: { content } }], usage: Option<{prompt_tokens,completion_tokens,total_tokens}> }`, return the `content` String + usage. (Do NOT trust arbitrary fields; serde-tolerant on the response.)
- `top_k`: include in JSON only when `> 0` (serialize `Option`/skip_if).
- `response_format`: include only when the provider's capability says structured output is supported.

## Part C — `OpenAiCompatibleProvider` (story-provider, new `openai_compatible.rs`)

```rust
pub struct OpenAiCompatibleProvider {
    id: String,
    config: ProviderConfig,
    client: reqwest::Client,
    caps: ProviderCapabilities,        // discovered; structured_output toggled by Auto probe
}
impl OpenAiCompatibleProvider {
    pub fn new(config: ProviderConfig) -> Result<Self, ProviderError>;   // builds reqwest client with timeouts
    fn chat_url(&self) -> String;
    fn health_url(&self) -> String;
}
#[async_trait] impl LlmProvider for OpenAiCompatibleProvider { ... }
```
- **`new`:** build `reqwest::Client` with `timeout(request)` + `connect_timeout`. **If `base_url` host is not loopback (not 127.0.0.1/localhost), emit a `tracing::warn!`** ("non-loopback provider URL — 외부 전송 가능성"). Do NOT block. No redirects by default (`redirect(Policy::none())`).
- **`capabilities`:** returns caps; `supports_structured_output` resolved from `structured_output` mode (On→true, Off→false, Auto→probed on first use / health).
- **`health`:** GET `health_url`. Map status: 200 → `Ready`; connection refused → `Unavailable`; 5xx → `Degraded`; timeout → `Degraded`. (Keep mapping simple + tested.)
- **`generate_turn`:**
  1. Build `ChatCompletionRequest` from the `TurnRequest` (messages, sampling, max_tokens; `response_format` only if caps support it).
  2. POST to `chat_url` with the configured timeout. **Retry policy:** on connection errors / 5xx / timeout, retry up to `max_retries` with backoff; **HTTP 4xx MUST NOT be retried** (return `Protocol`/`InvalidGeneration` immediately). Honor the `cancel` token between attempts (`tokio::select!`).
  3. On 200: extract `content`, run `recovery::parse_generated_turn(&content, request.generation_id)`. On parse failure after recovery, if `structured_output` was `Auto` and the server may have ignored `response_format`, you may retry once WITHOUT `response_format` then parse again (this is the "disable capability for the session" fallback). Otherwise return `InvalidGeneration`.
  4. Return the `GeneratedTurnDraft`.

## Part D — tests

- **HTTP-fixture contract tests** using a mock HTTP server (add `wiremock = "0.6"` OR `httpmock = "0.7"` as a `[dev-dependencies]` — pick one, justify briefly). Tests:
  - Happy path: mock `/v1/chat/completions` returns a valid envelope → `generate_turn` returns the parsed draft.
  - `/health` 200 → Ready; refused → Unavailable; 500 → Degraded.
  - 4xx → no retry, returns error promptly (assert mock received exactly 1 request).
  - 5xx then 200 → retried once, succeeds (assert 2 requests).
  - Timeout/cancel → returns Cancelled/Timeout, stops retrying.
  - Non-loopback base_url → `tracing` warning emitted (capture via a test subscriber or assert a flag).
  - Malformed 200 body → recovery parser runs; if unparseable → InvalidGeneration.
- **`#[ignore]` real-MLX smoke** (`tests/mlx_smoke.rs`): a test gated behind `--ignored` that, if `STORYTUI_MLX_URL` env var is set, hits the real server for `/health`, `/v1/models`, one Korean chat completion, and asserts valid JSON. If unset, the test is skipped (not failed). Document that this is the runtime gate, NOT run in normal CI.

## Tasks (TDD)
- **W6.1:** `config.rs` (ProviderConfig + defaults + serde). Commit `feat(story-provider): provider TOML config`.
- **W6.2:** `envelope.rs` request/response types + serde (top_k skip-if, response_format optional). Commit `feat(story-provider): OpenAI-compatible request/response envelope`.
- **W6.3:** `openai_compatible.rs` provider impl + `LlmProvider` (health, generate_turn, retry/cancel, loopback warn). Commit `feat(story-provider): OpenAiCompatibleProvider against local MLX server`.
- **W6.4:** HTTP-fixture contract tests (happy/health/4xx-no-retry/5xx-retry/cancel/non-loopback/malformed) + `#[ignore]` real-MLX smoke stub. Commit `test(story-provider): OpenAI-compatible HTTP contract and MLX smoke gate`.

## Completion gate (ST-017) — code/contract only
- `cargo test --workspace` green (real-MLX smoke is `#[ignore]`, not run).
- `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --check` clean.
- `./scripts/check-core-deps.sh` OK (story-provider may use reqwest/tokio — that's allowed; only story-core/story-model are HTTP-free).
- HTTP contract tests pass against the mock server with NO real MLX process.
- 4xx not retried; 5xx retried up to `max_retries`; cancel honored; non-loopback warned; loopback default.
- **Explicitly NOT done (runtime gate):** real 10-turn play, server-command/package/revision recording, JSON-validity rate on the candidate model, Terminal.app/iTerm2 checks. These require a live MLX server and are recorded as unverified.
