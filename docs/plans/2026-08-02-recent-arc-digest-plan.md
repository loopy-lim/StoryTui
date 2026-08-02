# Recent-arc digest + repetition detection — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Give the model a compact semantic digest of the recent arc plus an safe repetition flag (with motifs), so weaker local models stop regressing/repeating earlier beats — without any extra LLM call.

**Architecture:** Store one digest line per accepted turn into the already-persisted (but unused) `GameState.recent_summaries` (model `interpretation` when present, else the narration's first clause). Render the last 4 as `# Recent arc` and run a deterministic token-overlap repetition check over the last 6, emitting `# 반복 감지` only when a motif is shared across ≥2 turns. This replaces the surface-level `# Anti-repeat` block added earlier.

**Tech Stack:** Rust 2024, `story-core` (`engine.rs`, `prompt_context.rs`), `story-model` (`GameState.recent_summaries: VecDeque<String>`), `insta` snapshots.

**Companion design doc:** `docs/plans/2026-08-02-recent-arc-digest-design.md`.

> **Commits:** Steps marked `Commit` are written per the plan format. The user has NOT yet authorized commits this session — hold them (leave changes in the working tree) until the user says to commit.

---

### Task 1: Repetition-detection pure functions (TDD)

**Files:**
- Modify: `crates/story-core/src/prompt_context.rs` (add `normalize_token`, `shared_motifs`, constants; add unit tests in the in-crate `mod tests`)

**Step 1: Write the failing tests** — append to the `mod tests` block in `prompt_context.rs`:

```rust
#[test]
fn normalize_token_strips_trailing_particle_and_filters() {
    use super::{normalize_token};
    assert_eq!(normalize_token("유나가"), Some("유나".to_string()));
    assert_eq!(normalize_token("커피를"), Some("커피".to_string()));
    assert_eq!(normalize_token("비"), None);            // too short after no strip
    assert_eq!(normalize_token("비가"), Some("비".to_string())); // 2-char base survives
    assert_eq!(normalize_token("했다"), None);          // stopword
    assert_eq!(normalize_token("…"), None);             // non-alphanumeric
}

#[test]
fn shared_motifs_returns_tokens_in_two_or_more_digests() {
    use super::shared_motifs;
    let digests = ["유나가 커피를 내렸다", "커피가 식었다", "비가 창을 때렸다"];
    let m = shared_motifs(&digests);
    assert!(m.contains(&"커피".to_string()), "{m:?}");
    assert!(!m.contains(&"유나".to_string())); // only in 1
}

#[test]
fn shared_motifs_empty_when_nothing_repeats() {
    use super::shared_motifs;
    let digests = ["유나가 웃었다", "비가 그쳤다", "문이 닫혔다"];
    assert!(shared_motifs(&digests).is_empty());
}
```

**Step 2: Run to verify failure**

Run: `cargo test -p story-core --lib normalize_token 2>&1 | tail`
Expected: compile error (functions not defined).

**Step 3: Implement** — add near the other helpers in `prompt_context.rs`:

```rust
/// Number of recent digests scanned for repetition (wider than the rendered arc
/// so a motif that started just outside the prose window is still caught).
const REPETITION_DETECT_TURNS: usize = 6;
const REPETITION_MAX_KEYWORDS: usize = 5;
/// Korean glue words / verb endings to ignore. Korean morphology is not
/// analyzed; this small list + the "shared across ≥2 turns" bar keeps noise low.
const REPETITION_STOPWORDS: &[&str] = &[
    "했다", "있다", "없다", "했다", "한다", "하는", "하고", "위해", "그", "이", "저",
    "등", "및", "수", "것", "때", "임", "됨",
];
/// Trailing particles stripped so inflected forms cluster (유나가/유나는 → 유나).
const REPETITION_PARTICLES: &[&str] = &[
    "들은", "는", "이가", "가", "은", "이", "을", "를", "에", "의", "도", "와", "과", "로",
];

/// Normalize one whitespace token into a comparable content token, or `None`
/// when it is too short, a stopword, or non-alphanumeric.
fn normalize_token(word: &str) -> Option<String> {
    let trimmed = word.trim_matches(|c: char| !c.is_alphanumeric());
    let mut w = trimmed.to_string();
    for p in REPETITION_PARTICLES {
        if let Some(stripped) = w.strip_suffix(p) {
            w = stripped.to_string();
            break;
        }
    }
    let w = w.trim_matches(|c: char| !c.is_alphanumeric()).to_string();
    if w.chars().count() < 2 || REPETITION_STOPWORDS.contains(&w.as_str()) {
        return None;
    }
    Some(w.to_lowercase())
}

/// Return content tokens that appear in at least 2 of `digests` (document
/// frequency, not raw count), sorted and capped. Empty when nothing repeats.
fn shared_motifs(digests: &[&str]) -> Vec<String> {
    use std::collections::{BTreeMap, BTreeSet};
    let mut df: BTreeMap<String, usize> = BTreeMap::new();
    for d in digests {
        let seen: BTreeSet<String> = d.split_whitespace().filter_map(normalize_token).collect();
        for t in seen {
            *df.entry(t).or_default() += 1;
        }
    }
    let mut out: Vec<String> = df
        .into_iter()
        .filter(|(_, c)| *c >= 2)
        .map(|(k, _)| k)
        .collect();
    out.sort_unstable();
    out.truncate(REPETITION_MAX_KEYWORDS);
    out
}
```

**Step 4: Run to verify pass**

Run: `cargo test -p story-core --lib normalize_token shared_motifs 2>&1 | tail`
Expected: 3 tests PASS.

**Step 5: Commit** — HOLD (see note at top).

---

### Task 2: Store a digest line on each accepted turn (TDD)

**Files:**
- Modify: `crates/story-core/src/prompt_context.rs` — change `fn first_clause` to `pub fn first_clause` (engine reuses it).
- Modify: `crates/story-core/src/engine.rs` — add `turn_digest` + `push_recent_summary`; call at the two history-push sites (`assemble_accepted_turn` ~`:1113`, `assemble_accepted_2stage_turn` ~`:1009`).
- Test: `crates/story-core/tests/engine.rs` (or wherever accept tests live — locate with `grep -rn "fn .*accept\|AcceptGeneration" crates/story-core/tests`).

**Step 1: Write the failing test** (append to an engine integration test file; reuse its existing fixture/helpers — if none build a minimal `EngineState`, run a turn via `reduce(... AcceptGeneration ...)`, and assert):

```rust
#[test]
fn accepted_turn_appends_a_recent_summary_digest() {
    // Build/drive an engine to accept ONE generated turn (use the existing
    // test helpers in this file). Then:
    let game = /* engine.game after accept */;
    assert_eq!(game.recent_summaries.len(), 1);
    // When the draft's summary.interpretation is set, the digest is that text.
    assert_eq!(game.recent_summaries[0], "유나가 거리를 둔 채 창 밖을 본다");
}
```

If driving a full accept is heavy in that file, add a focused unit test of the helper instead:

```rust
#[test]
fn turn_digest_prefers_interpretation_then_falls_back_to_narration() {
    use story_core::engine::turn_digest; // make `turn_digest` pub(crate) or pub
    assert_eq!(
        turn_digest("비가 그쳤다. 유나가 찻잔을 내려놓는다.", Some("유나가 거리를 둔 채 창 밖을 본다")),
        "유나가 거리를 둔 채 창 밖을 본다"
    );
    // No interpretation → first clause of narration.
    let fb = turn_digest("비가 그쳤다. 유나가 찻잔을 내려놓는다.", None);
    assert_eq!(fb, "비가 그쳤다");
}
```

**Step 2: Run to verify failure**

Run: `cargo test -p story-core turn_digest 2>&1 | tail`
Expected: compile error (`turn_digest` undefined).

**Step 3: Implement**

In `prompt_context.rs`, change visibility:
```rust
pub fn first_clause(text: &str, max_chars: usize) -> String { /* unchanged body */ }
```

In `engine.rs`, add (near the other accept helpers, e.g. just above `assemble_accepted_turn`):
```rust
/// Recent-arc storage cap (FIFO). Keeps the prompt digest bounded; the engine
/// state snapshot persists older consequences.
const RECENT_SUMMARY_CAP: usize = 12;

/// One-line semantic digest of an accepted turn, stored for the prompt's
/// `# Recent arc` block. Prefers the model's own `interpretation`; falls back
/// to the narration's first clause when the model omitted one.
pub fn turn_digest(narration: &str, interpretation: Option<&str>) -> String {
    if let Some(interp) = interpretation.map(str::trim).filter(|s| !s.is_empty()) {
        return interp.to_string();
    }
    crate::prompt_context::first_clause(narration, 120)
}

/// Push `digest` onto `game.recent_summaries`, keeping the most recent
/// [`RECENT_SUMMARY_CAP`] entries (1:1 alignment with `game.history`).
fn push_recent_summary(game: &mut story_model::GameState, digest: String) {
    let q = &mut game.recent_summaries;
    q.push_back(digest);
    while q.len() > RECENT_SUMMARY_CAP {
        q.pop_front();
    }
}
```

Call it right after each `game.history.push(entry);`:

At `assemble_accepted_turn` (the active single-call path, ~`:1113`), where `draft` is in scope:
```rust
game.history.push(entry);
push_recent_summary(
    &mut game,
    turn_digest(&entry.narration, draft.summary.interpretation.as_deref()),
);
```

At `assemble_accepted_2stage_turn` (~`:1009`) there is no `TurnSummary`; use narration only:
```rust
game.history.push(entry);
push_recent_summary(&mut game, turn_digest(&entry.narration, None));
```

**Step 4: Run to verify pass**

Run: `cargo test -p story-core turn_digest 2>&1 | tail`
Expected: PASS.

**Step 5: Commit** — HOLD.

---

### Task 3: Render `# Recent arc` + `# 반복 감지`; remove `# Anti-repeat` (TDD)

**Files:**
- Modify: `crates/story-core/src/prompt_context.rs` — `build_user`: capture the full history, render the new blocks, delete the `anti_repeat_cue` call and its function.
- Test: `crates/story-core/tests/prompt_context.rs`

**Step 1: Write the failing tests** — append:

```rust
#[test]
fn recent_arc_renders_from_stored_summaries() {
    let (pack, mut state, _) = fixture(false);
    state.recent_summaries.extend([
        "유나가 경계하며 뒷걸음쳤다".to_string(),
        "유나가 창밖만 본다".to_string(),
    ]);
    // Make recent_summaries aligned with history length (2 entries ⇒ 2 history).
    let recent = vec![
        recap_history_entry(1, "무언가."),
        recap_history_entry(2, "무언가."),
    ];
    let bundle = build_prompt(&pack, &state, &recent, None);
    assert!(bundle.user.contains("# Recent arc"));
    assert!(bundle.user.contains("유나가 경계하며 뒷걸음쳤다"));
}

#[test]
fn repetition_note_fires_when_a_motif_is_shared() {
    let (pack, mut state, _) = fixture(false);
    let recent: Vec<HistoryEntry> =
        (1..=6)
            .map(|t| recap_history_entry(t, &format!("유나가 커피를 {t}번 내렸다")))
            .collect();
    // aligned: same length
    let bundle = build_prompt(&pack, &state, &recent, None);
    assert!(bundle.user.contains("# 반복 감지"), "{}", bundle.user);
    assert!(bundle.user.contains("커피"));
}

#[test]
fn no_repetition_note_when_each_turn_differs() {
    let (pack, state, _) = fixture(false);
    let recent = vec![
        recap_history_entry(1, "유나가 웃었다."),
        recap_history_entry(2, "편지가 발견됐다."),
    ];
    let bundle = build_prompt(&pack, &state, &recent, None);
    assert!(!bundle.user.contains("# 반복 감지"));
}

#[test]
fn anti_repeat_phrase_block_is_gone() {
    let (pack, state, recent) = fixture(false);
    let bundle = build_prompt(&pack, &state, &recent, None);
    assert!(!bundle.user.contains("# Anti-repeat"));
}
```

**Step 2: Run to verify failure**

Run: `cargo test -p story-core --test prompt_context recent_arc repetition anti_repeat 2>&1 | tail`
Expected: failures (no `# Recent arc`, `# Anti-repeat` still present).

**Step 3: Implement** — in `build_user`:

1. Capture the full history before windowing (rename the param locally):
```rust
let full = recent;
let recent_start = full.len().saturating_sub(RECENT_TURN_WINDOW);
let earlier = &full[..recent_start];
let recent = &full[recent_start..];
```
2. Remove the `s.push_str(&anti_repeat_cue(recent));` line and delete the `anti_repeat_cue` function (keep `first_clause`).
3. After the `# Recent turns` prose loop, add:
```rust
s.push_str(&recent_arc_block(state, full));
```
4. Add the builder:
```rust
const RECENT_ARC_RENDER_TURNS: usize = 4;

/// Render `# Recent arc` (last 4 digests) and, only when a motif is shared
/// across ≥2 of the last [`REPETITION_DETECT_TURNS`] digests, a
/// `# 반복 감지` note. Empty string when there is no history.
fn recent_arc_block(state: &story_model::GameState, full: &[HistoryEntry]) -> String {
    if full.is_empty() {
        return String::new();
    }
    let rs = &state.recent_summaries;
    let aligned = rs.len() == full.len();
    let digest_for = |i: usize| -> String {
        if aligned {
            if let Some(stored) = rs.get(i) {
                let s = stored.trim();
                if !s.is_empty() {
                    return s.to_string();
                }
            }
        }
        first_clause(&full[i].narration, 120)
    };

    let mut s = String::from("\n# Recent arc\n");
    let render_start = full.len().saturating_sub(RECENT_ARC_RENDER_TURNS);
    for i in render_start..full.len() {
        s.push_str(&format!("## turn {} ({})\n", full[i].turn, full[i].scene_id));
        s.push_str(&digest_for(i));
        s.push('\n');
    }

    let det_end = full.len();
    let det_start = full.len().saturating_sub(REPETITION_DETECT_TURNS);
    let digests: Vec<String> = (det_start..det_end).map(digest_for).collect();
    let refs: Vec<&str> = digests.iter().map(String::as_str).collect();
    let motifs = shared_motifs(&refs);
    if !motifs.is_empty() {
        s.push_str(&format!(
            "\n# 반복 감지\n\
             최근 흐름에서 이 표현/소재가 반복되고 있다: {}.\n\
             같은 분위기로 되풀이하지 말고, 한 가지 새로운 관찰 가능한 압력이나 방향으로 전진시킨다.\n",
            motifs.join(", ")
        ));
    }
    s
}
```
5. `build_user` signature already takes `state: &GameState` — confirm `state` is available where the block is rendered (it is; it's a param).

**Step 4: Run to verify pass**

Run: `cargo test -p story-core --test prompt_context 2>&1 | tail`
Expected: the 4 new tests PASS; existing prompt tests still PASS (the golden snapshot test will now FAIL — handle in Task 4).

**Step 5: Commit** — HOLD.

---

### Task 4: Update golden snapshot + full verification

**Files:**
- Update: `crates/story-core/tests/snapshots/prompt_context__golden_prompt_snapshot_is_stable.snap`

**Step 1: Regenerate the snapshot deliberately**

Run: `INSTA_UPDATE=always cargo test -p story-core --test prompt_context golden_prompt_snapshot_is_stable 2>&1 | tail`
Then inspect: `sed -n '/=== USER ===/,/=== HASH ===/p' crates/story-core/tests/snapshots/prompt_context__golden_prompt_snapshot_is_stable.snap`
Expected: `# Anti-repeat` is gone; `# Recent arc` present with 2 digest lines (the fixture has 2 history entries; no `# 반복 감지` since "유나가 커피를 내렸다" / "비가 창을 때렸다" share no content token).

**Step 2: Full workspace test**

Run: `cargo test --workspace 2>&1 | grep -E "test result:|FAILED|error\[" | tail -20`
Expected: all PASS, 0 FAILED.

**Step 3: Clippy**

Run: `cargo clippy -p story-core -p story-provider --all-targets 2>&1 | grep -E "warning:|error:"`
Expected: no warnings/errors.

**Step 4: Commit** — HOLD (ask user).

---

## Notes for the executor

- The `first_clause` function already exists from the earlier A3 work; Task 2 only flips it to `pub`.
- `recent_summaries` is part of `GameState` ⇒ already serialized in `SavePayload` (`stable_state`). No schema version bump, no migration. Old saves load with it empty; the render-time `first_clause` fallback covers them.
- The `# Earlier story` recap (A1) and B1/B2 thinking-strip work are untouched.
- If `turn_digest` testability requires `pub`, expose it `pub` (it lives behind the crate boundary used by tests in the same workspace).
