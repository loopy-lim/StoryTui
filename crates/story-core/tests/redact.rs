//! No-leak audit for `story_core::redact` (ST-035, Acceptance §20).
//!
//! This is the load-bearing test for "default logs/diagnostics contain NONE of
//! prompt/body/free-input/secret". Each sensitive class is represented by a
//! UNIQUE, unambiguous marker; we run `redact` with `default_policy()` and
//! assert ZERO markers survive.

use story_core::redact::{RedactionPolicy, default_policy, redact};

const API_KEY_LEAK: &str = "sk-LEAK-NOLEAK-API-KEY-1234567890abcdef";
const SYSTEM_PROMPT_LEAK: &str = "LEAK_SYSTEM_PROMPT_BODY_unique_marker_xyz";
const NARRATION_LEAK: &str = "LEAK_NARRATION_BODY_unique_marker_qrs";
const FREE_INPUT_LEAK: &str = "LEAK_FREE_INPUT_unique_marker_tuv";

fn build_diagnostic_excerpt() -> String {
    let mut s = String::new();
    s.push_str("=== StoryTUI diagnostic excerpt ===\n");
    // API key as an env-style line.
    s.push_str("api_key=");
    s.push_str(API_KEY_LEAK);
    s.push('\n');
    // HTTP-style header — Bearer + token.
    s.push_str("Authorization: Bearer ");
    s.push_str(API_KEY_LEAK);
    s.push('\n');
    // JSON-style.
    s.push_str("\"api_key\":\"");
    s.push_str(API_KEY_LEAK);
    s.push_str("\"\n");
    // System prompt as a section block.
    s.push_str("<<<system_prompt>>>\n");
    s.push_str("You are a narrator. ");
    s.push_str(SYSTEM_PROMPT_LEAK);
    s.push_str("\n<<</system_prompt>>>\n");
    // Narration as a section block.
    s.push_str("<<<narration>>>\n");
    s.push_str("유나가 말을 잇는다. ");
    s.push_str(NARRATION_LEAK);
    s.push_str("\n<<</narration>>>\n");
    // Free input as a section block.
    s.push_str("<<<free_input>>>\n");
    s.push_str("플레이어 자유 입력: ");
    s.push_str(FREE_INPUT_LEAK);
    s.push_str("\n<<</free_input>>>\n");
    s
}

/// THE load-bearing no-leak audit: four unique markers, one per sensitive
/// class (secret / system prompt / narration body / free input), none survive
/// `redact` under `default_policy()`.
#[test]
fn no_leak_audit_catches_all_four_marker_classes() {
    let text = build_diagnostic_excerpt();
    let redacted = redact(&text, &default_policy(), &[API_KEY_LEAK]);

    // Sanity: each marker is present in the raw text.
    assert!(text.contains(API_KEY_LEAK), "raw text must contain api key");
    assert!(
        text.contains(SYSTEM_PROMPT_LEAK),
        "raw text must contain system-prompt marker"
    );
    assert!(
        text.contains(NARRATION_LEAK),
        "raw text must contain narration marker"
    );
    assert!(
        text.contains(FREE_INPUT_LEAK),
        "raw text must contain free-input marker"
    );

    // After redaction, NONE of the unique markers may survive.
    assert!(
        !redacted.contains(API_KEY_LEAK),
        "API key leaked into redacted output:\n{redacted}"
    );
    assert!(
        !redacted.contains(SYSTEM_PROMPT_LEAK),
        "system prompt body leaked into redacted output:\n{redacted}"
    );
    assert!(
        !redacted.contains(NARRATION_LEAK),
        "narration body leaked into redacted output:\n{redacted}"
    );
    assert!(
        !redacted.contains(FREE_INPUT_LEAK),
        "free input leaked into redacted output:\n{redacted}"
    );
}

/// The redacted text must still be human-readable — markers are replaced with
/// fixed placeholders, not silently elided.
#[test]
fn redact_leaves_mask_placeholders() {
    let text = format!("api_key={API_KEY_LEAK}");
    let redacted = redact(&text, &default_policy(), &[API_KEY_LEAK]);
    assert_eq!(redacted, "api_key=***");
}

#[test]
fn redact_masks_bearer_header() {
    let text = format!("Authorization: Bearer {API_KEY_LEAK}");
    let redacted = redact(&text, &default_policy(), &[API_KEY_LEAK]);
    assert!(
        !redacted.contains(API_KEY_LEAK),
        "bearer token leaked: {redacted}"
    );
    // Both "authorization" and "bearer" markers fire.
    assert!(redacted.contains("***"), "expected mask token: {redacted}");
}

#[test]
fn redact_masks_json_quoted_value() {
    let text = format!("{{\"api_key\":\"{API_KEY_LEAK}\"}}");
    let redacted = redact(&text, &default_policy(), &[API_KEY_LEAK]);
    assert!(
        !redacted.contains(API_KEY_LEAK),
        "json api_key leaked: {redacted}"
    );
}

#[test]
fn redact_strips_section_content_but_keeps_tags() {
    let text = format!("<<<narration>>>\n{NARRATION_LEAK}\n<<</narration>>>");
    let redacted = redact(&text, &default_policy(), &[]);
    assert!(
        !redacted.contains(NARRATION_LEAK),
        "narration leaked: {redacted}"
    );
    // The section delimiters remain so a reader knows content was elided.
    assert!(
        redacted.contains("<<<narration>>>") && redacted.contains("<<</narration>>>"),
        "section delimiters lost: {redacted}"
    );
}

#[test]
fn redact_handles_unclosed_section_safely() {
    // No closing tag — everything after the open tag is elided to be safe.
    let text = format!("<<<free_input>>>{FREE_INPUT_LEAK} more text no close");
    let redacted = redact(&text, &default_policy(), &[]);
    assert!(
        !redacted.contains(FREE_INPUT_LEAK),
        "unclosed section leaked: {redacted}"
    );
}

#[test]
fn redact_is_case_insensitive_on_markers() {
    let text = format!("API_KEY={API_KEY_LEAK}; ApiKey: {API_KEY_LEAK}");
    let redacted = redact(&text, &default_policy(), &[API_KEY_LEAK]);
    assert!(
        !redacted.contains(API_KEY_LEAK),
        "case-insensitive marker leak: {redacted}"
    );
}

#[test]
fn redact_preserves_unrelated_text() {
    let text = "engine_version=0.1.0\nos=macos\nturn=42";
    let redacted = redact(text, &default_policy(), &[]);
    assert_eq!(redacted, text, "non-sensitive text should pass through");
}

#[test]
fn redact_with_empty_policy_only_masks_known_secrets() {
    let policy = RedactionPolicy {
        secret_markers: vec![],
        strip_sections: vec![],
    };
    let text = format!("api_key={API_KEY_LEAK}");
    let redacted = redact(&text, &policy, &[API_KEY_LEAK]);
    assert_eq!(redacted, "api_key=***");
    // Without the secret list, an empty policy does not catch the key.
    let unredacted = redact(&text, &policy, &[]);
    assert!(unredacted.contains(API_KEY_LEAK));
}

#[test]
fn default_policy_lists_expected_secret_markers() {
    let p = default_policy();
    for marker in [
        "api_key",
        "authorization",
        "bearer",
        "token",
        "secret",
        "password",
    ] {
        let lowered = marker.to_string();
        assert!(
            p.secret_markers.iter().any(|m| m == &lowered),
            "default policy missing secret marker {marker}"
        );
    }
    for section in ["system_prompt", "narration", "free_input"] {
        assert!(
            p.strip_sections.contains(&section),
            "default policy missing strip section {section}"
        );
    }
}
