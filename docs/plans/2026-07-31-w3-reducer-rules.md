# W3 Validator, Atomic Reducer & Rules Engine Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build the deterministic, LLM-free engine core in `story-core`: condition-DSL evaluation, the 8-stage `StatePatch` validation pipeline, an all-or-nothing atomic reducer with audit log, and route/relationship/ending state machines that the model can never bypass. ST-007, ST-008, ST-009, ST-010.

**Architecture:** `story-core` depends ONLY on `story-model` + `thiserror` (ADR-003). It defines the canonical rule data structures (`StoryRules` and friends) that the W4 Pack loader will later produce from TOML. All patch application is `clone → validate → dry-run → commit-or-rollback`; a failed patch leaves the original state byte-for-byte unchanged and `revision` is bumped exactly once on success. Conditions are pure & deterministic.

**Tech Stack:** story-model, thiserror. No UI/HTTP/FS (enforced by `scripts/check-core-deps.sh`, which must keep passing).

---

## Part A — story-model: rule structures + change log (extends W2)

Add to `crates/story-model/src/rules.rs` (new file) and re-export from `lib.rs`.

### Defaults (consts in rules.rs)
```rust
pub const DEFAULT_DELTA_MIN: i16 = -3;
pub const DEFAULT_DELTA_MAX: i16 = 3;
pub const DEFAULT_STAT_MIN: i16 = -100;
pub const DEFAULT_STAT_MAX: i16 = 100;
```

### Range helper
```rust
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Int16Range { pub min: i16, pub max: i16 }
impl Int16Range { pub fn contains(&self, v: i16) -> bool { v >= self.min && v <= self.max } }
```
`DeltaLimits { affection: Int16Range, trust: Int16Range, tension: Int16Range }` with `DEFAULT` = all `(-3..=3)`.

### Rule types (canonical — W4 produces these from `rules.toml`)
```rust
pub struct StoryRules {
    pub locations: BTreeMap<LocationId, LocationDef>,
    pub allowed_flags: BTreeSet<String>,
    pub numbers: BTreeMap<String, NumberVarDef>,
    pub strings: BTreeMap<String, StringVarDef>,
    pub allowed_items: BTreeSet<String>,
    pub events: BTreeMap<EventId, EventDef>,
    pub routes: BTreeMap<String, RouteDef>,
    pub endings: BTreeMap<String, EndingDef>,
    pub delta_limits: DeltaLimits,
    pub stat_limits: Int16Range,           // final affection/trust/tension bounds, default -100..=100
    pub default_choice_count: u8,           // MVP = 3
}
pub struct LocationDef { pub id: LocationId, pub display_name: String, pub adjacent: BTreeSet<LocationId> }
pub struct NumberVarDef  { pub id: String, pub min: i64, pub max: i64, pub default: i64 }
pub struct StringVarDef  { pub id: String, pub max_len: u32, pub allowed: Option<BTreeSet<String>> }
pub struct EventDef {
    pub id: EventId, pub trigger: ConditionExpr, pub priority: u32,
    pub effects: EventEffects, pub allow_relationship_skip: bool, pub route_lock: Option<String>,
}
pub struct EventEffects {
    pub affection_delta: BTreeMap<CharacterId, i16>,
    pub set_flags: BTreeMap<String, bool>,
    pub number_delta: BTreeMap<String, i64>,
    pub add_events: BTreeSet<String>,
    pub force_location: Option<LocationId>,
    pub force_relationship: BTreeMap<CharacterId, RelationshipStage>,
    pub ending: Option<String>,
}
pub struct RouteDef  { pub id: String, pub enter_condition: ConditionExpr, pub premise: String }
pub struct EndingDef { pub id: String, pub condition: ConditionExpr, pub label: String }
```

### Change log (referenced but undefined in W2 — define now)
In `crates/story-model/src/change_log.rs`:
```rust
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeSource  { LlmPatch, EventRule, Reducer, Migration, PlayerAction }
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeDecision { Applied, Rejected, Clamped, Deferred }
pub struct StateChangeLog {
    pub id: uuid::Uuid, pub generation_id: uuid::Uuid, pub turn: u32, pub source: ChangeSource,
    pub field_path: String,
    pub before: serde_json::Value, pub proposed: serde_json::Value, pub applied: Option<serde_json::Value>,
    pub decision: ChangeDecision, pub rule_id: Option<String>, pub reason_code: Option<String>,
}
```
**Narration text MUST NEVER appear in a StateChangeLog.**

`GenerationWarning` already exists in story-model (W2) — reuse it.

### Part A task: serde round-trip + golden for StoryRules/StateChangeLog; add tests. Commit `feat(story-model): canonical rule structures and change log`.

---

## Part B — story-core: the engine

