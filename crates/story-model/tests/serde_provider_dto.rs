//! Serde contract tests for the W5 provider cross-boundary DTOs
//! (`ChatMessage`, `TurnRequest`, `GeneratedTurnDraft`, `ProviderHealth`,
//! `ProviderCapabilities`, `ProviderError`).
//!
//! These types cross the `story-provider` <-> `story-core` boundary as pure
//! data, so their serde shape must be stable and round-trip cleanly.

use story_model::{
    ChatMessage, GeneratedTurnDraft, ProviderCapabilities, ProviderError, ProviderHealth,
    TurnRequest, TurnSummary,
};
use story_model::{Choice, RiskLevel, StatePatch, TokenUsage};

use uuid::Uuid;

fn sample_choice(id: &str) -> Choice {
    Choice {
        id: id.to_string(),
        label: format!("label-{id}"),
        intent: format!("intent-{id}"),
        tone: None,
        target: None,
        risk: RiskLevel::Safe,
        hidden_requirements: vec![],
        preview: None,
    }
}

fn sample_summary() -> TurnSummary {
    TurnSummary {
        facts: vec!["fact-a".to_string()],
        unresolved: vec!["thread-b".to_string()],
        relationship_changes: vec![],
        knowledge_changes: vec![],
        interpretation: None,
        address_form_changes: vec![],
        important_player_actions: vec![],
        source_history_ids: vec![],
    }
}

fn sample_draft() -> GeneratedTurnDraft {
    GeneratedTurnDraft {
        generation_id: Uuid::nil(),
        narration: "카페 창에 비가 흘렀다.".to_string(),
        choices: vec![sample_choice("c1"), sample_choice("c2")],
        proposed_patch: StatePatch::default(),
        summary: sample_summary(),
        usage: Some(TokenUsage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
        }),
    }
}

// ---------------------------------------------------------------------------
// ChatMessage / TurnRequest
// ---------------------------------------------------------------------------

