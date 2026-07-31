# W5 Tranche 1: Provider, Prompt & Parser Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build the model-free, highly-testable core of the vertical slice: the `LlmProvider` trait + `MockProvider`, deterministic `PromptContext` prompt assembly, and the strict `GeneratedTurn` JSON parser with limited recovery. ST-016, ST-018, ST-019. No real model, no network, no TUI yet.

**Architecture:** Cross-boundary DTOs (`TurnRequest`, `GeneratedTurnDraft`, `ProviderHealth`, `ProviderCapabilities`, `ProviderError`) live in `story-model` (pure data, no async). The `LlmProvider` async trait + `MockProvider` + OpenAI envelope + JSON recovery live in `story-provider` (reqwest, tokio, async-trait, tokio-util). `PromptContext` (deterministic prompt + hash) lives in `story-core`. ADR-003 unchanged.

**Tech Stack:** async-trait 0.1.91, tokio 1.53.1, tokio-util 0.7.19, reqwest 0.13.4, sha2 0.11.0, serde/serde_json, insta. story-core still `story-model + thiserror` only.

---

## Part A — story-model: cross-boundary DTOs (new file `provider_dto.rs`)

```rust
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage { pub role: String, pub content: String }   // role: "system"|"user"|"assistant"

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TurnRequest {
    pub generation_id: uuid::Uuid,
    pub messages: Vec<ChatMessage>,
    pub model: String,
    pub temperature: f32,
    pub top_p: f32,
    pub top_k: u32,
    pub max_tokens: u32,
    pub json_schema: Option<serde_json::Value>,  // included only if capability confirmed
}

// What the provider returns BEFORE the engine validates the patch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GeneratedTurnDraft {
    pub generation_id: uuid::Uuid,
    pub narration: String,
    pub choices: Vec<Choice>,
    pub proposed_patch: StatePatch,
    pub summary: TurnSummary,
    pub usage: Option<TokenUsage>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderHealth { Ready, Loading, Unavailable, Degraded, Misconfigured }   // FR-037

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub supports_structured_output: bool,   // response_format json_schema
    pub supports_director: bool,            // MVP: false
    pub supports_streaming: bool,           // MVP: false
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Serialize, Deserialize)]
pub enum ProviderError {
    #[error("provider unavailable: {0}")] Unavailable(String),
    #[error("provider loading: {0}")] Loading(String),
    #[error("protocol error: {0}")] Protocol(String),
    #[error("timeout after {0_ms}ms")] Timeout { ms: u64 },
    #[error("cancelled")] Cancelled,
    #[error("unsupported capability: {0}")] UnsupportedCapability(String),
    #[error("invalid generation: {0}")] InvalidGeneration(String),
}
```
Re-export from `lib.rs`. serde round-trip tests. (`thiserror::Error` on a Serialize type is fine.)

## Part B — story-provider: `LlmProvider` trait + `MockProvider`

File layout `crates/story-provider/src/`:
```
lib.rs
provider.rs      — #[async_trait] LlmProvider
mock.rs          — MockProvider
envelope.rs      — OpenAI-compatible request/response envelope (used by the W6 real provider; Mock bypasses it)
recovery.rs      — JSON parse + repair
error.rs         — re-export ProviderError (from story-model) + any provider-internal conversions
```

### provider.rs (ST-016)
```rust
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    fn id(&self) -> &str;
    fn capabilities(&self) -> ProviderCapabilities;
    async fn health(&self) -> Result<ProviderHealth, ProviderError>;
    async fn generate_turn(
        &self,
        request: TurnRequest,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<GeneratedTurnDraft, ProviderError>;
    // MVP: these two return UnsupportedCapability. Real impls come in W6/W10.
    async fn direct(&self, _req: (), _cancel: CancellationToken) -> Result<(), ProviderError> {
        Err(ProviderError::UnsupportedCapability("director".into()))
    }
    async fn write_stream(&self, _req: (), _sink: (), _cancel: CancellationToken) -> Result<(), ProviderError> {
        Err(ProviderError::UnsupportedCapability("streaming".into()))
    }
}
```
(`direct`/`write_stream` signatures are placeholder for MVP — they return UnsupportedCapability. Use `()` params with a leading underscore so the trait compiles cleanly; refine in W10. Keep it simple.)

### mock.rs — `MockProvider` (ST-016)
A programmable fixture provider for tests. Holds a queue of `MockResponse`s and a generation-id policy.
```rust
pub enum MockResponse {
    Draft(GeneratedTurnDraft),          // success
    Delayed { draft: GeneratedTurnDraft, ms: u64 },   // latency
    Malformed(String),                  // raw bad JSON to exercise recovery/retry
    Timeout { ms: u64 },
    Error(ProviderError),
    Late { draft: GeneratedTurnDraft, ms: u64 },      // completes AFTER a typical cancel window
}
pub struct MockProvider { id: String, queue: Mutex<VecDeque<MockResponse>>, health: ProviderHealth, caps: ProviderCapabilities }
```
- `generate_turn`: pop next response. For `Delayed`/`Late`, `tokio::select!` between `tokio::time::sleep` and `cancel.cancelled()` → on cancel return `ProviderError::Cancelled` and the late draft is DISCARDED (never returned). For `Timeout`, sleep then return `Timeout`. For `Malformed`, return the raw string via `Err(ProviderError::InvalidGeneration(raw))` OR (better for parser tests) — actually the parser is separate; Mock returns a `Draft` whose fields were parsed by `recovery::parse` already. Keep Mock focused on transport behavior (success/delay/timeout/cancel/late/error); the recovery module is tested against raw strings directly.
- `health()` returns the configured health.
- Thread-safe (`Send + Sync`) via `tokio::sync::Mutex` or `std::sync::Mutex` (queue is brief-hold; std Mutex is fine and avoids async-lock overhead).

