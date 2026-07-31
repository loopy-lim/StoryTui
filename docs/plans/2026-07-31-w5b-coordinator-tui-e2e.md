# W5 Tranche 2: Coordinator, Story Screen & E2E Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Wire the engine into a playable vertical slice: the Elm-style `EngineCommand → Transition` boundary with checkpoints + late-response discard, a Story screen rendering the view, and a rainy-reunion 3-turn Mock end-to-end test. ST-020 (coordinator/cancel), ST-021 (Story screen vertical slice).

**Architecture:** `story-core` owns the pure engine: `reduce(state, command, pack) -> Result<Transition, CoreError>` returning `EngineEffect`s (RequestGeneration / PublishView / ShowRecoverableError). `AppViewModel` (pure data, no ratatui) lives in `story-model` and is what the engine publishes. `story-tui` renders `AppViewModel` into ratatui widgets and maps keys to `EngineCommand`s. `apps/storytui` runs the event loop: it executes `RequestGeneration` by calling the provider (Mock for now) and feeds the result back as `AcceptGeneration`. ADR-003 unchanged: story-core has no UI/HTTP/FS; it only *describes* effects.

**Tech Stack:** story-core (+ existing sha2), story-tui (ratatui 0.30.2, crossterm 0.29.0), apps/storytui (tokio), insta for TUI snapshots.

---

## Part A — story-model: view model (new file `view.rs`)

```rust
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppViewModel {
    pub screen: Screen,
    pub story_title: String,
    pub chapter: String,
    pub location_display: String,
    pub time_slot: TimeSlot,
    pub narration: String,
    pub choices: Vec<ChoiceView>,
    pub status: StatusLine,
    pub loading: Option<LoadingState>,
    pub error: Option<DomainErrorView>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Screen { Story, FreeInput, History, Character, GameState, SaveLoad, Settings, ErrorDetail }
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChoiceView { pub id: String, pub label: String, pub risk: RiskLevel, pub preview: Option<String> }
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StatusLine { pub route: Option<String>, pub affection: Option<i16>, pub trust: Option<i16>, pub tension: Option<i16>, pub turn: u32 }
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoadingState { pub generation_id: uuid::Uuid, pub message_ko: String }
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DomainErrorView { pub code: String, pub title_ko: String, pub message_ko: String, pub recoverable: bool }
```
Re-export; serde round-trip test.

## Part B — story-core: engine boundary (new files `engine_cmd.rs`, `engine.rs`)

### engine_cmd.rs
```rust
pub enum EngineCommand {
    StartNewGame,
    SubmitChoice { choice_id: String },
    AcceptGeneration { draft: GeneratedTurnDraft },
    CancelGeneration { generation_id: uuid::Uuid },
    DismissError,
    Quit,
}
pub enum EngineEffect {
    RequestGeneration { generation_id: uuid::Uuid, request: TurnRequest },
    PublishView(AppViewModel),
    ShowRecoverableError(DomainErrorView),
}
pub struct Transition { pub state: EngineState, pub effects: Vec<EngineEffect>, pub audit: Vec<StateChangeLog> }
```
`StartNewGame` initializes from the Pack; `SubmitFreeAction`/`RegenerateCurrentTurn`/`RestoreCheckpoint` are deferred to W7 (do not implement — out of Tranche 2 scope).

### EngineState (engine.rs)
```rust
pub struct EngineState {
    pub game: GameState,
    pub pack_metadata: PackMetadata,        // title/chapter etc. for the view
    pub pending: Option<PendingGeneration>,
}
pub struct PendingGeneration {
    pub generation_id: uuid::Uuid,
    pub checkpoint: GameState,              // pre-choice stable state to restore on cancel/failure
    pub player_choice_id: String,
    pub requested_at_turn: u32,
}
```

