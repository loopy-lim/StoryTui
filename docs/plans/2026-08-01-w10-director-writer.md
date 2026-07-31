# W10 Director/Writer Split & SSE Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Separate structural planning (Director) from Korean prose (Writer): the Director returns a validated plan (non-streaming JSON) whose patch is reducer-dry-run-confirmed BEFORE the Writer streams body text (SSE) that realizes the plan. The Writer has NO authority to emit patches/choices/routes/endings; partial stream text never enters history/save until confirmed. ST-033 (DirectorPlan + validation), ST-034 (Writer SSE + commit). **ADR-005: the 2-stage path is code-complete but NOT the default until a real 20-turn A/B gate passes — the MVP single-call path stays default.**

**Architecture:** `DirectorPlan` + DTOs live in `story-model`. Director validation + Writer commit logic live in `story-core`. SSE parsing + the `direct()`/`write_stream()` provider impls live in `story-provider` (extending `OpenAiCompatibleProvider`). The engine gains an OPTIONAL 2-stage turn path (off by default).

**Tech Stack:** story-provider (reqwest, tokio, futures-util for the SSE stream), story-core (+ sha2), story-model. ADR-003 unchanged.

---

## Part A — story-model: Director/Writer DTOs (extend, new `director.rs`)

`DirectorPlan` (Technical Spec §10.5):
```rust
pub struct DirectorPlan {
    pub scene_goal: String,
    pub required_beats: Vec<String>,
    pub emotion_arcs: BTreeMap<CharacterId, EmotionArc>,
    pub reveal_fact_ids: Vec<String>,
    pub must_not_reveal_fact_ids: Vec<String>,
    pub proposed_patch: StatePatch,
    pub choices: Vec<Choice>,
    pub conflict_checks: Vec<ConflictCheck>,
    pub end_condition: String,
}
pub struct EmotionArc { pub character: CharacterId, pub start: Emotion, pub end: Emotion, pub reason: Option<String> }
pub struct ConflictCheck { pub id: String, pub description: String, pub resolved: bool }
```
Provider DTOs:
```rust
pub struct DirectorRequest { pub generation_id: Uuid, pub messages: Vec<ChatMessage>, pub model: String, /* + sampling */ }
pub struct WriterRequest { pub generation_id: Uuid, pub messages: Vec<ChatMessage>, pub model: String, /* + sampling, stream:true */ pub plan_digest: String }
pub enum WriterEvent { Chunk(String), Done(Option<TokenUsage>), Error(String) }
pub struct WriterFinish { pub generation_id: Uuid, pub full_text: String, pub usage: Option<TokenUsage> }
pub struct WriterLintIssue { pub code: String, pub detail: String }
```
serde round-trip tests.

## Part B — story-core: Director validation + Writer commit (new `director.rs`, `writer.rs`)

