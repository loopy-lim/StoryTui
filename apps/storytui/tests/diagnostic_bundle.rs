//! Redacted-diagnostic-bundle test (ST-035).
//!
//! Asserts that a bundle assembled with a fake API key present in the
//! environment (and an error detail that echoes it) serializes with ZERO
//! secret leakage through `serialize_redacted`.

use story_model::{DiagnosticBundle, DomainErrorView, ProviderConfigSafe, ProviderHealth};
use story_provider::ProviderConfig;
use storytui::diagnostic::{
    ProviderHealthInput, ProviderInput, build_diagnostic_bundle, collect_known_secrets,
    serialize_redacted,
};

const FAKE_API_KEY: &str = "sk-FAKE-BUNDLE-NOLEAK-9876543210abcdef";

fn sample_provider_config() -> ProviderConfig {
    ProviderConfig::defaults()
}

fn sample_bundle() -> DiagnosticBundle {
    let cfg = sample_provider_config();
    // Plant the api key in the environment so `collect_known_secrets` picks
    // it up.
    // SAFETY: tests in this file are single-threaded; no other code reads
    // these env vars concurrently while we mutate them.
    unsafe {
        std::env::set_var("OPENAI_API_KEY", FAKE_API_KEY);
        std::env::set_var("STORYTUI_API_KEY", FAKE_API_KEY);
    }

    // Plant the api key inside a real provider-style error detail (simulating
    // an error string that accidentally echoed the secret).
    let last_error = DomainErrorView {
        code: "provider_protocol".to_string(),
        title_ko: "공급자 오류".to_string(),
        message_ko: format!("연결 실패: bearer {FAKE_API_KEY}"),
        recoverable: true,
    };

    build_diagnostic_bundle(
        &ProviderInput::Config(&cfg),
        ProviderHealthInput(Some(ProviderHealth::Degraded)),
        Some(&last_error),
        Some("rainy-reunion"),
        Some(7),
        &collect_known_secrets(),
    )
}

#[test]
fn bundle_provider_config_safe_drops_secret_fields() {
    let b = sample_bundle();
    let safe_json = serde_json::to_string(&b.provider_config_redacted).unwrap();
    assert!(
        !safe_json.to_lowercase().contains("api_key"),
        "ProviderConfigSafe serialized api_key: {safe_json}"
    );
}

#[test]
fn redacted_serialization_omits_api_key() {
    let b = sample_bundle();
    let redacted = serialize_redacted(&b, &collect_known_secrets());
    assert!(
        !redacted.contains(FAKE_API_KEY),
        "fake api key leaked into redacted bundle:\n{redacted}"
    );
    // Even when the api key was planted in last_error.message_ko, the redact
    // pass catches it.
    assert!(
        redacted.contains("***"),
        "expected mask token in redacted bundle:\n{redacted}"
    );
}

#[test]
fn build_diagnostic_bundle_marks_loopback() {
    let cfg = sample_provider_config(); // 127.0.0.1 — loopback
    let b = build_diagnostic_bundle(
        &ProviderInput::Config(&cfg),
        ProviderHealthInput(None),
        None,
        None,
        None,
        &[],
    );
    assert!(b.provider_config_redacted.is_loopback);
    assert_eq!(b.provider_config_redacted.model, "");
    assert_eq!(b.provider_config_redacted.structured_output, "auto");
}

#[test]
fn build_diagnostic_bundle_from_safe_value() {
    // The host can also pass an already-built ProviderConfigSafe (e.g. when
    // loading from a saved profile). The bundle copies it verbatim.
    let safe = ProviderConfigSafe {
        base_url: "http://192.168.0.5:8080".to_string(),
        model: "mlx-community/x".to_string(),
        structured_output: "off".to_string(),
        request_timeout_ms: 60_000,
        connect_timeout_ms: 3_000,
        max_retries: 1,
        is_loopback: false,
    };
    let b = build_diagnostic_bundle(
        &ProviderInput::Safe(safe.clone()),
        ProviderHealthInput(None),
        None,
        None,
        None,
        &[],
    );
    assert_eq!(b.provider_config_redacted, safe);
}

#[test]
fn collect_known_secrets_reads_env() {
    // SAFETY: single-threaded test.
    unsafe {
        std::env::set_var("OPENAI_API_KEY", FAKE_API_KEY);
    }
    let secrets = collect_known_secrets();
    assert!(
        secrets.iter().any(|s| s == FAKE_API_KEY),
        "OPENAI_API_KEY not collected: {secrets:?}"
    );
}

#[test]
fn redacted_serialization_preserves_non_sensitive_fields() {
    let b = sample_bundle();
    let redacted = serialize_redacted(&b, &collect_known_secrets());
    assert!(redacted.contains("rainy-reunion"));
    assert!(redacted.contains("engine_version"));
    assert!(redacted.contains("provider_health"));
}
