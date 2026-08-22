# Release Runtime Gate — OPEN

**Status as of W11 (2026-08-01):** every code-achievable milestone (W0–W11)
is complete and tested. The items below are **runtime-only**: they require
live hardware, a running MLX server, and/or human scoring. They are
explicitly **NOT done** by the code in this branch. A release cut must close
each one and record the artifact (log, score sheet, binary, checksum) under
`docs/release/<date>/`.

## Why these are not code

Each item below is bounded by something code cannot deliver on its own:

- A real model introduces non-determinism, latency, and content the test
  suite cannot predict.
- A "clean machine" install requires a host without the toolchain already
  present.
- A blind Korean evaluation requires human readers who have not seen the
  packs.
- An Apple-Silicon binary requires building on the target hardware with the
  target toolchain.
- A second-Pack swap requires live 10-turn evidence with the committed second
  pack, not only loader tests.

## Gate checklist

### G0 — Per-model Korean / structured / safe-content smoke

**Goal:** reject a model configuration before a long playtest if either the
Writer or lightweight choice model is missing, cannot satisfy StoryTUI's JSON
contract, emits no Korean, violates the all-ages story policy, or
collapses the three choices into duplicate/passive variants.

```sh
STORYTUI_MLX_URL=http://127.0.0.1:8000 \
STORYTUI_MLX_MODEL='<writer-model-id>' \
STORYTUI_MLX_CHOICE_MODEL='<choice-model-id>' \
cargo test -p story-provider --test mlx_smoke -- --ignored --nocapture
```

**Pass criterion:** health/model-list/basic chat, compact choice-model gate,
and full Writer turn gate all pass. Record model revisions, profile settings,
wall time, token usage, and whether speculative decoding actually loaded (a
silent native-engine fallback is not a DFlash pass).

### G1 — Real 100-turn session + server-kill / app-restart

**Goal:** prove the engine survives a full 100-turn playthrough against a
real MLX provider, plus an abrupt server kill mid-generation followed by an
app restart that restores the autosave checkpoint.

**Reproduction commands (operator):**

```sh
# 1. Start the MLX server (separate terminal) on the loopback default.
#    e.g. `mlx_lm.server --model mlx-community/<model> --host 127.0.0.1 --port 8080`

# 2. Point the app at it and start a new game. The host loop is wired; the
#    remaining gate is real-session evidence, not CLI plumbing.
cargo run --release -p storytui -- \
    --provider-config ./path/to/your/provider.toml \
    --pack ./stories/rainy-reunion

# 3. Play 100 turns. Verify:
#    - no patch_unknown_flag / rule_violation storms;
#    - autosave slot advances each turn;
#    - affection / trust / tension stay in pack-defined ranges.

# 4. Halfway through a turn (after the loading overlay appears), kill the
#    MLX server with Ctrl+C. Wait for the recoverable-error overlay, then
#    restart the server. Confirm:
#    - the autosave restores the last committed state;
#    - the next SubmitChoice re-issues the generation;
#    - no late-discard surfaces a stale draft.
```

**Pass criterion:** 100 turns committed; autosave checkpoint restores
byte-identical to the pre-kill state; no unrecoverable error code paths
triggered.

**Existing test coverage (NOT a substitute):**
`crates/story-core/tests/soak.rs` runs a 100-turn synthetic soak with the
`MockProvider`; `crates/story-persistence/tests/save_round_trip.rs` runs a
100-iteration save round-trip. Neither touches a real server.

During the run, also inspect the long-context behavior described in
[`CONTEXT-MANAGEMENT.md`](./CONTEXT-MANAGEMENT.md): the newest two turns remain
raw, earlier turns use semantic digests, old choices/scene descriptions do not
reappear as if new, and latency does not jump when the summary FIFO reaches 12.

### G2 — Clean-machine install repro

**Goal:** starting from a host that has never seen Rust, install the
toolchain, clone, build, and run a 3-turn Mock session end-to-end.

**Reproduction commands (operator):**

```sh
# On a fresh macOS 15+ Apple-Silicon machine:
xcode-select --install
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
git clone <repo> && cd StoryTui
cargo test --workspace        # green
cargo run -p storytui         # live provider is a separate runtime dependency
```

**Pass criterion:** the workspace test suite is green on a host that has
never had Rust installed. Record the rustc version, macOS build number, and
full `cargo test --workspace` summary.

### G3 — Blind Korean evaluation ≥ 3.8 / 5

**Goal:** three Korean-speaking evaluators who have not seen the source
packs rate the model's narration across at least 3 packs at ≥ 3.8 / 5
average on a rubric covering (a) prose quality, (b) character voice, (c)
rule respect, (d) player sovereignty (no unprompted confessions / contact /
forgiveness).

**Reproduction commands (operator):**

```sh
# Per evaluator (blind: evaluator does not see source code or the packs):
cargo run --release -p storytui -- --pack ./stories/<pack-id>
# Play 10 turns per pack. Record the four sub-scores per turn; average.

# Aggregate: pass if mean across evaluators × packs × turns ≥ 3.80 on every
# sub-axis. Archive the score sheet under docs/release/<date>/eval.csv.
```

**Pass criterion:** ≥ 3.8 / 5 on every sub-axis, per pack.

### G4 — Apple Silicon binary + checksum

**Goal:** ship a stripped `storytui` binary built on Apple Silicon plus a
SHA-256 checksum a player can verify before running.

**Reproduction commands (operator):**

```sh
# On an Apple-Silicon host with the pinned toolchain:
cargo build --release -p storytui
strip target/release/storytui
shasum -a 256 target/release/storytui > target/release/storytui.sha256

# Verify on a player machine:
shasum -a 256 -c storytui.sha256
```

**Pass criterion:** binary runs on a clean Apple-Silicon macOS 15+ host;
the checksum verifies; bundle the binary + checksum + this gate document.

### G5 — Second-Pack swap

**Goal:** load the committed second pack, `starlight-station`, and play it for
≥ 10 turns without code changes. Proves the engine is not over-fit to one
pack.

**Reproduction commands (operator):**

```sh
# Validate both committed packs load:
cargo test -p story-persistence --test rainy_reunion
cargo test -p story-persistence --test starlight_station

# Run:
cargo run --release -p storytui -- --pack ./stories/starlight-station
# Play 10 turns. Confirm:
#    - choices render correctly;
#    - no patch_unknown_flag (the pack's flags are honored);
#    - the validator's stat clamps fire as the pack defines.
```

**Pass criterion:** 10 turns committed on the second pack; no engine code
changes between the two pack runs.

## When this document changes

- An item moves from OPEN to DONE: append the artifact path
  (`docs/release/<date>/...`) next to it. **Do not delete the item** — the
  history of when each gate closed is part of the release record.
- A new runtime gate is identified: add a new `G<n>` section with the same
  shape (goal, reproduction commands, pass criterion).