### recovery.rs — strict parse + limited repair (ST-019)
```rust
pub struct ParseOutcome { pub draft: GeneratedTurnDraft, pub repairs: Vec<String> }
pub fn parse_generated_turn(raw: &str, generation_id: Uuid) -> Result<ParseOutcome, ProviderError>;
```
Steps:
1. Strip markdown code fences (` ```json ... ``` ` / ` ``` `) and leading/trailing prose if the JSON object can be located (find first `{` .. matching last `}`).
2. `serde_json::from_str` into a strict DTO (`narration`, `choices[]`, `state_patch`, `summary`). Use `StatePatch`'s `deny_unknown_fields` (so unknown fields reject — do NOT silently drop).
3. Limited repair (each repair recorded in `repairs`): strip trailing commas; unescape wrapped quotes. ONE repair attempt; on second failure → `Err(InvalidGeneration(detail))`. Do NOT loop indefinitely.
4. Validate: choice count 1..=6 (MVP target 3 but allow 2..=4 flex — reject >6 / 0), narration non-empty, `risk`/enum values valid (serde enforces), summary present.
5. Map into `GeneratedTurnDraft`.
**On failure, the raw response is NOT logged by default** — return only a sanitized `detail` (no full raw body). (Logging redaction is the caller's job; here just don't embed raw in the error message beyond a short prefix.)

### Tests (ST-016, ST-019)
- Mock contract: success returns the draft; delayed then succeeds; timeout returns `Timeout`; cancel during delay returns `Cancelled` and the late draft never surfaces; error propagates.
- Parser golden: a valid envelope parses 0 repairs; fenced JSON parses with 1 repair; trailing-comma repairs; unknown field in `state_patch` → `Err` (NOT silently dropped); invalid enum → `Err`; empty narration → `Err`; >6 choices → `Err`; missing `state_patch` → `Err`.
- 12 malformed fixtures under `tests/fixtures/parse/` covering the above.

## Part C — story-core: `PromptContext` (ST-018)

File `crates/story-core/src/prompt_context.rs`.
```rust
pub struct PromptBundle { pub system: String, pub user: String, pub hash: String }
pub fn build_prompt(pack: &StoryPack, state: &GameState, recent: &[HistoryEntry]) -> PromptBundle;
```
- **Deterministic ordering:** iterate characters/flags/numbers in `BTreeMap` key order; recent history in turn order. Same inputs ⇒ identical bytes ⇒ identical hash.
- `hash` = hex SHA-256 of `system + "\n\n---\n\n" + user` (use `sha2`; story-core may NOT add new runtime deps — BUT sha2 is not in story-core's allowed deps). **Resolution:** compute the hash via re-exported `story_model::sha2`? story-model doesn't depend on sha2 either. **Decision:** add `sha2` as a workspace dep available to story-core is FORBIDDEN by ADR-003? No — ADR-003 forbids ratatui/crossterm/reqwest/FS, NOT sha2. sha2 is a pure crypto crate, allowed. Add `sha2 = { workspace = true }` to story-core's `[dependencies]` (it's in `[workspace.dependencies]`). This keeps story-core UI/HTTP/FS-free (verify `check-core-deps.sh` still passes — it only greps ratatui/crossterm/reqwest). 
- System prompt includes: role/safety/output-contract (from `pack.prompts.system`), allowed character/location/flag IDs, allowed delta range (-3..3), choice count, player-sovereignty rule ("do not fix emotions/actions the player didn't specify").
- User prompt includes: world/style/opening markdown excerpts, current state (canonical compact JSON: location, time, characters' affection/trust/tension/emotion/stage, relevant flags, inventory), recent N turns narration+action.
- Golden snapshot test: build_prompt for a fixed (pack, state, recent) ⇒ identical string + stable hash across runs; ordering test (shuffling BTreeMap construction must still produce identical output since we sort).

## Tasks (TDD)
- **W5.1:** story-model `provider_dto.rs` DTOs + serde tests. Commit `feat(story-model): provider cross-boundary DTOs`.
- **W5.2:** story-provider `LlmProvider` trait + `MockProvider` + contract tests (success/delay/timeout/cancel/late/error). Commit `feat(story-provider): LlmProvider trait and MockProvider`.
- **W5.3:** story-provider `recovery.rs` parser + 12 malformed fixtures + repair tests. Commit `feat(story-provider): GeneratedTurn strict parser and limited recovery`.
- **W5.4:** story-core `prompt_context.rs` + golden prompt snapshot + determinism/ordering tests. Commit `feat(story-core): deterministic PromptContext and prompt hash`.

## Completion gate (ST-016/018/019)
- `cargo test --workspace` green; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --check` clean.
- `./scripts/check-core-deps.sh` OK (story-core may now also depend on `sha2` — confirm the script still passes; it only checks ratatui/crossterm/reqwest).
- Mock provider contract tests pass without network/reqwest (Mock uses no HTTP).
- Parser: unknown field rejected (not dropped), 12 fixtures covered, raw response not embedded in errors.
- PromptContext: deterministic (same input → same hash), ordering-independent of map construction.
- No TUI, no coordinator, no AppViewModel yet (those are Tranche 2).