#[test]
fn chat_message_round_trip() {
    let m = ChatMessage {
        role: "system".to_string(),
        content: "you are a narrator".to_string(),
    };
    let json = serde_json::to_string(&m).unwrap();
    let back: ChatMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(back, m);
    assert_eq!(json, r#"{"role":"system","content":"you are a narrator"}"#);
}

#[test]
fn turn_request_round_trip_with_schema() {
    let req = TurnRequest {
        generation_id: Uuid::nil(),
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: "s".to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: "u".to_string(),
            },
        ],
        model: "gpt-test".to_string(),
        temperature: 0.7,
        top_p: 0.9,
        top_k: 40,
        max_tokens: 1024,
        json_schema: Some(serde_json::json!({"type": "object"})),
    };
    let json = serde_json::to_string(&req).unwrap();
    let back: TurnRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(back, req);
    // json_schema is optional and must survive the round trip.
    assert!(json.contains(r#""json_schema":{"type":"object"}"#));
}

#[test]
fn turn_request_round_trip_without_schema() {
    let req = TurnRequest {
        generation_id: Uuid::nil(),
        messages: vec![],
        model: "m".to_string(),
        temperature: 0.0,
        top_p: 1.0,
        top_k: 0,
        max_tokens: 1,
        json_schema: None,
    };
    let json = serde_json::to_string(&req).unwrap();
    let back: TurnRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(back, req);
    assert!(json.contains(r#""json_schema":null"#));
}

// ---------------------------------------------------------------------------
// GeneratedTurnDraft
// ---------------------------------------------------------------------------

#[test]
fn generated_turn_draft_round_trip() {
    let draft = sample_draft();
    let json = serde_json::to_string(&draft).unwrap();
    let back: GeneratedTurnDraft = serde_json::from_str(&json).unwrap();
    assert_eq!(back, draft);
    // proposed_patch and summary must be present in the wire shape.
    assert!(json.contains(r#""proposed_patch""#));
    assert!(json.contains(r#""summary""#));
    assert!(json.contains(r#""narration""#));
}

#[test]
fn generated_turn_draft_usage_optional() {
    let mut draft = sample_draft();
    draft.usage = None;
    let json = serde_json::to_string(&draft).unwrap();
    assert!(json.contains(r#""usage":null"#));
    let back: GeneratedTurnDraft = serde_json::from_str(&json).unwrap();
    assert_eq!(back, draft);
    assert!(back.usage.is_none());
}

// ---------------------------------------------------------------------------
// ProviderHealth
// ---------------------------------------------------------------------------

#[test]
fn provider_health_serde_snake_case() {
    for (h, expected) in [
        (ProviderHealth::Ready, "ready"),
        (ProviderHealth::Loading, "loading"),
        (ProviderHealth::Unavailable, "unavailable"),
        (ProviderHealth::Degraded, "degraded"),
        (ProviderHealth::Misconfigured, "misconfigured"),
    ] {
        let json = serde_json::to_string(&h).unwrap();
        assert_eq!(json, format!("\"{expected}\""));
        let back: ProviderHealth = serde_json::from_str(&json).unwrap();
        assert_eq!(back, h);
    }
}

// ---------------------------------------------------------------------------
// ProviderCapabilities
// ---------------------------------------------------------------------------

#[test]
fn provider_capabilities_round_trip() {
    let caps = ProviderCapabilities {
        supports_structured_output: true,
        supports_director: false,
        supports_streaming: false,
    };
    let json = serde_json::to_string(&caps).unwrap();
    let back: ProviderCapabilities = serde_json::from_str(&json).unwrap();
    assert_eq!(back, caps);
    assert_eq!(
        json,
        r#"{"supports_structured_output":true,"supports_director":false,"supports_streaming":false}"#
    );
}

// ---------------------------------------------------------------------------
// ProviderError
// ---------------------------------------------------------------------------

#[test]
fn provider_error_round_trip_all_variants() {
    let cases = vec![
        ProviderError::Unavailable("offline".to_string()),
        ProviderError::Loading("warm".to_string()),
        ProviderError::Protocol("bad json".to_string()),
        ProviderError::Timeout { ms: 5000 },
        ProviderError::Cancelled,
        ProviderError::UnsupportedCapability("streaming".to_string()),
        ProviderError::InvalidGeneration("empty narration".to_string()),
    ];
    for err in &cases {
        let json = serde_json::to_string(err).unwrap();
        let back: ProviderError = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, err, "round-trip failed for {json}");
    }
}

#[test]
fn provider_error_display_messages_are_stable() {
    // Display strings are part of the contract surfaced in logs/diagnostics.
    assert_eq!(
        ProviderError::Unavailable("x".into()).to_string(),
        "provider unavailable: x"
    );
    assert_eq!(
        ProviderError::Timeout { ms: 42 }.to_string(),
        "timeout after 42ms"
    );
    assert_eq!(ProviderError::Cancelled.to_string(), "cancelled");
    assert_eq!(
        ProviderError::UnsupportedCapability("director".into()).to_string(),
        "unsupported capability: director"
    );
}

#[test]
fn provider_error_timeout_serializes_as_struct_with_ms() {
    let json = serde_json::to_string(&ProviderError::Timeout { ms: 7 }).unwrap();
    assert_eq!(json, r#"{"Timeout":{"ms":7}}"#);
    let back: ProviderError = serde_json::from_str(&json).unwrap();
    assert_eq!(back, ProviderError::Timeout { ms: 7 });
}

#[test]
fn provider_error_unit_variants_serialize_as_bare_tags() {
    // Cancelled is a unit variant -> bare tag string.
    let json = serde_json::to_string(&ProviderError::Cancelled).unwrap();
    assert_eq!(json, r#""Cancelled""#);
    let back: ProviderError = serde_json::from_str(&json).unwrap();
    assert_eq!(back, ProviderError::Cancelled);
}