### `reduce(state: &EngineState, cmd: EngineCommand, pack: &StoryPack) -> Result<Transition, CoreError>`
- **StartNewGame:** build initial `GameState` from `pack.metadata.initial_state` + characters (affection/trust/tension 0, `relationship_stage` from `initial_player_relationship`, emotion Neutral). `pending = None`. Effects: `[PublishView(opening view)]`. The opening narration = `pack.opening_md` (or a first generation — for the slice, seed the opening from `opening_md` and present the FIRST set of choices via an initial generation request). **Decision:** StartNewGame publishes the opening view (narration = opening_md, empty choices) AND emits `RequestGeneration` for turn 1's choices so the player immediately gets 3 choices. Keep it simple but playable.
- **SubmitChoice { choice_id }:** validate `choice_id` is among the current view's choices (else `Err`). Create `PendingGeneration { generation_id: Uuid::new_v4(), checkpoint: state.game.clone(), player_choice_id, requested_at_turn: state.game.turn }`. Build the prompt via `prompt_context::build_prompt(pack, &state.game, recent_from_history)` and a `TurnRequest`. New state: `pending = Some(...)`, game unchanged. Effects: `[RequestGeneration{generation_id, request}, PublishView(loading view)]`.
- **AcceptGeneration { draft }:**
  - If `state.pending` is None OR `draft.generation_id != pending.generation_id` → **late/mismatched: discard**. Return a Transition with NO state change and an empty/minor effect (e.g. PublishView(current view) or none). This is the late-response-discard invariant.
  - Else: run `reducer::apply_patch(&pending.checkpoint, &pack.rules, &draft.proposed_patch, draft.generation_id)`.
    - On Ok: build next `GameState` = result.state; bump `turn` by 1; append a `HistoryEntry` { turn, scene_id, narration: draft.narration, player_action: None (choice recorded via player_choice_id — store choice label in history.choices or a note), choices: draft.choices, applied_patch: result's sanitized/accepted patch, generation_id }; `active_route`/`ending` from result; clear `pending`. Effects: `[PublishView(story view with new narration + draft.choices + status), ]` (PersistStableState is W8 — omit the real save effect for now; you may emit a no-op or skip).
    - On Err: restore `game = pending.checkpoint.clone()` (keep checkpoint), clear pending, Effects: `[ShowRecoverableError(DomainErrorView from the CoreError), PublishView(checkpoint view)]`. **The committed stable state is never corrupted** — checkpoint preserved.
- **CancelGeneration { generation_id }:** if matches pending → `game = pending.checkpoint.clone()`, clear pending, `[PublishView(checkpoint view)]`. Else no-op.
- **DismissError:** clear error, `[PublishView]`.
- **Quit:** terminal-only (the app handles it; engine returns current state).

View construction helper `build_view(state, pack, loading, error) -> AppViewModel` reads display names from `pack.rules.locations`, title from `pack.metadata.title`, status from `game.characters` (first/primary character's stats), `game.active_route`, `game.turn`.

**Invariants (tested):**
- Late/mismatched `AcceptGeneration` never mutates `game` (compare serialized state before/after).
- A failed `apply_patch` on AcceptGeneration restores the checkpoint exactly (byte-identical) and never leaves a partial commit.
- `CancelGeneration` restores the checkpoint.
- No two generations can be in flight: a second `SubmitChoice` while `pending.is_some()` is rejected (`Err`).
- Turn increments by exactly 1 per accepted generation.

## Part C — story-tui: Story screen (new `screens/story.rs`, `app.rs`, `event.rs`)

- `app.rs`: `App { engine_state, view: AppViewModel, selected: usize, should_quit: bool }` with `handle_key(key) -> Option<EngineCommand>` and `render(frame, area)`.
- `screens/story.rs`: render title/chapter/location/time (status line), narration (wrapped, Korean width via `unicode-width`), 3 choices as a selectable list with risk indicators, a status bar (route/affection/trust/tension/turn), and loading/error overlays. Use `ratatui::text::Line` with `unicode-width` for Korean wrapping.
- `event.rs`: key mapping per the Technical Spec table — `↑/↓`,`j/k` move selection; `Enter`/`1..3` confirm choice; `c` cancel generation; `Esc` back/dismiss error; `q`/`Ctrl+C` quit. (Map only the Story-screen keys needed for the slice; full keymap is later.)
- **Snapshot tests (insta):** render the Story view at 80×24 and 120×40 into a `ratatui::Terminal::new(Buffer::with_dimensions)` and assert snapshots for: normal story, loading overlay, error overlay, below-min-size warning. These run headless (no real terminal).

## Part D — apps/storytui: event loop + 3-turn Mock E2E

- `main.rs`: minimal — for the slice, a `#[tokio::test]`-style or `#[test]` harness that drives the engine with a `MockProvider` (the real `main()` can stay a stub or a minimal loop; a full interactive main is nice-to-have, the E2E test is the gate).
- **E2E test** (`apps/storytui/tests/mock_e2e.rs` or `tests/contracts/mock_e2e.rs`): load `stories/rainy-reunion`, init engine, drive 3 turns: StartNewGame → (get choices) → SubmitChoice(choice0) → Mock returns a valid GeneratedTurnDraft → AcceptGeneration → assert turn advanced, patch committed, new choices presented, no state corruption. Repeat 3×. Also test: cancel mid-generation restores checkpoint; a malformed Mock response → ShowRecoverableError, stable state intact.

## Tasks (TDD)
- **W5.5 (story-model + story-core):** `view.rs` + `engine_cmd.rs` + `engine.rs` reduce + invariants. Commit `feat(story-core): engine command/effect boundary with checkpoints and late-discard`.
- **W5.6 (story-tui):** Story screen + key mapping + 4 insta snapshots. Commit `feat(story-tui): Story screen with choices, loading and error overlays`.
- **W5.7 (apps/storytui):** Mock-backed 3-turn E2E + cancel/malformed cases. Commit `test(storytui): rainy-reunion 3-turn Mock end-to-end slice`.

## Completion gate (ST-020/021)
- `cargo test --workspace` green; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --check` clean.
- `./scripts/check-core-deps.sh` OK; `cargo tree -p story-core` still `story-model + thiserror + sha2` (no ratatui/crossterm/tokio/reqwest).
- E2E: 3 turns complete; only validated patches commit; checkpoint preserved after cancel/failure; late/mismatched generation discarded.
- TUI snapshots present and stable for story/loading/error/min-size.
- No real HTTP (Mock only). No save-to-disk (W8).
