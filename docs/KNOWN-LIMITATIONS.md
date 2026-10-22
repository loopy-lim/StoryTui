# Known Limitations

A frank list of what StoryTUI does **not** do as of W11. Read this alongside
[RELEASE-RUNTIME-GATE.md](./RELEASE-RUNTIME-GATE.md) (the open runtime
items) and [INSTALL.md](./INSTALL.md).

## Interactive host loop is connected

The binary `apps/storytui` now runs the interactive event pump:
`crossterm` event stream → `App::handle_key` → `EngineCommand` → `reduce` →
`provider.generate_turn` → `AcceptGeneration` → publish view. Provider
failures restore the last stable checkpoint and render a recoverable error.
Automated coverage already proves:

- The full engine flow against a `MockProvider` over a 100-turn synthetic
  soak (`crates/story-core/tests/soak.rs`).
- A 3-turn rainy-reunion Mock E2E that exercises opening + 3 player turns,
  cancel-mid-generation, and a malformed-draft recovery
  (`apps/storytui/tests/mock_e2e.rs`).
- The diagnostic bundle assembly + redacted serialization
  (`apps/storytui/tests/diagnostic_bundle.rs`).
- The deterministic benchmark runner
  (`apps/storytui/tests/bench.rs`).

The host loop is playable with a reachable, authenticated OpenAI-compatible
server. A live first-scene generation has been verified against the checked-in
OMLX profile. A real 100-turn session is still an open release gate because the
automated long-run suite uses MockProvider.

## MVP quality metrics are pending a real model

Every numeric quality target (prose, voice, rule respect, sovereignty) in
the PRD requires a real MLX server. The Mock cannot score on these axes —
its drafts are scripted. The code-level gates that ARE in place:

- 0 sensitive-text leaks in default logs/diagnostics (`tests/redact.rs`).
- Deterministic prompt + patch hashes (`tests/bench.rs`).
- 100-turn synthetic soak + 100-iteration save round-trip.

The runtime trace records only structural provider metadata, never a raw model
response excerpt, and is capped at 1 MiB. Pack file reads resolve canonical
targets and reject paths outside the canonical Pack root. Save slot ids are
portable filename stems (`[A-Za-z0-9_-]`, 1–64 characters), not paths.

Closing the real-model quality gate is documented in
`RELEASE-RUNTIME-GATE.md` (G3).

## Model license and safe-pack principles

StoryTUI is built for local, player-controlled fiction. The project itself
ships no model weights and takes no position on which MLX model an operator
chooses to load. Two principles the code enforces:

1. **Supported ratings only.** A pack must use `general` or `teen`. The
   checked-in packs and prompts target an all-ages presentation, and the
   loader rejects unsupported rating values before play starts.
2. **Player sovereignty.** The system prompt (see
   `crates/story-core/src/prompt_context.rs`) instructs the model not to
   fix emotions, dialogue, physical contact, forgiveness, confessions, or
   decisions the player did not specify. Free-text actions reinforce this
   in Korean. A model that ignores this instruction is a model-quality
   issue the engine cannot fully prevent.

## Streaming is the two-stage path, not the single-pass default

The `OpenAiCompatibleProvider` implements both `direct` (Director) and
`write_stream` (Writer SSE) — see W10. The single-call path
(`generate_turn`) does not stream tokens to the renderer: its response is
one structured JSON draft (narration + choices + patch), so there is no
prose to reveal early — the loading overlay reflects the whole call. The
streaming experience lives in the optional 2-stage Director → Writer path
(`EngineState::use_director_writer`): the Director plans silently, then the
Writer's narration streams into the transcript live. Players toggle that
path in Settings (`g`) or persist it in `user-settings.toml`; the shipped
default stays single-pass (ADR-005) until the real-model A/B gate passes.

## Expression sprites are keyword-classified, not model-judged

Character expression sprites (`pack/images/expressions/<character-id>/<emotion>.png`,
with the character portrait as the fallback) are picked by a zero-inference
keyword classifier over each turn's narration (`story_tui::classify_emotion`)
— deliberately NOT by a separate classifier model the way SillyTavern's
Expressions extension runs one. A weak local setup pays no extra generation
for expressions, and a misread degrades to "no sprite / neutral", never to
wrong story state. The tradeoff: the rule table reads surface cues only
(Korean-led, a few English cues) — sarcasm, negation, and who is speaking are
out of reach, and narration in other languages classifies as neutral. The
Settings `f` toggle turns sprites off without touching scene images (`i`).

## Cross-pack runtime evidence is still limited

Two packs are committed: `stories/rainy-reunion` and
`stories/starlight-station`. Both pass loader validation, while only
`rainy-reunion` has been exercised through the app-level integration path.
The 10-turn live-model swap to `starlight-station` remains runtime gate G5.

Pack loading now fails closed for future `min_engine_version`, choice counts
outside 1–6, duplicate identifiers, invalid numeric/stat ranges, and locations
that cannot be reached from the initial location through declared adjacency.
This is conservative structural validation, not a proof that every conditional
event or ending will be reached in a real playthrough.

## Non-goal: cloud providers

The provider abstraction is `OpenAiCompatible`. Cloud providers that need
proprietary auth flows, SDKs, or non-OpenAI wire shapes are intentionally
out of scope. A cloud provider that speaks the OpenAI shape (e.g.
`openrouter`) will work; one that does not, will not.

## Settings are profile-based

The current Settings contract is a TOML profile plus CLI and per-save-directory
user overrides. The in-app model picker reads `/v1/models` and persists the
main selection to `saves/user-settings.toml`; Director/Choice/Writer
assignments are per provider profile — keyed by profile name in
`[stage_models.<name>]`, restored when the profile is switched to, with
`[[providers]]` entries able to carry per-profile defaults (precedence:
user-settings profile entry, then the profile default, then the file-wide
default). Old flat `director_model`/`choice_model`/`writer_model` user
settings are folded into the active profile on load. If a persisted model
disappears, startup and switches repair it to an advertised model. Provider
URL, authentication, sampling, timeouts, retry budget, context budget, and
save directory still come from the base profile or CLI. Editing those values
in the TUI remains a future UX improvement.
