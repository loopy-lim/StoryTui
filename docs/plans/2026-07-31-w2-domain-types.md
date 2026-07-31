# W2 Domain Types & Serde Contracts Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Define every `story-model` type the rest of the engine shares — IDs, enums, `GameState`, `CharacterState`, `Choice`, `StatePatch`, `GeneratedTurn`, `HistoryEntry`, `MemoryEntry`, `ConditionExpr` — with a fixed serde/version policy and golden-JSON + round-trip tests. `story-model` stays free of UI/HTTP/FS (ADR-003 / NFR-007).

**Architecture:** Pure-data crate. All types derive `Clone, Debug` and `Serialize, Deserialize`. Newtype IDs are `#[serde(transparent)]`. Enums are `#[serde(rename_all = "snake_case")]`. `StatePatch` is `#[serde(deny_unknown_fields)]` (strict — model output). `GameState` is forward-tolerant (no `deny_unknown_fields`) and carries `save_schema_version`. This plan **resolves every type the Technical Spec references but does not define** (the "open design holes").

**Tech Stack:** serde 1.0.229, serde_json 1.0.151, semver 1.0.28, uuid 1.24.0, chrono 0.4.45, thiserror 2.0.19.

---

## File layout (crates/story-model/src/)
```
lib.rs            — re-exports, SAVE_SCHEMA_VERSION const, crate docs
id.rs             — newtype IDs
enums.rs          — RelationshipStage, Emotion, TimeSlot, RiskLevel, CompareOp, ActionType, MemoryKind
state.rs          — GameState, CharacterState, ChoiceRef
choice.rs         — Choice
patch.rs          — StatePatch
condition.rs      — ConditionExpr
turn.rs           — GeneratedTurn, TurnSummary, GenerationWarning, TokenUsage, ParsedAction
history.rs        — HistoryEntry
memory.rs         — MemoryEntry, MemoryFact
error.rs          — CoreError / error category skeleton (re-exported, minimal for now)
```

---

## Resolved type definitions (implement EXACTLY these)

### id.rs — newtype IDs
```rust
use serde::{Deserialize, Serialize};
macro-like or hand-written. Each is:
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct <Name>(pub String);
```
Define: `CharacterId`, `LocationId`, `FlagId`(`String` semantically but newtype), `EventId`, `ItemId`. Provide a `pub fn new()`? No — keep `pub(String)`. Add `impl std::fmt::Display` for each (renders the inner string) for diagnostics.

### enums.rs
```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipStage { Stranger, Acquaintance, Friend, CloseFriend, RomanticInterest, Partner, Estranged }

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Emotion { Neutral, Happy, Sad, Angry, Anxious, Surprised, Affectionate, Cold, Embarrassed, Guilty }

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeSlot { Dawn, Morning, Afternoon, Evening, Night }

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel { Safe, Low, Medium, High, Severe }

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompareOp { Eq, Ne, Lt, Le, Gt, Ge }

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionType { Choose, Dialogue, Action, Move, GeneralAction }

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind { Promise, SecretReveal, RelationshipChange, KeyItem, FirstMeeting, Confession, Betrayal, Other }
```

### state.rs
```rust
pub struct CharacterState {
    pub affection: i16,
    pub trust: i16,
    pub tension: i16,
    pub relationship_stage: RelationshipStage,
    pub current_emotion: Emotion,
    pub known_facts: BTreeSet<String>,
    pub address_form_overrides: BTreeMap<CharacterId, String>,
}

pub struct ChoiceRef {            // GameState.last_choice
    pub choice_id: String,
    pub label: String,
    pub turn: u32,
}

pub struct GameState {
    pub save_schema_version: u32,
    pub story_id: String,
    pub story_version: semver::Version,
    pub pack_schema_version: semver::Version,
    pub session_id: uuid::Uuid,
    pub current_chapter: String,
    pub current_scene_id: String,
    pub turn: u32,
    pub location: LocationId,
    pub time_slot: TimeSlot,
    pub active_route: Option<String>,
    pub characters: BTreeMap<CharacterId, CharacterState>,
    pub flags: BTreeMap<String, bool>,
    pub numbers: BTreeMap<String, i64>,
    pub strings: BTreeMap<String, String>,
    pub inventory: BTreeMap<String, u32>,
    pub occurred_events: BTreeSet<String>,
    pub recent_summaries: VecDeque<String>,
    pub memories: Vec<MemoryEntry>,
    pub history: Vec<HistoryEntry>,
    pub last_choice: Option<ChoiceRef>,
    pub revision: u64,
    pub stable_generation_id: Option<uuid::Uuid>,
}
```
`lib.rs`: `pub const SAVE_SCHEMA_VERSION: u32 = 1;`