`director.rs`:
```rust
pub fn validate_plan(plan: &DirectorPlan, state: &GameState, pack: &StoryPack) -> Result<ValidatedPlan, CoreError>;
```
Checks:
- `proposed_patch` passes the W3 validator (reference/range/permission/transition) — reuse `validator::validate`.
- `must_not_reveal_fact_ids` are NOT in `reveal_fact_ids` and are not already-revealed-known facts inconsistent with knowledge scope (no revealing a secret the character shouldn't know).
- `emotion_arcs` reference existing characters; `end` is a valid `Emotion`.
- `choices` count/shape valid (reuse parser/validator shape checks).
- `end_condition` non-empty.
- The plan's patch is then **dry-run** through `reducer::apply_patch` on a clone — if it would fail, reject the plan (Writer is NOT called).
Returns the sanitized plan + the dry-run result (so the Writer knows the confirmed patch without re-judging).

`writer.rs`:
```rust
pub fn finalize_writer_output(text: String, plan: &ValidatedPlan, generation_id: Uuid) -> Result<WriterResult, CoreError>;
pub struct WriterResult { pub narration: String, pub lint: Vec<WriterLintIssue> }
```
- The Writer's text becomes `narration` ONLY. It does NOT carry a patch/choices — those come from the confirmed Director plan.
- **Lint:** detect forbidden.Writer attempts (the Writer text should NOT contain embedded JSON patch/choices — if it does, flag a `WriterLintIssue` and strip/ignore; never apply). Light MVP lint: warn if the text contains a fenced JSON block.
- Partial/unconfirmed text is NEVER placed in history or save.

## Part C — story-provider: SSE + provider impls (new `sse.rs`, extend `openai_compatible.rs`)

`sse.rs` — SSE byte/line parser:
```rust
pub struct SseParser { buffer: Vec<u8>, done: bool }
pub enum SseEvent { Data(String), Done, Incomplete }
impl SseParser {
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseEvent>;   // splits on \n\n; decodes `data: ` lines; [DONE] → Done; UTF-8 boundary-safe (accumulate partial multi-byte)
}
```
- **UTF-8 boundary safety:** never split a multi-byte UTF-8 sequence across chunks — buffer incomplete trailing bytes (use `std::str::from_utf8` on the accumulated buffer; if it fails at the tail, retain the partial).
- `[DONE]` sentinel → `SseEvent::Done`.
- Coalescing at 16–33ms is the consumer's job (the provider yields `WriterEvent::Chunk` per data line; the UI coalesces).

`openai_compatible.rs` — implement the two trait methods:
- `direct(DirectorRequest) -> DirectorPlan`: POST `/v1/chat/completions` non-streaming, parse JSON into `DirectorPlan` (strict; reuse a director parse path — if it fails, `InvalidGeneration`). Retry/cancel rules same as `generate_turn`. 4xx no-retry.
- `write_stream(WriterRequest, sink: Sender<WriterEvent>, cancel) -> WriterFinish`: POST with `stream:true`, read the body as a byte stream (`reqwest::Response::bytes_stream()`), feed an `SseParser`, forward `WriterEvent::Chunk`s to the sink, coalesce the full text, return `WriterFinish` on `[DONE]`. On cancel/disconnect/late-chunk: stop, return `Err(Cancelled)` / propagate; any text after cancel is discarded. Honor `cancel` via `tokio::select!`.

(The trait's MVP `direct`/`write_stream` currently return UnsupportedCapability — replace those default bodies with the real impls on `OpenAiCompatibleProvider`.)

## Part D — engine 2-stage path (optional, off by default) (extend `engine.rs`)

Add an opt-in 2-stage command path (gated by `EngineState.use_director_writer: bool`, default `false`):
- When on, `SubmitChoice` → `RequestDirector` effect → app calls `provider.direct()` → `AcceptDirectorPlan` → engine validates+dry-runs → `RequestWriter` → app calls `provider.write_stream()` (streaming chunks → `PublishView` partial) → `AcceptWriterFinish` → engine `finalize_writer_output` + commit the confirmed plan patch + append history (narration from Writer, patch/choices from Director).
- When off (default), the existing single-call MVP path is unchanged.
- **Invariants:** Writer can never change state; partial text never committed; if Director plan fails validation, Writer is never requested.

Tests (ST-033/034): Director plan with forbidden reveal rejected; invalid patch plan rejected (Writer not called); SSE parser splits UTF-8 safely (feed a multi-byte char split across two chunks); `[DONE]` terminates; cancel mid-stream discards late chunks; Writer text with embedded JSON patch → lint + not applied; 2-stage off-by-default (single-call path still the default).

## Tasks (TDD)
- **W10.1 (story-model):** Director/Writer DTOs + serde. Commit `feat(story-model): Director/Writer DTOs`.
- **W10.2 (story-core):** `director.rs` validate_plan + dry-run; `writer.rs` finalize + lint. Commit `feat(story-core): Director validation and Writer finalize/lint`.
- **W10.3 (story-provider):** `sse.rs` UTF-8-safe parser + tests; impl `direct`/`write_stream` on the provider. Commit `feat(story-provider): SSE parser and Director/Writer provider methods`.
- **W10.4 (story-core):** optional 2-stage engine path (off by default) + invariants. Commit `feat(story-core): optional Director/Writer turn path`.

## Completion gate (ST-033/034) — code/contract only
- `cargo test --workspace` green; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --check` clean.
- `./scripts/check-core-deps.sh` OK; `cargo tree -p story-core` unchanged.
- SSE parser UTF-8-safe; `[DONE]` handled; cancel discards late chunks (tested with a mock SSE stream, no real server).
- Director plan validated + dry-run before Writer; Writer can't change state; partial text never committed.
- 2-stage path OFF by default (MVP single-call unchanged).
- **Explicitly NOT done (runtime/ADR-005 gate):** the real 20-turn A/B that would make 2-stage the default; real streaming latency measurement.