File layout `crates/story-core/src/`:
```
lib.rs              — re-exports
eval.rs             — ConditionExpr evaluation
validator.rs        — 8-stage pipeline
reducer.rs          — atomic apply
state_machine.rs    — route / relationship / ending
engine.rs           — top-level orchestration (validate -> reduce -> state machines)
error.rs            — CoreError variants + reason codes
```

### eval.rs (ST-009) — `ConditionExpr::evaluate`
```rust
pub fn evaluate(cond: &ConditionExpr, ctx: &EvalContext) -> bool;
pub struct EvalContext<'a> {
    pub state: &'a GameState,
    pub rules: &'a StoryRules,
}
```
- `All` → all children; `Any` → any child; `Not` → !child.
- `FlagEquals` → `state.flags.get(id) == Some(value)` (missing flag = treat absent as `false`; `value==false` matches absent).
- `NumberCompare` → read `state.numbers.get(id)`; if missing, treat as the var's `default` from rules (or 0 if unknown id — but unknown id should have been caught at Pack validation; here be defensive: missing → 0).
- `AffectionAtLeast`/`TrustAtLeast` → read `state.characters.get(character)`.
- `EventOccurred` → `state.occurred_events.contains(id)`.
- `AtLocation` → `state.location == location`.
- `TurnAtLeast` → `state.turn >= value`.
Tests: full truth table for every variant; nested `All(Any(Not(..)))`; determinism (same input → same output). Equal-priority event ordering is handled in `engine.rs`, not here.

