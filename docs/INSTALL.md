# Installation

StoryTUI is a Korean interactive-fiction terminal UI that talks to a local,
OpenAI-compatible LLM server (the project targets an Apple-Silicon MLX
server, but any OpenAI-compatible endpoint works). This document lists the
prerequisites, how to build, and how to run.

> **Status:** W0–W11 code-complete. The interactive host loop in
> `apps/storytui` is **not yet wired** — the engine, providers, persistence,
> TUI screens, and library surfaces (diagnostics, benchmark) are all
> integrated-tested, but the binary itself only prints a banner. See
> [RELEASE-RUNTIME-GATE.md](./RELEASE-RUNTIME-GATE.md) for the open runtime
> items and [KNOWN-LIMITATIONS.md](./KNOWN-LIMITATIONS.md) for the principle.

## Prerequisites

| Component | Version | Notes |
|---|---|---|
| macOS | 15+ | Apple Silicon recommended (MLX). The Settings screen targets 80×24 minimum. |
| Rust toolchain | 1.88+ (project tracks 1.97.1 in `rust-toolchain.toml`) | `rustup show` auto-installs the pinned toolchain. |
| Python | 3.11+ | Only if you intend to run the reference MLX server. |
| MLX server | any OpenAI-compatible endpoint | Default `base_url` is the loopback `http://127.0.0.1:8080`. |

## Build

```sh
# From the repository root:
cargo build --workspace              # debug
cargo build --workspace --release    # optimized (LTO = "thin")
```

The workspace builds six crates:

| Crate | Builds |
|---|---|
| `story-model` | Pure data + serde. |
| `story-core` | Reducer, validator, prompt context, redaction (`redact`). Depends only on `story-model + thiserror + sha2`. |
| `story-provider` | `MockProvider` + `OpenAiCompatibleProvider`. Owns `reqwest`. |
| `story-persistence` | Pack loader, atomic save, migration. |
| `story-tui` | `ratatui` screens (story, save/load, settings, keymap). |
| `storytui` (app) | Binary + `diagnostic` and `bench` library surfaces. |

## Run

The binary prints a banner today; the host loop is the open runtime item
(see [RELEASE-RUNTIME-GATE.md](./RELEASE-RUNTIME-GATE.md)). When wired, it
will read a provider config from TOML and start the TUI against the loaded
pack.

```sh
cargo run -p storytui
```

## Provider configuration

The defaults mirror Technical Spec §10.2 — a **loopback** endpoint. Set
`model` in your TOML; everything else fills in.

```toml
[provider]
kind        = "openai_compatible"
name        = "local-mlx-vlm"
base_url    = "http://127.0.0.1:8080"   # loopback default
chat_path   = "/v1/chat/completions"
health_path = "/health"
model       = "mlx-community/your-model"
temperature = 0.85
top_p       = 0.92
top_k       = 40
max_tokens  = 1800
structured_output = "auto"
```

### Loopback warning (ADR-002)

A non-loopback `base_url` (`192.168.*`, `10.*`, public hostnames, etc.) is
allowed but:

- The Settings screen renders `⚠ 주의: base_url이 루프백이 아닙니다 — 외부로
  전송될 수 있습니다.` next to the provider block.
- `OpenAiCompatibleProvider::new` emits a one-shot `tracing::warn!`.
- The diagnostic bundle records `is_loopback: false` (see `ProviderConfigSafe`).

Keep the default loopback URL unless you understand the exposure.

## Tests

```sh
cargo test --workspace                                # full suite
cargo test -p storytui --test bench                   # benchmark determinism
cargo test -p story-core --test redact                # no-leak audit
cargo test --workspace -- --ignored                   # real-MLX smoke (needs server)
./scripts/check-core-deps.sh                          # ADR-003 enforcement
```

The `--ignored` suite is the only test path that touches the network. It is
opt-in and skipped by default; point `[provider]` at a running MLX server
before invoking it.

## CI commands

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
./scripts/check-core-deps.sh
```