### choice.rs
```rust
pub struct Choice {
    pub id: String,
    pub label: String,
    pub intent: String,
    pub tone: Option<String>,
    pub target: Option<CharacterId>,
    pub risk: RiskLevel,
    pub hidden_requirements: Vec<ConditionExpr>,
    pub preview: Option<String>,
}
```

### patch.rs
```rust
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StatePatch {
    pub affection_delta: BTreeMap<CharacterId, i16>,
    pub trust_delta: BTreeMap<CharacterId, i16>,
    pub tension_delta: BTreeMap<CharacterId, i16>,
    pub relationship_transitions: BTreeMap<CharacterId, RelationshipStage>,
    pub set_flags: BTreeMap<String, bool>,
    pub number_delta: BTreeMap<String, i64>,
    pub set_strings: BTreeMap<String, String>,
    pub inventory_delta: BTreeMap<String, i32>,
    pub add_events: BTreeSet<String>,
    pub next_location: Option<LocationId>,
    pub next_time_slot: Option<TimeSlot>,
    pub proposed_route: Option<String>,
}
```
**NOTE:** `deny_unknown_fields` is mandatory. Add a derived `PartialEq` for test ergonomics (spec did not list it; it does not break serde). Also implement `StatePatch::is_empty()`.

### condition.rs
```rust
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]            // tagged union for clean JSON + future eval
pub enum ConditionExpr {
    All { children: Vec<ConditionExpr> },
    Any { children: Vec<ConditionExpr> },
    Not { child: Box<ConditionExpr> },
    FlagEquals { id: String, value: bool },
    NumberCompare { id: String, op: CompareOp, value: i64 },
    AffectionAtLeast { character: CharacterId, value: i16 },
    TrustAtLeast { character: CharacterId, value: i16 },
    EventOccurred { id: String },
    AtLocation { location: LocationId },
    TurnAtLeast { value: u32 },
}
```
(The spec wrote `All(Vec<…>)` tuple form; use the `#[serde(tag="kind")]` named-field form so JSON is self-describing and stable across schema changes. Evaluation is W3 — here only the type + serde.)

### turn.rs
```rust
pub struct ParsedAction {
    pub action_type: ActionType,
    pub target: Option<CharacterId>,
    pub intent: Option<String>,
    pub tone: Option<String>,
    pub content: String,
    pub original_text: String,
    pub confidence: Option<f32>,
}

pub struct TurnSummary {
    // MVP wire fields (non-optional)
    pub facts: Vec<String>,
    pub unresolved: Vec<String>,
    pub relationship_changes: Vec<String>,
    pub knowledge_changes: Vec<String>,
    // §12.3 extended fields (optional, populated from M6+)
    pub interpretation: Option<String>,
    pub address_form_changes: Vec<String>,
    pub important_player_actions: Vec<String>,
    pub source_history_ids: Vec<uuid::Uuid>,
}

pub struct GenerationWarning { pub code: String, pub detail: String }

pub struct TokenUsage { pub prompt_tokens: u32, pub completion_tokens: u32, pub total_tokens: u32 }

pub struct GeneratedTurn {
    pub generation_id: uuid::Uuid,
    pub narration: String,
    pub choices: Vec<Choice>,
    pub proposed_patch: StatePatch,
    pub accepted_patch: StatePatch,
    pub summary: TurnSummary,
    pub warnings: Vec<GenerationWarning>,
    pub usage: Option<TokenUsage>,
}
```

### history.rs
```rust
pub struct HistoryEntry {
    pub id: uuid::Uuid,
    pub turn: u32,
    pub scene_id: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub narration: String,
    pub player_action: Option<ParsedAction>,
    pub choices: Vec<Choice>,
    pub applied_patch: StatePatch,
    pub generation_id: uuid::Uuid,
}
```