### validator.rs (ST-007) — 8-stage pipeline
```rust
pub struct ValidationOutcome {
    pub sanitized: StatePatch,     // clamped/filtered patch safe to apply
    pub warnings: Vec<GenerationWarning>,
    pub change_log: Vec<StateChangeLog>,
}
pub fn validate(state: &GameState, rules: &StoryRules, patch: &StatePatch, generation_id: Uuid)
    -> Result<ValidationOutcome, CoreError>;
```
Stages, each returning `Err(CoreError)` (reject whole patch) on hard failure or splitting cosmetic issues into `warnings`:
1. **Structural**: choice count (if patch carries choices upstream it's not here — patches don't carry choices; but validate `narration`/`summary` non-emptiness happens at parse time, not here). Here: no unknown fields (serde already enforced `deny_unknown_fields`).
2. **Reference**: every `CharacterId` in any delta/transition exists in `state.characters`; every flag in `set_flags` is in `rules.allowed_flags`; every number var in `number_delta` is in `rules.numbers`; every string var in `set_strings` is in `rules.strings`; every item in `inventory_delta` is in `rules.allowed_items`; every event in `add_events` is in `rules.events`; `next_location` exists in `rules.locations`. Unknown → `Err` with `PATCH_UNKNOWN_*` reason code.
3. **Range**: each affection/trust/tension delta within `rules.delta_limits.<field>` (default -3..=3); projected final stat within `rules.stat_limits` (-100..=100) — out-of-range final → reject (NO automatic clamping); number-delta projected within var min/max; string length ≤ `max_len` and (if `allowed` set) value in allowed set; inventory projected ≥ 0 (negative → reject).
4. **Permission**: reject any field the model may not set — there is no `ending` field on the patch (structurally true), but also reject `relationship_transitions` to a stage more than one step away from current unless an applicable event grants `allow_relationship_skip`. `proposed_route` is allowed (it's only a *proposal*).
5. **Transition**: `next_location` must be adjacent to current (in `rules.locations[current].adjacent`) UNLESS an event effect forces it (handled in engine). `next_time_slot` always allowed (time advances). 
6. **Cross**: internal consistency — e.g. a delta targeting a character not present in scene is a warning, not error. (Keep light for MVP.)
7–8 (dry-run + commit) live in the reducer, which calls `validate` first.

Each rejected field produces a `StateChangeLog` entry with `decision: Rejected` + `reason_code`. Sanitized patch drops rejected deltas only where safe; **default policy is reject-the-whole-patch** if ANY hard stage fails (atomicity). Cosmetic-only issues become `GenerationWarning`s and do NOT reject.

### state_machine.rs (ST-010) — route / relationship / ending
- `confirm_route(state, rules, proposed: &Option<String>) -> Option<String>`: returns the proposed route id ONLY if `rules.routes[id].enter_condition` evaluates true; otherwise keeps `state.active_route`. Unmet proposal never mutates state.
- `resolve_relationship_transition(current, target, rules, event_skip_allowed) -> Result<RelationshipStage, CoreError>`: enforce single-step transitions on the order `Stranger → Acquaintance → Friend → CloseFriend → RomanticInterest → Partner` (and `→ Estranged` allowed from any). Skip beyond one step allowed only when an applicable fired event has `allow_relationship_skip`. Else reject.
- `resolve_ending(state, rules) -> Option<String>`: after events fire, evaluate every `EndingDef.condition`; if exactly one matches, that's the ending. **Never read from the patch.** (Multiple simultaneous endings → pick by stable id sort and warn; none → `None`.)

Define a stable step ordering for `RelationshipStage` (a method `step_index(&self) -> i8`).

### reducer.rs (ST-008) — atomic apply
```rust
pub struct ApplyResult {
    pub state: GameState,
    pub change_log: Vec<StateChangeLog>,
    pub warnings: Vec<GenerationWarning>,
    pub fired_events: Vec<EventId>,
    pub ending: Option<String>,
}
pub fn apply_patch(state: &GameState, rules: &StoryRules, patch: &StatePatch, generation_id: Uuid)
    -> Result<ApplyResult, CoreError>;
```
Algorithm:
1. `let outcome = validator::validate(state, rules, patch, generation_id)?;` (hard fail → return Err, original state untouched).
2. `let mut next = state.clone();`
3. Apply sanitized deltas/maps to `next` (affection/trust/tension add deltas; set_flags; number_delta; set_strings; inventory_delta; add_events; relationship_transitions via `resolve_relationship_transition`; next_location; next_time_slot).
4. For each `add_events` event actually present in `rules.events` whose `trigger` evaluates true and (if `route_lock`) matches `next.active_route`: fire it — apply its `EventEffects`, record `fired_events`. Order: sort candidates by `(priority desc, id asc)` for determinism.
5. After events: `ending = resolve_ending(&next, rules)`. `active_route = confirm_route(&next, rules, &patch.proposed_route)`.
6. `next.revision = state.revision + 1;` (exactly once). `next.turn = state.turn + 1;` (a turn advances on commit — confirm this is desired; if uncertain, do NOT bump turn here, only revision; **decision: bump revision only, leave turn to the engine layer**).
7. Build `StateChangeLog` entries (`decision: Applied`) for each changed field with before/proposed/applied JSON.
8. On ANY error after cloning: return `Err`; `state` (the caller's original) is never mutated because we only ever mutated the clone.

**Invariants (tested explicitly):**
- A patch that fails at any stage leaves the input `GameState` unchanged (compare serialized bytes before/after).
- Success increments `revision` by exactly 1.
- Same `(state, rules, patch)` → identical `ApplyResult` 100× (determinism proptest/loop).
- No narration text in any change-log entry.
- `ending`/`route` never come from the patch directly.

### engine.rs — orchestration
A thin `pub fn commit_turn(state, rules, patch, generation_id) -> Result<ApplyResult, CoreError>` delegating to `apply_patch`. (The full `EngineCommand`/`Transition`/`EngineEffect` boundary is W5; here expose the patch-commit primitive.)

### error.rs
```rust
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("invalid patch ({code}): {detail}")]
    InvalidPatch { code: &'static str, detail: String },
    #[error("rule violation ({code}): {detail}")]
    RuleViolation { code: &'static str, detail: String },
    #[error("forbidden transition: {0}")]
    ForbiddenTransition(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
```
Reason-code constants: `PATCH_UNKNOWN_CHARACTER`, `PATCH_UNKNOWN_FLAG`, `PATCH_UNKNOWN_LOCATION`, `PATCH_DELTA_OUT_OF_RANGE`, `PATCH_STAT_OUT_OF_RANGE`, `PATCH_NEGATIVE_INVENTORY`, `PATCH_RELATIONSHIP_SKIP`, `PATCH_NONADJACENT_LOCATION`.

---

## Tasks (TDD)
- **W3.1 (Part A):** rule structs + change log in story-model; serde round-trip + golden. Commit `feat(story-model): canonical rule structures and change log`.
- **W3.2:** `eval.rs` — full truth table + nested + determinism tests. Commit `feat(story-core): condition DSL evaluator`.
- **W3.3:** `validator.rs` — boundary table (delta -4/+4, stat ±101, unknown id, negative inventory, nonadjacent location, relationship skip), each rejected with correct reason code; cosmetic → warning. Commit `feat(story-core): 8-stage patch validator`.
- **W3.4:** `reducer.rs` + `state_machine.rs` — rollback fault test (failed patch leaves state byte-identical), revision-exactly-once, 100× determinism, route-confirm-only-when-true, ending-never-from-patch, forbidden relationship transition. Commit `feat(story-core): atomic reducer and route/relationship/ending machines`.
- **W3.5:** `engine.rs` `commit_turn` + integration test wiring validate→apply→ending using a hand-built `StoryRules` + `GameState`. Commit `feat(story-core): turn commit orchestration`.

## Completion gate (ST-007/008/009/010)
- `cargo test --workspace` green; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --check` clean.
- `./scripts/check-core-deps.sh` still OK; `cargo tree -p story-core` still only story-model + thiserror.
- Zero test cases where an invalid patch is even partially applied (Acceptance §20).
- Determinism test (100× same input → same output) passes.
- Route/ending reproducible with NO LLM input.
