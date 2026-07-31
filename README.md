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

## CI commands

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
./scripts/check-core-deps.sh
```

`scripts/check-core-deps.sh` enforces ADR-003 by failing if `story-core` ever pulls in `ratatui`, `crossterm`, or `reqwest`.
