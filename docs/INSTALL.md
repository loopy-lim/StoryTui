# Installation

StoryTUI is a Korean interactive-fiction terminal UI that talks to a local,
OpenAI-compatible LLM server (the project targets an Apple-Silicon MLX
server, but any OpenAI-compatible endpoint works). This document lists the
prerequisites, how to build, and how to run.

> **Status:** W0–W11 host loop is wired. The binary loads a provider profile
> and Story Pack, starts the opening generation, renders the TUI, handles
> provider results/errors, and autosaves committed turns. Live-model quality,
> clean-machine, 100-turn, and release checks remain open; see
> [RELEASE-RUNTIME-GATE.md](./RELEASE-RUNTIME-GATE.md).

## Prerequisites

| Component | Version | Notes |
|---|---|---|
| macOS | 15+ | Apple Silicon recommended (MLX). The Settings screen targets 80×24 minimum. |
| Rust toolchain | 1.88+ (project tracks 1.97.1 in `rust-toolchain.toml`) | `rustup show` auto-installs the pinned toolchain. |
| Python | 3.11+ | Only if you intend to run the reference MLX server. |
| MLX server | any OpenAI-compatible endpoint | Checked-in OMLX profile uses `http://127.0.0.1:8000`. |

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

The checked-in profile intentionally contains no machine-specific model ID —
you do not need to edit any config to start. The app opens the start menu
directly and wires the model itself:

- **First run**: with no model selected, the app probes the configured
  endpoint and then the common local OpenAI-compatible endpoints
  (`127.0.0.1:8080`, `:1234`, `:11434`), filters out non-chat models
  (embedding/rerank/audio), and auto-selects one. The model ID is persisted
  to `saves/user-settings.toml` (the URL never is), so later runs start with
  your selection immediately.
- **No server running**: the start menu still opens, with a one-line Korean
  status notice. Turn your model server on, then open 설정 (Settings) — the
  app retries auto-setup there.
- **Model changes**: press `o`, then `p`, to open the server-advertised model
  picker; `Enter` selects the main model, while `d` and `w` select the
  optional Director and Writer models. `c` selects the lightweight model used
  only for a rejected/manual choice rewrite. If a saved model is no longer
  advertised at startup, it falls back to the private local profile model (or
  the first advertised model) and repairs `saves/user-settings.toml`.

```sh
cargo run -p storytui
```

The app starts at the 시작 메뉴 (start menu): 새 게임 / 이어하기 / 설정 /
카드 가져오기 / 종료 / **팩 선택**. Use `↑↓` + `Enter` to navigate — pack
switching lives in 팩 선택, which lists the story packs found next to the
current one. In the story screen, `1..9` or `Enter` choose, `j/k` or arrow
keys move, `c` cancels a generation, `Esc` dismisses an error, and
`q`/`Ctrl+C` quits.

To run the live model contract gates (they remain ignored in normal CI):

```sh
STORYTUI_MLX_URL=http://127.0.0.1:8000 \
STORYTUI_MLX_MODEL='<writer-model-id>' \
STORYTUI_MLX_CHOICE_MODEL='<choice-model-id>' \
cargo test -p story-provider --test mlx_smoke -- --ignored --nocapture
```

The gates verify advertised model IDs, Korean generation, the exact StoryTUI
JSON contracts, a compact three-choice response, distinct active choices and
risk levels, previews, and a supported all-ages scene. They do not
replace the longer human-scored release gates.

CLI overrides are available for one-off runs:

```sh
cargo run -p storytui -- \
  --provider-config ./config/storytui.local.toml \
  --pack ./stories/rainy-reunion \
  --model local-model-id \
  --base-url http://127.0.0.1:8000
```

`--save-dir` also selects which `user-settings.toml` is loaded, so an isolated
run does not inherit model choices from the default `./saves` directory.

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
api_key_file = "omlx-api-key" # config/omlx-api-key, untracked secret file
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

OMLX may require authentication for chat completions. Keep the token out of
TOML and put only the raw token in the ignored file referenced by
`api_key_file`:

```sh
printf '%s\n' '<your-omlx-api-key>' > config/omlx-api-key
chmod 600 config/omlx-api-key
cargo run -p storytui
```

The path is relative to the provider TOML directory. The provider sends the
token as `Authorization: Bearer ...`; diagnostics, saves, and the Settings
renderer never include it. `api_key_env` remains available as an alternative
for CI or shell-managed environments.

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
