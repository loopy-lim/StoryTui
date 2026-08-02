# Known Limitations

A frank list of what StoryTUI does **not** do as of W11. Read this alongside
[RELEASE-RUNTIME-GATE.md](./RELEASE-RUNTIME-GATE.md) (the open runtime
items) and [INSTALL.md](./INSTALL.md).

## Interactive host loop is connected

The binary `apps/storytui` now runs the interactive event pump:
`crossterm` event stream → `App::handle_key` → `EngineCommand` → `reduce` →
`provider.generate_turn` → `AcceptGeneration` → publish view. Provider
failures restore the last stable checkpoint and render a recoverable error.
What remains to prove with runtime evidence:

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
server. A real 100-turn session is still an open release gate because the
automated long-run suite uses MockProvider.

## MVP quality metrics are pending a real model

Every numeric quality target (prose, voice, rule respect, sovereignty) in
the PRD requires a real MLX server. The Mock cannot score on these axes —
its drafts are scripted. The code-level gates that ARE in place:

- 0 sensitive-text leaks in default logs/diagnostics (`tests/redact.rs`).
- Deterministic prompt + patch hashes (`tests/bench.rs`).
- 100-turn synthetic soak + 100-iteration save round-trip.

Closing the real-model quality gate is documented in
`RELEASE-RUNTIME-GATE.md` (G3).

## Model license and teen-pack principles

StoryTUI is built for local, player-controlled fiction. The project itself
ships no model weights and takes no position on which MLX model an operator
chooses to load. Two principles the code enforces:

1. **teen content only with safe pack opt-in.** A pack must declare
   `all_major_characters_safe: true` and a non-`General` rating is the
   pack author's responsibility. The loader validates the pack schema, not
   the moral content.
2. **Player sovereignty.** The system prompt (see
   `crates/story-core/src/prompt_context.rs`) instructs the model not to
   fix emotions, dialogue, physical contact, forgiveness, confessions, or
   decisions the player did not specify. Free-text actions reinforce this
   in Korean. A model that ignores this instruction is a model-quality
   issue the engine cannot fully prevent.

## No streaming in the MVP binary path

The `OpenAiCompatibleProvider` implements both `direct` (Director) and
`write_stream` (Writer SSE) — see W10. The MVP single-call path
(`generate_turn`) does not stream tokens to the renderer; the loading
overlay only reflects the start/end of one generation call. The
`EngineState::use_director_writer` flag toggles the 2-stage path; the
settings UI does not yet expose it.

## Single-pack focus

The committed pack is `stories/rainy-reunion`. A second content-complete
pack is an open runtime gate (G5). The loader and validator are pack-shape
driven, not pack-specific, but only `rainy-reunion` has been exercised
end-to-end at the integration-test level.

## Non-goal: cloud providers

The provider abstraction is `OpenAiCompatible`. Cloud providers that need
proprietary auth flows, SDKs, or non-OpenAI wire shapes are intentionally
out of scope. A cloud provider that speaks the OpenAI shape (e.g.
`openrouter`) will work; one that does not, will not.

## Settings are profile-based

The current Settings contract is a TOML profile plus CLI overrides. The
renderer is still read-only, but `config/storytui.toml` supplies the provider
URL, selected model, authentication file/environment variable, sampling,
timeouts, retry budget, context budget, and save directory. In-app editing and
writing a profile remain a future UX improvement.
