//! Provider TOML config (ST-017, Technical Spec §10.2).
//!
//! `[provider]` block deserialized into [`ProviderConfig`]. Defaults mirror the
//! spec's local-MLX example exactly: loopback `127.0.0.1:8080`, temperature
//! 0.85, top_p 0.92, top_k 40, max_tokens 1800, request/connect timeouts
//! 180000/3000 ms, 2 retries, 24576-token context budget, structured output
//! auto. The operator is expected to set `model`; the default is empty.

use serde::{Deserialize, Serialize};

/// Whether to advertise structured-output (`response_format` JSON Schema)
/// support to the OpenAI-compatible server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StructuredOutputMode {
    /// Optimistically send `response_format`; if the server ignores it and the
    /// body fails to parse, disable for the session and retry once without it.
    Auto,
    /// Always send `response_format`.
    On,
    /// Never send `response_format` (free-form JSON with a field contract).
    Off,
}

/// Configuration for [`crate::openai_compatible::OpenAiCompatibleProvider`].
///
/// All fields carry `#[serde(default = ...)]` so a partial `[provider]` table
/// fills in the spec defaults.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Adapter kind (always `"openai_compatible"` for this provider).
    #[serde(default = "default_kind")]
    pub kind: String,
    /// Stable provider id surfaced in logs.
    #[serde(default = "default_name")]
    pub name: String,
    /// Base URL of the OpenAI-compatible server (loopback by default).
    #[serde(default = "default_base_url")]
    pub base_url: String,
    /// Chat-completions path appended to `base_url`.
    #[serde(default = "default_chat_path")]
    pub chat_path: String,
    /// Health-check path appended to `base_url`.
    #[serde(default = "default_health_path")]
    pub health_path: String,
    /// Model id to target. Defaults to empty — the operator must set this.
    #[serde(default)]
    pub model: String,
    /// Optional environment variable whose value is sent as a Bearer token.
    /// The secret itself is never serialized or stored in diagnostics.
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Optional file containing the Bearer token. The loaded secret is kept
    /// only in memory and is skipped by serde/diagnostic projections.
    #[serde(default)]
    pub api_key_file: Option<String>,
    #[serde(skip)]
    pub api_key: Option<String>,
    /// Sampling temperature.
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    /// Nucleus sampling probability mass.
    #[serde(default = "default_top_p")]
    pub top_p: f32,
    /// Top-k sampling cutoff (0 = disabled / server default).
    #[serde(default = "default_top_k")]
    pub top_k: u32,
    /// Maximum output tokens per turn.
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// Soft lower target for narration length. `0` disables the optional
    /// quality retry; a short but valid response is never rejected solely for
    /// this setting.
    #[serde(default)]
    pub min_narration_chars: usize,
    /// Per-request deadline in milliseconds.
    #[serde(default = "default_request_timeout_ms")]
    pub request_timeout_ms: u64,
    /// TCP connect deadline in milliseconds.
    #[serde(default = "default_connect_timeout_ms")]
    pub connect_timeout_ms: u64,
    /// Maximum retry attempts on transient failures (5xx / connect / timeout).
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Soft context budget in tokens for prompt construction.
    #[serde(default = "default_context_budget_tokens")]
    pub context_budget_tokens: u32,
    /// Structured-output mode (see [`StructuredOutputMode`]).
    #[serde(default = "default_structured_output")]
    pub structured_output: StructuredOutputMode,
}

impl ProviderConfig {
    /// The Technical Spec §10.2 local-MLX example. `model` is left empty — the
    /// operator must populate it from their own `[provider]` table.
    pub fn defaults() -> Self {
        Self {
            kind: default_kind(),
            name: default_name(),
            base_url: default_base_url(),
            chat_path: default_chat_path(),
            health_path: default_health_path(),
            model: String::new(),
            api_key_env: None,
            api_key_file: None,
            api_key: None,
            temperature: default_temperature(),
            top_p: default_top_p(),
            top_k: default_top_k(),
            max_tokens: default_max_tokens(),
            min_narration_chars: 0,
            request_timeout_ms: default_request_timeout_ms(),
            connect_timeout_ms: default_connect_timeout_ms(),
            max_retries: default_max_retries(),
            context_budget_tokens: default_context_budget_tokens(),
            structured_output: default_structured_output(),
        }
    }
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self::defaults()
    }
}

