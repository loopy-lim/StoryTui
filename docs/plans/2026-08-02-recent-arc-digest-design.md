# Recent-arc digest + repetition detection — Design

Date: 2026-08-02
Status: Approved (brainstorming complete)

## Problem

Weaker local models (e.g. `private-local-model`) replay earlier beats: a new scene
appears without error but reuses an old mood/image/plot point. Root cause
established in the prior debugging pass:

- The prompt only renders the last `RECENT_TURN_WINDOW = 4` turns as full prose.
- Long-term memory is **event-driven only** (`memories_from_turn`), so for a
  slice-of-life session `state.memories` is usually empty and the
  `# Recalled long-term memory` block is empty.
- The model already produces a validated per-turn `TurnSummary` (interpretation,
  relationship changes, …), but it is **discarded after validation**.
- `GameState.recent_summaries: VecDeque<String>` is an unused slot (always empty).

So beyond the 4-turn prose window the model has only state numbers, and it
regresses. (A bounded `# Earlier story` recap was already added for turns before
the window; this design adds the semantic "current arc" + an safe repetition
signal for the recent window.)

## Goal

Give the model, each turn:

1. A compact **semantic digest of the recent arc** (mood + story), and
2. An safe **repetition flag** (with the recurring motifs) so it steers to a
   new beat.

…without any extra LLM call.

## Decisions (from brainstorming)

- **Source: hybrid (C).** Store the model's per-turn `interpretation` when
  available; derive a fallback from narration on the fly when it is not.
- **Repetition output (B).** Emit a warning only when a motif is clearly shared,
  and list the shared keywords then. No output when nothing repeats (zero noise).

## Data flow & storage

- On `AcceptGeneration`, build one digest line and push it onto
  `game.recent_summaries` (capped to the most recent 12, FIFO).
  - Digest line = `summary.interpretation` if non-empty, else
    `first_clause(draft.narration)`, optionally suffixed with the net
    relationship-delta direction from the applied patch (e.g. `· 호감↑`).
- `recent_summaries` lives inside `GameState`, which is already the keystone of
  `SavePayload` → it persists and restores with **no schema change and no
  migration**. Old saves load with an empty `recent_summaries`; the render-time
  fallback derives digests from history narration, so the feature works
  immediately even for in-progress games.

## Prompt structure (change)

```
… # Recent turns        (last 4, full prose — unchanged)
# Recent arc            (NEW — last 4 turns, one semantic line each)
# 반복 감지             (NEW — only when a motif is clearly shared)
… # Choice roles        (unchanged)
```

The previously added surface-level `# Anti-repeat` (forbidden opening phrases)
is **removed**: the arc digest + repetition note is strictly richer and
subsumes it, keeping prompt size bounded. The `# Earlier story` recap (turns
before the 4-window) is unchanged.

## Repetition detection (deterministic)

Inputs: digests for the last `N_DETECT = 6` turns (stored value if present and
index-aligned, else derived from history narration).

1. Tokenize each digest on whitespace / punctuation.
2. Light Korean normalization: strip a trailing particle from
   `{는,가,이,을,를,에,의,도,와,과}` (so `유나가`/`유나는` → `유나`).
3. Drop tokens shorter than 2 chars and a small Korean stopword set
   (verb endings / glue words: 했다, 있다, 없다, 그, 이, 저, 등, 및, 수, 것, 때, …).
4. A token is "shared" if it occurs in ≥ 2 of the `N_DETECT` digests.
5. If ≥ 1 shared token exists → emit
   `# 반복 감지: <up to 5 tokens> · 이 흐름이 반복되고 있다. 새 압력·방향으로 한 가지 관찰 가능한 변화를 전진시킨다.`
   Otherwise emit nothing.

Korean morphology is not analyzed, so extraction is imperfect by design — the
"clearly shared (≥2 turns)" bar + stopword filter keeps false positives low, and
the model still has the arc digest itself to judge from.

## Edge cases

- Empty history / 1-turn session → omit `# Recent arc` entirely (byte-identical
  to today for the very first turn).
- `interpretation == None` → fallback to narration first-clause.
- Old save (empty `recent_summaries`) → render-time fallback from history.
- Short session (≤ window) → golden snapshot gains only the arc lines (no
  repetition note, since few digests won't share content tokens).

## Testing

- Pure-fn unit tests (new module): digest formatting, tokenizer + particle
  strip, shared-keyword extraction, the ≥2-turn threshold, "no shared ⇒ none".
- `prompt_context` integration tests: arc renders from `recent_summaries`;
  repetition note appears when digests share a motif; note omitted otherwise;
  empty history has no arc.
- Engine test: `AcceptGeneration` pushes exactly one digest line into
  `recent_summaries` and the cap holds.
- Update the golden prompt snapshot (`# Anti-repeat` removed, `# Recent arc`
  added).

## Out of scope

- No extra LLM summarization call.
- No change to event-driven long-term memory.
- No save-schema migration.
