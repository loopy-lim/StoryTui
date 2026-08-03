//! Diagnostic-bundle data types (ST-035).
//!
//! Pure data — no IO, no UI, no HTTP. Lives in `story-model` so both
//! `apps/storytui` (assembly) and `story-tui` (settings screen) can consume it
//! without depending on each other. See ADR-003.
//!
//! # No-leak contract
//!
//! [`DiagnosticBundle`] carries only metadata + a [`ProviderConfigSafe`] that
//! has NO secret fields by construction. [`DiagnosticBundle::provider_config_redacted`]
//! is the only provider surface ever serialized; the live API key never enters
//! this type. The `apps/storytui` assembly additionally routes the serialized
//! form through [`story_core::redact::redact`] before any write/print, so a
//! downstream error string that accidentally echoes a secret is still masked.

use serde::{Deserialize, Serialize};

use crate::{DomainErrorView, ProviderHealth};

/// Provider configuration with **no secret fields** — the only provider
/// surface ever carried in a [`DiagnosticBundle`]. Constructed by the host
/// from a live `ProviderConfig` by copying across only the safe fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfigSafe {
    /// Base URL of the OpenAI-compatible server (e.g. `http://127.0.0.1:8080`).
    pub base_url: String,
    /// Resolved model id (never contains an API key).
    pub model: String,
    /// Director model override if set.
    #[serde(default)]
    pub director_model: Option<String>,
    /// Writer model override if set.
    #[serde(default)]
    pub writer_model: Option<String>,
    /// Structured-output mode rendered as a stable lowercase token
    /// (`"auto"` / `"on"` / `"off"`).
    pub structured_output: String,
    /// Per-request deadline in milliseconds.
    pub request_timeout_ms: u64,
    /// TCP connect deadline in milliseconds.
    pub connect_timeout_ms: u64,
    /// Maximum retry attempts on transient failures.
    pub max_retries: u32,
    /// `true` iff `base_url` host is loopback (ADR-002). Surfaced so the
    /// settings/diagnostic UI can show a non-loopback warning without
    /// re-parsing the URL.
    pub is_loopback: bool,
}

/// A redacted, serializable snapshot of engine + provider + pack state for
/// bug reports and the `/diagnostic` command. See the module docs for the
/// no-leak contract.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticBundle {
    /// Engine / workspace version (`env!("CARGO_PKG_VERSION")`).
    pub engine_version: String,
    /// Rust toolchain version (`rustc --version`) if known.
    pub rust_version: Option<String>,
    /// Operating system label (`std::env::consts::OS`).
    pub os: String,
    /// Last known provider health, surfaced to the UI status bar.
    pub provider_health: Option<ProviderHealth>,
    /// Provider config with secrets stripped (see [`ProviderConfigSafe`]).
    pub provider_config_redacted: ProviderConfigSafe,
    /// Last recoverable error rendered for the player, if any.
    pub last_error: Option<DomainErrorView>,
    /// Loaded pack's `story_id`, if any.
    pub pack_story_id: Option<String>,
    /// Current turn counter, if a game is in progress.
    pub turn: Option<u32>,
    /// RFC-3339 timestamp the bundle was assembled.
    pub generated_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProviderHealth;

    fn sample() -> DiagnosticBundle {
        DiagnosticBundle {
            engine_version: "0.1.0".to_string(),
            rust_version: Some("rustc 1.88.0".to_string()),
            os: "macos".to_string(),
            provider_health: Some(ProviderHealth::Ready),
            provider_config_redacted: ProviderConfigSafe {
                base_url: "http://127.0.0.1:8080".to_string(),
                model: "mlx-community/test".to_string(),
                director_model: None,
                writer_model: None,
                structured_output: "auto".to_string(),
                request_timeout_ms: 180_000,
                connect_timeout_ms: 3_000,
                max_retries: 2,
                is_loopback: true,
            },
            last_error: None,
            pack_story_id: Some("rainy-reunion".to_string()),
            turn: Some(7),
            generated_at: "2026-08-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn bundle_round_trips() {
        let b = sample();
        let json = serde_json::to_string(&b).expect("serialize");
        let back: DiagnosticBundle = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, b);
    }

    #[test]
    fn provider_config_safe_has_no_secret_fields() {
        // Structural guarantee: the type itself cannot carry an API key.
        let safe = sample().provider_config_redacted;
        let json = serde_json::to_string(&safe).expect("serialize");
        assert!(
            !json.to_lowercase().contains("api_key"),
            "ProviderConfigSafe must not have an api_key field: {json}"
        );
        assert!(
            !json.to_lowercase().contains("authorization"),
            "ProviderConfigSafe must not have an authorization field: {json}"
        );
    }
}