### memory.rs
```rust
pub struct MemoryFact {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub known_by: BTreeSet<CharacterId>,
    pub certainty: f32,
}

pub struct MemoryEntry {
    pub id: uuid::Uuid,
    pub kind: MemoryKind,
    pub facts: Vec<MemoryFact>,
    pub interpretation: Option<String>,
    pub participants: BTreeSet<CharacterId>,
    pub location: Option<LocationId>,
    pub importance: u8,
    pub unresolved: bool,
    pub source_history_ids: Vec<uuid::Uuid>,
    pub created_turn: u32,
    pub last_recalled_turn: Option<u32>,
}
```

### error.rs (minimal skeleton — fleshed out in W3/W5)
```rust
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("invalid patch: {0}")]
    InvalidPatch(String),
    #[error("rule violation: {0}")]
    RuleViolation(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
```

### lib.rs
Re-export all public types from submodules. `pub const SAVE_SCHEMA_VERSION: u32 = 1;`. Module docs.

---

## Serde / version policy (encode in tests)
1. **Newtype IDs**: transparent — `"c-namu"` round-trips, not `{"0":"c-namu"}`.
2. **Enums**: snake_case — `RelationshipStage::CloseFriend` ↔ `"close_friend"`.
3. **StatePatch**: `deny_unknown_fields` — an unknown field in patch JSON is a hard serde error (tested).
4. **GameState**: forward-tolerant — adding a future optional field does NOT break deserialization of old saves (tested by deserializing a JSON object missing newly-added fields with defaults); `save_schema_version == SAVE_SCHEMA_VERSION`.
5. **semver fields**: `semver::Version` serializes as a string `"0.1.0"`.
6. **ConditionExpr**: tagged `{"kind":"all","children":[...]}`.

---

## Tasks (TDD)

### Task W2.1: IDs + enums + module skeleton
Create the file layout, `lib.rs` re-exports, `id.rs`, `enums.rs`, `SAVE_SCHEMA_VERSION`. Write `tests/serde_ids_enums.rs`: round-trip every ID (transparent) and every enum variant (snake_case). `cargo test -p story-model`. Commit `feat(story-model): ID newtypes and core enums`.

### Task W2.2: GameState, CharacterState, ChoiceRef, Choice
Implement `state.rs`, `choice.rs`. Test: build a representative `GameState`, round-trip via serde, assert field equality. Commit `feat(story-model): game/character state and choice`.

### Task W2.3: StatePatch + ConditionExpr
Implement `patch.rs`, `condition.rs`. Tests: `StatePatch` default `is_empty()`; `deny_unknown_fields` rejects an unknown key (expect `Err`); `ConditionExpr` tagged round-trip including nested `All/Any/Not`. Commit `feat(story-model): state patch and condition DSL types`.

### Task W2.4: GeneratedTurn, ParsedAction, TurnSummary, HistoryEntry, MemoryEntry/MemoryFact, warnings/usage, error skeleton
Implement `turn.rs`, `history.rs`, `memory.rs`, `error.rs`. Test: round-trip a fully-populated `GeneratedTurn` and `HistoryEntry`. Commit `feat(story-model): turn/history/memory types and error skeleton`.

### Task W2.5: Golden JSON fixtures + version policy tests
Create `tests/fixtures/golden/gamestate_v1.json` and `generated_turn_v1.json` (write from Rust, then snapshot). Tests:
- `GameState` deserializes from golden with `save_schema_version == 1`.
- `StatePatch` with unknown field → `serde_json::from_slice` returns `Err`.
- Forward-tolerance: deserialize a `GameState` JSON missing an optional future field succeeds.
- Commit `test(story-model): golden JSON fixtures and serde/version policy`.

## Completion gate (ST-006)
- `cargo test -p story-model` all pass; `cargo clippy -p story-model -- -D warnings` clean.
- `./scripts/check-core-deps.sh` still passes (story-model gained NO UI/HTTP/FS deps).
- Every public type round-trips through serde.
- `deny_unknown_fields` on StatePatch enforced by test.
- Golden JSON fixtures committed under `tests/fixtures/golden/`.
