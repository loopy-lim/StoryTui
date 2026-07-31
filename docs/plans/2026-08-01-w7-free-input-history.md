# W7 Free Input & History Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Let the player act outside the prepared choices via Korean free text, parse it into a safe `ParsedAction` (never distorting intent), and support regenerate/restore over committed-turn history. ST-022 (grapheme input), ST-023 (rule-based ParsedAction), ST-024 (history/regenerate/checkpoint).

**Architecture:** `story-tui` owns the grapheme-cluster edit buffer (`unicode-segmentation` + `unicode-width`). `story-core` owns the rule-based `ParsedAction` parser and extends the engine with `SubmitFreeAction`, `RegenerateCurrentTurn`, `RestoreCheckpoint` + generation lineage. Player-sovereignty rule: the parser only *structures* intent; low-confidence input becomes `ActionType::GeneralAction` with `original_text` preserved verbatim, and the prompt contract tells the model not to confirm/distort unstated emotions/actions.

**Tech Stack:** unicode-segmentation 1.13.3, unicode-width 0.2.2 (story-tui); story-core stays `story-model + thiserror + sha2`.

---

## Part A — story-tui: grapheme free-input buffer (new `input.rs`)

```rust
pub struct InputBuffer { graphemes: Vec<String>, cursor: usize, max_chars: usize }
impl InputBuffer {
    pub fn new(max_chars: usize) -> Self;
    pub fn insert_str(&mut self, s: &str);        // splits into graphemes via unicode-segmentation, filters control chars, enforces max_chars
    pub fn insert_grapheme(&mut self, g: &str);
    pub fn backspace(&mut self);                  // delete grapheme before cursor
    pub fn delete(&mut self);                     // delete grapheme at cursor
    pub fn cursor_left(&mut self) / cursor_right / cursor_start / cursor_end(&mut self);  // all grapheme-aligned
    pub fn text(&self) -> String;                 // join
    pub fn char_count(&self) -> usize;            // grapheme count for the char counter
    pub fn display_width(&self) -> usize;         // sum of unicode-width per grapheme (for layout)
    pub fn clear(&mut self);
}
```
- Grapheme splitting: `unicode_segmentation::UnicodeSegmentation::graphemes(.., true)`.
- Control-char filtering on paste: drop C0 controls except `\n` (if multi-line) / tab handling per your choice; keep it simple.
- `max_chars` counts GRAPHEMES, not bytes/scalars.
- Tests (ST-022 fixture): Korean composition (가/각/안ㅇ), emoji (👁️ + ZWJ sequences), combining marks (e.g. `é`), mixed CJK; cursor movement lands on grapheme boundaries (never mid-syllable); backspace deletes a full grapheme; paste over max_chars truncates cleanly; display_width sums correctly.

## Part B — story-core: rule-based ParsedAction (new `action_parse.rs`)

```rust
pub fn parse_action(raw: &str, ctx: &ParseContext) -> ParsedAction;
pub struct ParseContext<'a> {
    pub known_characters: &'a BTreeMap<CharacterId, CharacterDef>,  // id + name + aliases
    pub known_locations: &'a BTreeMap<LocationId, LocationDef>,
}
```
Rules (deterministic, conservative — never over-interpret):
1. Normalize: trim; if empty → `GeneralAction` with empty content (caller rejects empty before submit anyway).
2. Detect a known **character** mentioned by name/alias (longest match wins) → `target`.
3. Detect **movement verbs** (`간다/가/이동`, `to <location>`, location name present) with a known location → `ActionType::Move`, `target` = location-derived (store in `content`/a note; ParsedAction.target is CharacterId-typed so for moves put the location in `content` and leave `target` None, OR extend — keep `target` for characters only; record location in `content`).
4. Detect **dialogue verbs** (`말한다/묻는다/대답한다/외친다` or quoted `"…"`) → `ActionType::Dialogue`.
5. Otherwise → `ActionType::Action`.
6. **Confidence:** high only when a clear verb + known entity match; if the match is weak/ambiguous → `ActionType::GeneralAction`, `confidence` low, and `content = original_text`.
7. **ALWAYS** set `original_text = raw` verbatim. Never fabricate `intent`/`tone` not present — leave them `None` unless directly derivable.

