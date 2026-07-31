# W0 Workspace Skeleton Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Stand up a building Rust 2024 workspace with 5 library crates + 1 binary, correct dependency direction, a forbidden-dependency check, and repeatable fmt/clippy/test commands — so every later milestone starts from a green baseline.

**Architecture:** A Cargo workspace at the repo root. `story-model` is the leaf (no UI/HTTP/FS). `story-core` depends only on `story-model` (no Ratatui/Crossterm/Reqwest/FS — CI-enforced). `story-provider` adds HTTP; `story-persistence` adds FS/checksum; `story-tui` adds the terminal UI. The `apps/storytui` binary wires everything together. ADR-003 (UI/Core separation) is the load-bearing constraint.

**Tech Stack:** Rust 1.97.1 / edition 2024, Cargo workspace. (See version table below.)

---

## Reference: pinned dependency versions (Technical Spec §2.2, 2026-07-31)

| crate | version |
|---|---|
| ratatui | 0.30.2 |
| crossterm | 0.29.0 |
| tokio | 1.53.1 |
| reqwest | 0.13.4 |
| serde | 1.0.229 |
| serde_json | 1.0.151 |
| toml | 1.1.4 |
| thiserror | 2.0.19 |
| anyhow | 1.0.104 |
| tracing | 0.1.44 |
| tracing-subscriber | 0.3.23 |
| insta | 1.48.0 |
| async-trait | 0.1.91 |
| uuid | 1.24.0 |
| chrono | 0.4.45 |
| unicode-width | 0.2.2 |
| unicode-segmentation | 1.13.3 |
| jsonschema | 0.49.2 |
| tokio-util | 0.7.19 |
| futures-util | 0.3.33 |
| sha2 | 0.11.0 |
| tempfile | 3.27.0 |
| semver | 1.0.28 |

**Version-resolution rule:** use these as caret targets (`=x.y.z` not required). If a specific pin does not resolve in the registry, fall back to the latest compatible release of that major and record the deviation in the W0 completion note.

---

## Crate dependency direction (immutable)

- `story-model` → serde, serde_json, semver, uuid, chrono, thiserror. **NO UI / HTTP / FS.**
- `story-core` → `story-model`, thiserror. **NO ratatui / crossterm / reqwest / filesystem.**
- `story-provider` → `story-model`, `story-core`, reqwest, tokio, async-trait, tokio-util, futures-util.
- `story-persistence` → `story-model`, `story-core`, serde_json, sha2, tempfile, semver, toml.
- `story-tui` → ratatui, crossterm, tokio, `story-model`, `story-core`.
- `apps/storytui` → all five crates + tokio (full) + tracing + tracing-subscriber.

---

## Task 1: Initialize git repo and root layout

**Files:**
- Create: `.gitignore`
- Create: `rust-toolchain.toml`

**Step 1:** `git init` at repo root (this is a greenfield project; main branch is acceptable).
**Step 2:** Write `.gitignore` ignoring `/target`, `/saves`, `Cargo.lock` is **kept** (binary app), IDE files.
**Step 3:** Write `rust-toolchain.toml` pinning stable channel, `components = ["rustfmt", "clippy"]`.
**Step 4:** Commit: `chore: init repo and toolchain`.

`rust-toolchain.toml` content:
```toml
[toolchain]
channel = "1.97.1"
components = ["rustfmt", "clippy"]
edition = "2024"
```

---

## Task 2: Workspace manifest and member crate skeletons

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `crates/story-model/{Cargo.toml,src/lib.rs}`
- Create: `crates/story-core/{Cargo.toml,src/lib.rs}`
- Create: `crates/story-provider/{Cargo.toml,src/lib.rs}`
- Create: `crates/story-persistence/{Cargo.toml,src/lib.rs}`
- Create: `crates/story-tui/{Cargo.toml,src/lib.rs}`
- Create: `apps/storytui/{Cargo.toml,src/main.rs}`

**Step 1:** Write root `Cargo.toml` with `[workspace] members = [...]`, `[workspace.package]` (edition 2024, rust-version), `[workspace.dependencies]` declaring every shared crate at the pinned version so members reference them by `workspace = true`.
**Step 2:** Write each member `Cargo.toml` with the exact dependency direction above, using `dep.workspace = true`.
**Step 3:** Each `src/lib.rs` is a module-doc-only stub describing the crate's responsibility (one short paragraph) and `#![warn(clippy::all)]`. `apps/storytui/src/main.rs` prints `storytui 0.1.0 — workspace skeleton` and exits 0.
**Step 4:** `cargo build --workspace` — expect success.
**Step 5:** Commit: `chore: workspace skeleton with 5 libs + binary`.

