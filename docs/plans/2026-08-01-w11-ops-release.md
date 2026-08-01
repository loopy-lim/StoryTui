# W11 Ops & Release Readiness Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Raise the project to release-candidate quality on the code-achievable axes: secret/content redaction with a no-leak audit, a redacted diagnostic bundle, a Settings/keymap screen with 80×24 min-size, and a deterministic benchmark runner. ST-035 (settings/health/privacy diagnostics), ST-036 (benchmark + release). **The runtime release gate (real 100-turn session, server-kill + app-restart, clean-machine install, blind Korean eval, Apple Silicon binary + checksum) requires live hardware/human scoring and is recorded as an open runtime gate — NOT closed by this code.**

**Architecture:** Pure redaction logic in `story-core` (`redact` — masks known secrets, strips prompt/body/free-input from any text). Diagnostic bundle as pure data in `story-model`, assembled in `apps/storytui`. Benchmark runner as a `storytui bench` path / test in `apps/storytui` (deterministic, Mock-backed). Settings + keymap screens in `story-tui`. ADR-003 unchanged.

**Tech Stack:** story-core (+ sha2), story-tui (ratatui), apps/storytui (tokio, tracing). No new heavy deps.

---

## Part A — redaction (story-core, new `redact.rs`) — the no-leak audit
```rust
pub struct RedactionPolicy { pub secret_markers: Vec<String>, pub strip_sections: Vec<&'static str> }  // e.g. ["api_key","authorization","bearer"], sections ["system_prompt","narration","free_input"]
pub fn redact(text: &str, policy: &RedactionPolicy, known_secrets: &[&str]) -> String;
pub fn default_policy() -> RedactionPolicy;
```
- Mask any occurrence of a `known_secret` (replace with `***`).
- Mask values of `secret_markers` (e.g. `api_key=...` → `api_key=***`).
- Strip/replace the content of `strip_sections` (prompt/narration/free-input) so a diagnostic excerpt never echoes player body or full prompt.
- Default policy per Acceptance §20: default logs/diagnostics contain NONE of prompt/body/free-input/secret.
Tests (load-bearing): plant unique markers for an API key, a system prompt, a narration body, a free input; run `redact`; assert NONE survive. This is the "0 sensitive-text leaks in default logs" gate.

## Part B — diagnostic bundle (story-model type + apps/storytui assembly)
`story-model` new `diagnostic.rs`:
```rust
pub struct DiagnosticBundle {
    pub engine_version: String, pub rust_version: Option<String>, pub os: String,
    pub provider_health: Option<ProviderHealth>, pub provider_config_redacted: ProviderConfigSafe,
    pub last_error: Option<DomainErrorView>, pub pack_story_id: Option<String>, pub turn: Option<u32>,
    pub generated_at: String,
}
pub struct ProviderConfigSafe { pub base_url: String, pub model: String, pub structured_output: String, /* NO api_key, timeouts only */ }
```
`apps/storytui`: `fn build_diagnostic_bundle(state, provider, pack) -> DiagnosticBundle` (pure-ish assembly; provider config passed in already sans-secret), then serialize through `redact::redact` before any write/print. Test: a bundle assembled with a fake API key in the environment is serialized with zero secret leakage.

## Part C — settings + keymap screens (story-tui)
- `screens/settings.rs`: render provider (base_url/model/structured_output, secrets MASKED), sampling (temp/top_p/top_k/max_tokens), keymap reference, theme placeholder. Min-size guard (80×24). Snapshot tests.
- `screens/keymap.rs` (or a section): the key table from Technical Spec §14.1.
- Non-loopback provider URL shown with a Korean warning in settings (ties to W6).
- Health display: map `ProviderHealth` → Korean label + color (Ready/Degraded/Unavailable/Misconfigured/Loading).

## Part D — benchmark runner (apps/storytui)
A deterministic benchmark over a fixed prompt bundle:
- `apps/storytui` `bench` module (or a `tests/bench.rs`): load rainy-reunion, run N turns against a deterministic Mock on a FIXED prompt bundle, record per-turn: prompt hash (from PromptContext), sampling params, result patch digest, wall-time. Assert reproducibility (same bundle ⇒ same prompt hashes + same result digests across runs — determinism of the pipeline).
- Records metadata (model id placeholder, sampling, seed-feasibility note) so a real run can be dropped in later.
- NO real model in normal tests (Mock only). A `#[ignore]` real-provider bench is allowed but not required.
This satisfies "benchmark reproduces identical prompt bundles" at the code level.

## Part E — release docs (docs/)
- `docs/INSTALL.md`: prerequisites (macOS 15+, Rust, Python venv, MLX server), build, run, the loopback provider default + non-loopback warning, the `--ignored` real-MLX smoke.
- `docs/RELEASE-RUNTIME-GATE.md`: the OPEN runtime gate checklist (real 100-turn + server-kill/restart, clean-machine install repro, blind Korean eval ≥3.8/5, Apple Silicon binary + checksum, second-Pack swap) — explicitly NOT done, with the exact reproduction commands a future operator runs.
- `docs/KNOWN-LIMITATIONS.md`: model-license/teen-pack principles, the `apps/storytui` interactive host loop not yet wired (library-tested via integration tests + 3-turn Mock E2E), MVP quality metrics pending real model.
- Update root `README.md` with a status section: milestones W0–W11 code-complete; runtime gates open.

## Tasks (TDD)
- **W11.1:** `redact.rs` + no-leak audit tests. Commit `feat(story-core): redaction policy and no-leak audit`.
- **W11.2:** diagnostic bundle type + assembly + redacted-serialize test. Commit `feat: redacted diagnostic bundle`.
- **W11.3:** settings + keymap screens + health display + snapshots. Commit `feat(story-tui): settings, keymap, and health screens`.
- **W11.4:** benchmark runner (deterministic, Mock) + reproducibility test. Commit `feat(storytui): deterministic benchmark runner`.
- **W11.5:** release docs (INSTALL, RELEASE-RUNTIME-GATE, KNOWN-LIMITATIONS, README status). Commit `docs: install, runtime-gate checklist, known limitations, status`.

## Completion gate (ST-035/036) — code/docs only
- `cargo test --workspace` green; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --check` clean.
- `./scripts/check-core-deps.sh` OK; `cargo tree -p story-core` unchanged.
- No-leak audit passes (0 prompt/body/free-input/secret markers survive `redact`).
- Diagnostic bundle serializes with zero secret leakage.
- Settings/keymap screens render at 80×24 (snapshots).
- Benchmark runner reproduces identical prompt hashes + result digests (determinism).
- **Explicitly NOT done (runtime gate):** real 100-turn + server-kill/restart, clean-machine install, blind Korean eval, Apple Silicon binary + checksum, second-Pack swap — documented in RELEASE-RUNTIME-GATE.md.