`ParsedAction` fields (from story-model W2): `action_type, target: Option<CharacterId>, intent: Option<String>, tone: Option<String>, content: String, original_text: String, confidence: Option<f32>`.

Tests (ST-023): 20 fixed free inputs covering — talk-to-known-character, move-to-known-location, quoted dialogue, ambiguous/unknown-noun → GeneralAction with original_text preserved, Korean particle variants (을/를/에게/한테), low-confidence never produces a confident false action. Assert `original_text` always equals input.

## Part C — story-core: engine free-action, regenerate, lineage (extend `engine.rs`)

Add command handling (the enum already has `SubmitFreeAction`, `RegenerateCurrentTurn`, `RestoreCheckpoint` per spec — if not present in the W5 enum, add them):
- **`SubmitFreeAction { action: ParsedAction }`**: like `SubmitChoice` — requires no pending generation; create `PendingGeneration` (checkpoint + gen_id), build a prompt that INCLUDES the parsed action + the player-sovereignty rule ("이 입력은 플레이어의 의도다. 감정이나 행동을 임의로 확정/왜곡하지 말 것"), emit `RequestGeneration`. The draft's patch is validated/committed the same way as a choice.
- **`RegenerateCurrentTurn`**: if a generation is pending, cancel it and re-request from the SAME checkpoint with a new gen_id (lineage: keep a list of attempted generation_ids for the current turn for diagnostics). The previously committed turn (if any) is NOT overwritten until a new draft is accepted.
- **`RestoreCheckpoint { checkpoint_id }`**: restore a prior checkpoint (kept in a small in-memory lineage ring) — for MVP, support restoring the most recent stable checkpoint.
- **Generation lineage:** `EngineState` gains `lineage: Vec<uuid::Uuid>` (attempted gen ids for the current pending turn) for diagnostics/regenerate. History (`game.history`) shows only COMMITTED generations by default (it already does — only accepted turns are appended).

Prompt contract (PromptContext): add the player-sovereignty clause to the system prompt when the turn originates from a free action. (Extend `build_prompt` to accept an optional `player_action: Option<&ParsedAction>` and include its `original_text` + the sovereignty rule.)

Tests (ST-024): SubmitFreeAction drives a turn; RegenerateCurrentTurn cancels pending and re-requests from the same checkpoint (old draft discarded, checkpoint unchanged); a failed regenerate does NOT overwrite the committed turn; history contains only committed entries.

## Tasks (TDD)
- **W7.1 (story-tui):** `InputBuffer` + grapheme fixture tests. Commit `feat(story-tui): grapheme-based free input buffer`.
- **W7.2 (story-core):** `parse_action` + 20-input suite. Commit `feat(story-core): rule-based ParsedAction parser with original-text preservation`.
- **W7.3 (story-core):** engine SubmitFreeAction/Regenerate/Restore + lineage + prompt sovereignty clause + tests. Commit `feat(story-core): free-action turns, regenerate, and generation lineage`.

## Completion gate (ST-022/023/024)
- `cargo test --workspace` green; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --check` clean.
- `./scripts/check-core-deps.sh` OK; story-core still `story-model + thiserror + sha2`.
- Grapheme tests cover Korean/emoji/combining/mixed; cursor/backspace are grapheme-aligned.
- Parser: original_text ALWAYS preserved; low-confidence → GeneralAction (never a confident false action).
- Regenerate never overwrites a committed turn; failed regen preserves prior committed turn.
- No save-to-disk (W8), no Director/Writer (W10), no real HTTP (W6).