Root `Cargo.toml` skeleton:
```toml
[workspace]
resolver = "3"
members = [
    "crates/story-model",
    "crates/story-core",
    "crates/story-provider",
    "crates/story-persistence",
    "crates/story-tui",
    "apps/storytui",
]

[workspace.package]
edition = "2024"
rust-version = "1.88"
license = "MIT OR Apache-2.0"

[workspace.dependencies]
# pinned versions here, referenced by members via workspace = true
serde = { version = "1.0.229", features = ["derive"] }
serde_json = "1.0.151"
semver = { version = "1.0.28", features = ["serde"] }
uuid = { version = "1.24.0", features = ["v4", "serde"] }
chrono = { version = "0.4.45", features = ["serde"] }
thiserror = "2.0.19"
anyhow = "1.0.104"
toml = "1.1.4"
tracing = "0.1.44"
tracing-subscriber = "0.3.23"
async-trait = "0.1.91"
reqwest = { version = "0.13.4", default-features = false, features = ["json", "rustls-tls"] }
tokio = { version = "1.53.1", features = ["full"] }
tokio-util = "0.7.19"
futures-util = "0.3.33"
sha2 = "0.11.0"
tempfile = "3.27.0"
ratatui = "0.30.2"
crossterm = { version = "0.29.0", features = ["event-stream"] }
unicode-width = "0.2.2"
unicode-segmentation = "1.13.3"
jsonschema = "0.49.2"
insta = "1.48.0"

# internal crates
story-model = { path = "crates/story-model" }
story-core = { path = "crates/story-core" }
story-provider = { path = "crates/story-provider" }
story-persistence = { path = "crates/story-persistence" }
story-tui = { path = "crates/story-tui" }

[profile.release]
lto = "thin"
```

---

## Task 3: Test directory scaffolding

**Files:**
- Create: `tests/fixtures/.gitkeep`
- Create: `tests/contracts/.gitkeep`
- Create: `tests/soak/.gitkeep`
- Create: `benches/.gitkeep`

**Step 1:** Create the three test dirs + benches with `.gitkeep`.
**Step 2:** Commit: `chore: scaffold test/fixture/bench directories`.

---

## Task 4: Forbidden-dependency check

**Files:**
- Create: `scripts/check-core-deps.sh`

**Step 1:** Write a script that runs `cargo tree -p story-core --all-targets` and fails (non-zero) if the output contains `ratatui`, `crossterm`, `reqwest`, or `std::fs`-style filesystem crates (`tokio` FS features are also disallowed in story-core). This encodes ADR-003 / NFR-007.
**Step 2:** `chmod +x` and run it; expect pass (story-core only depends on story-model + thiserror).
**Step 3:** Commit: `chore: add story-core forbidden-dependency check`.

```sh
#!/usr/bin/env sh
set -eu
out="$(cargo tree -p story-core --all-targets 2>/dev/null)"
forbidden='ratatui crossterm reqwest'
for f in $forbidden; do
  if echo "$out" | grep -q "$f"; then
    echo "FAIL: story-core depends on forbidden crate '$f'" >&2
    echo "$out" >&2
    exit 1
  fi
done
echo "OK: story-core has no UI/HTTP dependencies"
```

---

## Task 5: README index and crate responsibility docs

**Files:**
- Create: `README.md` (repo root — short overview + the exact CI command block)
- Modify: each `src/lib.rs` already has a doc comment from Task 2.

**Step 1:** Write repo `README.md` with project one-liner, crate map, and the CI command block:
```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
./scripts/check-core-deps.sh
```
**Step 2:** Commit: `docs: workspace README and CI commands`.

---

## Completion evidence (W0 gate, Handoff §4.W0 / Acceptance §20)

- `cargo fmt --check` → clean
- `cargo clippy --workspace --all-targets -- -D warnings` → no warnings
- `cargo test --workspace` → passes (no tests yet, but compiles + runs)
- `cargo tree -p story-core` → no ratatui/crossterm/reqwest (verified by `scripts/check-core-deps.sh`)
- `cargo build --workspace` → succeeds
- `apps/storytui` runs and prints the skeleton banner

Record any dependency-version deviations from the table at the top of this plan.
