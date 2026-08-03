//! Diagnostic-bundle assembly (ST-035).
//!
//! Pure-ish glue: takes already-resolved provider state (config OR a pre-built
//! [`ProviderConfigSafe`]) plus engine/pack metadata, and produces a
//! [`DiagnosticBundle`] that is safe to print or attach to a bug report. The
//! bundle's [`ProviderConfigSafe`] has NO secret fields by construction — the
//! live API key never enters the type. [`serialize_redacted`] additionally
//! routes the JSON form through [`story_core::redact`] with the caller's
//! `known_secrets` so any secret that accidentally surfaced in
//! `last_error.message_ko` (or anywhere else) is masked.
//!
//! The interactive host loop (reading keys, polling health on a timer) is not
//! wired here — see `docs/KNOWN-LIMITATIONS.md`. This module is the
//! library-tested core a future host loop calls.

use story_core::redact::{default_policy, redact};
use story_model::{DiagnosticBundle, DomainErrorView, ProviderConfigSafe, ProviderHealth};
use story_provider::{ProviderConfig, StructuredOutputMode, is_loopback_base_url};

/// Engine version stamped into every bundle. Mirrors the workspace version.
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Environment variables polled by [`collect_known_secrets`]. Any non-empty
/// value is added to the known-secret list for [`serialize_redacted`].
pub const SECRET_ENV_VARS: &[&str] = &[
    "OPENAI_API_KEY",
    "STORYTUI_API_KEY",
    "MLX_API_KEY",
    "ANTHROPIC_API_KEY",
    "STORYTUI_PROVIDER_TOKEN",
];

/// Provider input for [`build_diagnostic_bundle`]: either a live
/// [`ProviderConfig`] (secret fields will be dropped) or a pre-built
/// [`ProviderConfigSafe`] (used when restoring from a saved profile).
pub enum ProviderInput<'a> {
    /// Live provider config — secret fields are dropped while building
    /// [`ProviderConfigSafe`].
    Config(&'a ProviderConfig),
    /// Already-redacted safe view; copied verbatim.
    Safe(ProviderConfigSafe),
}

/// Wrapper carrying the most-recently-observed [`ProviderHealth`]. A newtype
/// so future host code can substitute a richer source without churning call
/// sites.
#[derive(Default)]
pub struct ProviderHealthInput(pub Option<ProviderHealth>);

impl From<Option<ProviderHealth>> for ProviderHealthInput {
    fn from(v: Option<ProviderHealth>) -> Self {
        Self(v)
    }
}

/// Read every environment variable listed in [`SECRET_ENV_VARS`] and return
/// the non-empty values. These are the literals [`serialize_redacted`] masks
/// wherever they appear.
pub fn collect_known_secrets() -> Vec<String> {
    let mut out = Vec::new();
    for var in SECRET_ENV_VARS {
        if let Ok(v) = std::env::var(var)
            && !v.is_empty()
        {
            out.push(v);
        }
    }
    out
}

/// Build a [`ProviderConfigSafe`] from a live [`ProviderConfig`], dropping the
/// secret fields (which `ProviderConfig` does not actually carry — secrets are
/// read from the environment by the HTTP provider — but this is the structural
/// guarantee the diagnostic bundle relies on).
pub fn provider_config_safe(cfg: &ProviderConfig) -> ProviderConfigSafe {
    provider_config_safe_with_models(cfg, None, None)
}

pub fn provider_config_safe_with_models(
    cfg: &ProviderConfig,
    director_model: Option<String>,
    writer_model: Option<String>,
) -> ProviderConfigSafe {
    ProviderConfigSafe {
        base_url: cfg.base_url.clone(),
        model: cfg.model.clone(),
        director_model,
        writer_model,
        structured_output: match cfg.structured_output {
            StructuredOutputMode::Auto => "auto",
            StructuredOutputMode::On => "on",
            StructuredOutputMode::Off => "off",
        }
        .to_string(),
        request_timeout_ms: cfg.request_timeout_ms,
        connect_timeout_ms: cfg.connect_timeout_ms,
        max_retries: cfg.max_retries,
        is_loopback: is_loopback_base_url(&cfg.base_url),
    }
}

/// Assemble a [`DiagnosticBundle`] from already-resolved state. `os` and
/// `rust_version` are filled from `std::env::consts` and the recorded rustc
/// version respectively; `generated_at` is RFC-3339 UTC. None of the inputs
/// reach the bundle as raw secrets — `provider` is rendered through
/// [`provider_config_safe`] if it is the `Config` variant.
///
/// `known_secrets` is stashed into the bundle's redacted view via
/// [`serialize_redacted`]; it is not stored on the bundle (the bundle never
/// holds secrets).
#[allow(clippy::needless_pass_by_value)]
pub fn build_diagnostic_bundle(
    provider: &ProviderInput<'_>,
    health: impl Into<ProviderHealthInput>,
    last_error: Option<&DomainErrorView>,
    pack_story_id: Option<&str>,
    turn: Option<u32>,
    _known_secrets: &[String],
) -> DiagnosticBundle {
    let provider_config_redacted = match provider {
        ProviderInput::Config(cfg) => provider_config_safe(cfg),
        ProviderInput::Safe(safe) => safe.clone(),
    };

    DiagnosticBundle {
        engine_version: ENGINE_VERSION.to_string(),
        rust_version: rustc_version(),
        os: std::env::consts::OS.to_string(),
        provider_health: health.into().0,
        provider_config_redacted,
        last_error: last_error.cloned(),
        pack_story_id: pack_story_id.map(str::to_string),
        turn,
        generated_at: story_model::chrono::Utc::now().to_rfc3339(),
    }
}

/// Serialize the bundle to pretty JSON, then run it through [`redact`] with
/// the default policy + the supplied `known_secrets`. The output is what gets
/// printed to the terminal / written to an attachment file.
pub fn serialize_redacted(bundle: &DiagnosticBundle, known_secrets: &[String]) -> String {
    let json = serde_json::to_string_pretty(bundle).expect("DiagnosticBundle serializes");
    let secrets: Vec<&str> = known_secrets.iter().map(String::as_str).collect();
    redact(&json, &default_policy(), &secrets)
}

/// Best-effort `rustc --version` string baked in at build time. `None` if the
/// env var was not set when this crate was compiled.
fn rustc_version() -> Option<String> {
    option_env!("RUSTC_VERSION").map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_input_config_drops_secrets() {
        let cfg = ProviderConfig::defaults();
        let safe = match ProviderInput::Config(&cfg) {
            ProviderInput::Config(c) => provider_config_safe(c),
            _ => unreachable!(),
        };
        assert!(safe.is_loopback);
        assert_eq!(safe.structured_output, "auto");
        let json = serde_json::to_string(&safe).unwrap();
        assert!(!json.to_lowercase().contains("api_key"));
    }

    #[test]
    fn structured_output_mode_renders_lowercase() {
        let mut cfg = ProviderConfig::defaults();
        cfg.structured_output = StructuredOutputMode::Off;
        let safe = provider_config_safe(&cfg);
        assert_eq!(safe.structured_output, "off");
        cfg.structured_output = StructuredOutputMode::On;
        let safe = provider_config_safe(&cfg);
        assert_eq!(safe.structured_output, "on");
    }

    #[test]
    fn non_loopback_url_flagged() {
        let mut cfg = ProviderConfig::defaults();
        cfg.base_url = "http://192.168.0.5:8080".to_string();
        let safe = provider_config_safe(&cfg);
        assert!(!safe.is_loopback);
    }
}
