# StoryTUI

A Korean interactive-fiction terminal UI powered by a local MLX LLM, written in Rust on `ratatui`.

## Crate map

| Crate | Path | Responsibility |
|---|---|---|
| `story-model` | `crates/story-model` | Domain model: story, scene, choice, memory, and state value types. Pure `serde` data — no UI, HTTP, or filesystem. |
| `story-core` | `crates/story-core` | Deterministic story core: reducer, validators, rules engine, condition DSL, memory selector, prompt context. Depends only on `story-model` (ADR-003: no UI/HTTP/FS). |
| `story-provider` | `crates/story-provider` | LLM provider abstraction and the local MLX HTTP client. Owns all `reqwest` network interaction. |
| `story-persistence` | `crates/story-persistence` | Story-pack loading, content-addressed cache, atomic save files, schema migrations. All filesystem IO isolated here. |
| `story-tui` | `crates/story-tui` | `ratatui` + `crossterm` rendering, Korean text shaping, and the async event pump. |
| `storytui` | `apps/storytui` | Binary entrypoint that wires the crates above into a running app. |

## Status

Milestones **W0–W11 are code-complete** (engine, providers, persistence,
TUI screens, redaction, diagnostic bundle, settings/keymap screen,
deterministic benchmark). All gates below are green:

- `cargo test --workspace` — full suite green (500+ tests).
- `cargo clippy --workspace --all-targets -- -D warnings` — clean.
- `cargo fmt --check` — clean.
- `./scripts/check-core-deps.sh` — ADR-003 intact (`story-core` =
  `story-model + thiserror + sha2`).

**Open runtime gates** (require live hardware / human scoring, NOT closable
by code) — see [`docs/RELEASE-RUNTIME-GATE.md`](./docs/RELEASE-RUNTIME-GATE.md):

- G1 — real 100-turn session + server-kill / app-restart.
- G2 — clean-machine install repro.
- G3 — blind Korean evaluation ≥ 3.8 / 5.
- G4 — Apple-Silicon binary + checksum.
- G5 — second-Pack swap.

**Interactive host loop**: the binary prints a banner today; the event
pump + provider polling is library-tested via integration tests but not
yet wired into `main.rs`. See
[`docs/KNOWN-LIMITATIONS.md`](./docs/KNOWN-LIMITATIONS.md).

For build / run / test instructions, see
[`docs/INSTALL.md`](./docs/INSTALL.md).

## CI commands

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
./scripts/check-core-deps.sh
```

`scripts/check-core-deps.sh` enforces ADR-003 by failing if `story-core` ever pulls in `ratatui`, `crossterm`, or `reqwest`.