// --- serde default functions (mirror `defaults()` exactly) ---

fn default_kind() -> String {
    "openai_compatible".to_string()
}
fn default_name() -> String {
    "local-mlx-vlm".to_string()
}
fn default_base_url() -> String {
    "http://127.0.0.1:8080".to_string()
}
fn default_chat_path() -> String {
    "/v1/chat/completions".to_string()
}
fn default_health_path() -> String {
    "/health".to_string()
}
fn default_temperature() -> f32 {
    0.85
}
fn default_top_p() -> f32 {
    0.92
}
fn default_top_k() -> u32 {
    40
}
fn default_max_tokens() -> u32 {
    1800
}
fn default_request_timeout_ms() -> u64 {
    180_000
}
fn default_connect_timeout_ms() -> u64 {
    3_000
}
fn default_max_retries() -> u32 {
    2
}
fn default_context_budget_tokens() -> u32 {
    24_576
}
fn default_structured_output() -> StructuredOutputMode {
    StructuredOutputMode::Auto
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec_section_10_2() {
        let c = ProviderConfig::defaults();
        assert_eq!(c.kind, "openai_compatible");
        assert_eq!(c.name, "local-mlx-vlm");
        assert_eq!(c.base_url, "http://127.0.0.1:8080");
        assert_eq!(c.chat_path, "/v1/chat/completions");
        assert_eq!(c.health_path, "/health");
        assert_eq!(c.temperature, 0.85);
        assert_eq!(c.top_p, 0.92);
        assert_eq!(c.top_k, 40);
        assert_eq!(c.max_tokens, 1800);
        assert_eq!(c.request_timeout_ms, 180_000);
        assert_eq!(c.connect_timeout_ms, 3_000);
        assert_eq!(c.max_retries, 2);
        assert_eq!(c.context_budget_tokens, 24_576);
        assert_eq!(c.structured_output, StructuredOutputMode::Auto);
        // 127.0.0.1 = loopback (ADR-002).
        assert!(crate::openai_compatible::is_loopback_base_url(&c.base_url));
    }

    #[test]
    fn round_trips_through_toml() {
        let mut c = ProviderConfig::defaults();
        c.model = "mlx-community/test".to_string();
        let toml = toml::to_string(&c).expect("serialize");
        let back: ProviderConfig = toml::from_str(&toml).expect("deserialize");
        assert_eq!(c, back);
    }

    #[test]
    fn partial_provider_table_fills_defaults() {
        let toml_str = r#"
[provider]
base_url = "http://127.0.0.1:9090"
model = "mlx-community/x"
"#;
        #[derive(Deserialize)]
        struct Cfg {
            provider: ProviderConfig,
        }
        let cfg: Cfg = toml::from_str(toml_str).expect("parse");
        // safe fields honored.
        assert_eq!(cfg.provider.base_url, "http://127.0.0.1:9090");
        assert_eq!(cfg.provider.model, "mlx-community/x");
        // Defaults fill the gaps.
        assert_eq!(cfg.provider.max_retries, 2);
        assert_eq!(cfg.provider.top_k, 40);
        assert_eq!(cfg.provider.structured_output, StructuredOutputMode::Auto);
    }

    #[test]
    fn structured_output_mode_serde_is_lowercase() {
        for (mode, s) in [
            (StructuredOutputMode::Auto, "auto"),
            (StructuredOutputMode::On, "on"),
            (StructuredOutputMode::Off, "off"),
        ] {
            let ser = serde_json::to_string(&mode).expect("serialize");
            assert_eq!(ser, format!("\"{s}\""));
            let de: StructuredOutputMode = serde_json::from_str(&ser).expect("deserialize");
            assert_eq!(de, mode);
        }
    }
}
