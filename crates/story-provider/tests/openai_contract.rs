//! HTTP contract tests for [`OpenAiCompatibleProvider`] against a wiremock
//! server (ST-017). No real MLX process is involved — the mock server runs on
//! `127.0.0.1` so the provider sees a loopback URL exactly as in production.
//!
//! Covers the ADR-002 contract: 4xx is NOT retried, 5xx IS retried up to
//! `max_retries`, the cancellation token stops the retry loop, a non-loopback
//! base_url warns (and is recorded) without blocking, and a malformed 200 body
//! is fed to the recovery parser and surfaces `InvalidGeneration`.

use std::time::Duration;

use story_model::{ChatMessage, ProviderError, ProviderHealth, TurnRequest};
use story_provider::{
    LlmProvider, OpenAiCompatibleProvider, ProviderConfig, StructuredOutputMode,
    is_loopback_base_url,
};
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A valid `GeneratedTurn` JSON body (recovery-parser input), mirroring the
/// W5.3 fixture shape.
const VALID_TURN: &str = r#"{
  "narration": "비가 그쳤다.",
  "choices": [
    {"id":"c1","label":"a","intent":"i","tone":null,"target":null,"risk":"safe","hidden_requirements":[],"preview":null}
  ],
  "state_patch": {
    "affection_delta": {},"trust_delta": {},"tension_delta": {},"relationship_transitions": {},
    "set_flags": {},"number_delta": {},"set_strings": {},"inventory_delta": {},"add_events": [],
    "next_location": null,"next_time_slot": null,"proposed_route": null
  },
  "summary": {
    "facts": [],"unresolved": [],"relationship_changes": [],"knowledge_changes": [],
    "interpretation": null,"address_form_changes": [],"important_player_actions": [],
    "source_history_ids": []
  }
}"#;

/// Wrap an assistant `content` string in an OpenAI-shaped chat-completion
/// envelope. Extra top-level keys (`id`, `model`, ...) verify the response is
/// serde-tolerant.
fn envelope(content: &str) -> serde_json::Value {
    serde_json::json!({
        "id": "chatcmpl-x",
        "object": "chat.completion",
        "model": "test",
        "choices": [
            {"index": 0, "message": {"role": "assistant", "content": content}, "finish_reason": "stop"}
        ],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
    })
}

fn turn_request() -> TurnRequest {
    TurnRequest {
        generation_id: story_model::uuid::Uuid::nil(),
        messages: vec![ChatMessage {
            role: "system".to_string(),
            content: "test".to_string(),
        }],
        model: "test-model".to_string(),
        temperature: 0.85,
        top_p: 0.92,
        top_k: 40,
        max_tokens: 1800,
        json_schema: None,
    }
}

/// Provider pointing at the mock server, loopback, structured output OFF (so
/// `response_format` is absent and the Auto-fallback never fires by default).
fn cfg_for(server: &MockServer) -> ProviderConfig {
    let mut c = ProviderConfig::defaults();
    c.base_url = server.uri();
    c.model = "test-model".to_string();
    c.request_timeout_ms = 5_000;
    c.connect_timeout_ms = 2_000;
    c.max_retries = 2;
    c.structured_output = StructuredOutputMode::Off;
    c
}

// ---------------------------------------------------------------------------

#[tokio::test]
async fn happy_path_returns_parsed_draft_with_usage() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(envelope(VALID_TURN)))
        .mount(&server)
        .await;

    let provider = OpenAiCompatibleProvider::new(cfg_for(&server)).expect("provider");
    let draft = provider
        .generate_turn(turn_request(), CancellationToken::new())
        .await
        .expect("ok");

    // The recovery parser ran on the assistant content (not reimplemented here).
    assert_eq!(draft.narration, "비가 그쳤다.");
    assert_eq!(draft.choices.len(), 1);
    // Usage flowed through from the envelope.
    let usage = draft.usage.expect("usage");
    assert_eq!(usage.prompt_tokens, 10);
    assert_eq!(usage.completion_tokens, 5);
    assert_eq!(usage.total_tokens, 15);
}

#[tokio::test]
async fn generation_uses_sampling_values_from_provider_config() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(envelope(VALID_TURN)))
        .mount(&server)
        .await;

    let mut config = cfg_for(&server);
    config.temperature = 0.2;
    config.top_p = 0.88;
    config.top_k = 0;
    config.max_tokens = 900;
    let provider = OpenAiCompatibleProvider::new(config).expect("provider");
    provider
        .generate_turn(turn_request(), CancellationToken::new())
        .await
        .expect("valid response");

    let requests = server.received_requests().await.expect("recorded request");
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("JSON body");
    assert_eq!(body["temperature"], 0.2);
    assert_eq!(body["top_p"], 0.88);
    assert_eq!(body["max_tokens"], 900);
    assert!(body.get("top_k").is_none());
}

#[tokio::test]
async fn generation_honors_request_model_and_smaller_output_cap() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(envelope(VALID_TURN)))
        .mount(&server)
        .await;

    let mut config = cfg_for(&server);
    config.model = "writer-27b".to_string();
    config.max_tokens = 8_192;
    let provider = OpenAiCompatibleProvider::new(config).expect("provider");
    let mut request = turn_request();
    request.model = "choice-4b".to_string();
    request.max_tokens = 700;
    provider
        .generate_turn(request, CancellationToken::new())
        .await
        .expect("valid response");

    let requests = server.received_requests().await.expect("recorded request");
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("JSON body");
    assert_eq!(body["model"], "choice-4b");
    assert_eq!(body["max_tokens"], 700);
}

#[tokio::test]
async fn short_valid_scene_gets_one_best_effort_expansion_without_becoming_an_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        // Return the same short fixture twice: the provider must retain the
        // original valid scene rather than turn a quality target into failure.
        .respond_with(ResponseTemplate::new(200).set_body_json(envelope(VALID_TURN)))
        .mount(&server)
        .await;

    let mut config = cfg_for(&server);
    config.min_narration_chars = 100;
    let provider = OpenAiCompatibleProvider::new(config).expect("provider");
    let draft = provider
        .generate_turn(turn_request(), CancellationToken::new())
        .await
        .expect("a short but valid scene remains usable");
    assert_eq!(draft.narration, "비가 그쳤다.");

    let requests = server.received_requests().await.expect("recorded requests");
    assert_eq!(requests.len(), 2, "one quality retry is allowed");
    let retry_body: serde_json::Value =
        serde_json::from_slice(&requests[1].body).expect("JSON retry body");
    assert!(
        retry_body["messages"]
            .as_array()
            .expect("messages array")
            .iter()
            .any(|message| message["content"]
                .as_str()
                .is_some_and(|text| text.contains("최소 100자"))),
        "the retry must explicitly request the configured target length"
    );
}

// --- model-list discovery --------------------------------------------------

#[tokio::test]
async fn model_list_uses_auth_and_returns_stable_nonempty_ids() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("authorization", "Bearer local-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "object": "list",
            "data": [
                {"id": " qwen-32b "},
                {"id": ""},
                {"id": "qwen-7b"},
                {"id": "qwen-32b"},
                {"id": "   "}
            ]
        })))
        .mount(&server)
        .await;

    let mut config = cfg_for(&server);
    config.api_key = Some("local-secret".to_string());
    let provider = OpenAiCompatibleProvider::new(config).expect("provider");

    assert_eq!(
        provider.list_models().await.expect("model list"),
        vec!["qwen-32b", "qwen-7b"]
    );
}

#[tokio::test]
async fn model_list_surfaces_http_and_decode_errors() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let provider = OpenAiCompatibleProvider::new(cfg_for(&server)).expect("provider");
    let error = provider.list_models().await.expect_err("401 must fail");
    assert!(matches!(error, ProviderError::Protocol(message) if message == "HTTP 401"));

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": {}})))
        .mount(&server)
        .await;
    let provider = OpenAiCompatibleProvider::new(cfg_for(&server)).expect("provider");
    let error = provider
        .list_models()
        .await
        .expect_err("invalid envelope must fail");
    assert!(
        matches!(error, ProviderError::Protocol(message) if message.starts_with("model-list response decode failed:"))
    );
}

// --- health mapping --------------------------------------------------------

#[tokio::test]
async fn health_ready_on_200() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let provider = OpenAiCompatibleProvider::new(cfg_for(&server)).expect("provider");
    assert_eq!(
        provider.health().await.expect("health"),
        ProviderHealth::Ready
    );
}

#[tokio::test]
async fn health_degraded_on_500() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let provider = OpenAiCompatibleProvider::new(cfg_for(&server)).expect("provider");
    assert_eq!(
        provider.health().await.expect("health"),
        ProviderHealth::Degraded
    );
}

#[tokio::test]
async fn health_unavailable_when_server_down() {
    // Claim a free port, then release it so connection attempts are refused.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);

    let mut c = ProviderConfig::defaults();
    c.base_url = format!("http://127.0.0.1:{port}");
    c.connect_timeout_ms = 500;
    c.request_timeout_ms = 1_000;
    let provider = OpenAiCompatibleProvider::new(c).expect("provider");
    assert_eq!(
        provider.health().await.expect("health"),
        ProviderHealth::Unavailable
    );
}

// --- retry / cancel contract (ADR-002) -------------------------------------

#[tokio::test]
async fn client_error_4xx_not_retried() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(400))
        .mount(&server)
        .await;

    let provider = OpenAiCompatibleProvider::new(cfg_for(&server)).expect("provider");
    let err = provider
        .generate_turn(turn_request(), CancellationToken::new())
        .await
        .expect_err("4xx should error");

    assert!(
        matches!(err, ProviderError::Protocol(ref msg) if msg.contains("400")),
        "expected Protocol(400), got {err:?}"
    );
    let n = server.received_requests().await.expect("recorded").len();
    assert_eq!(
        n, 1,
        "4xx MUST NOT be retried — expected exactly 1 request, got {n}"
    );
}

#[tokio::test]
async fn server_error_5xx_then_200_retried_once() {
    let server = MockServer::start().await;
    // The 500 mock has the higher priority (1) and a 1-call budget: request 1
    // hits 500 (then the mock is exhausted), request 2 falls through to the 200
    // mock at default priority (5).
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(envelope(VALID_TURN)))
        .mount(&server)
        .await;

    let provider = OpenAiCompatibleProvider::new(cfg_for(&server)).expect("provider");
    let draft = provider
        .generate_turn(turn_request(), CancellationToken::new())
        .await
        .expect("retried then succeeded");
    assert_eq!(draft.narration, "비가 그쳤다.");

    let n = server.received_requests().await.expect("recorded").len();
    assert_eq!(
        n, 2,
        "5xx MUST be retried once — expected exactly 2 requests, got {n}"
    );
}

#[tokio::test]
async fn malformed_200_envelope_is_retried_then_valid_response_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{truncated"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(envelope(VALID_TURN)))
        .mount(&server)
        .await;

    let provider = OpenAiCompatibleProvider::new(cfg_for(&server)).expect("provider");
    let draft = provider
        .generate_turn(turn_request(), CancellationToken::new())
        .await
        .expect("a transient malformed envelope must recover");
    assert_eq!(draft.narration, "비가 그쳤다.");
    assert_eq!(
        server.received_requests().await.expect("recorded").len(),
        2,
        "a malformed HTTP 200 envelope receives one retry"
    );
}

#[tokio::test]
async fn cancel_honored_no_retry_storm() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(envelope(VALID_TURN))
                .set_delay(Duration::from_secs(30)),
        )
        .mount(&server)
        .await;

    let provider = OpenAiCompatibleProvider::new(cfg_for(&server)).expect("provider");
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    let handle =
        tokio::spawn(async move { provider.generate_turn(turn_request(), cancel_clone).await });

    // Let the request reach the server, then cancel mid-flight.
    tokio::time::sleep(Duration::from_millis(100)).await;
    cancel.cancel();

    let result = tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("test hung")
        .expect("join");
    assert!(
        matches!(result, Err(ProviderError::Cancelled)),
        "expected Cancelled, got {result:?}"
    );
    let n = server.received_requests().await.expect("recorded").len();
    assert!(
        n <= 1,
        "cancel MUST stop the retry loop — expected at most 1 request, got {n}"
    );
}

#[tokio::test]
async fn malformed_body_returns_invalid_generation() {
    let server = MockServer::start().await;
    // Structured output OFF + no schema => no Auto-fallback; single attempt.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(envelope("not a json object at all")),
        )
        .mount(&server)
        .await;

    let provider = OpenAiCompatibleProvider::new(cfg_for(&server)).expect("provider");
    let err = provider
        .generate_turn(turn_request(), CancellationToken::new())
        .await
        .expect_err("should fail");
    assert!(
        matches!(err, ProviderError::InvalidGeneration(_)),
        "expected InvalidGeneration, got {err:?}"
    );
    let n = server.received_requests().await.expect("recorded").len();
    assert_eq!(
        n, 1,
        "no-rf path must not trigger the Auto-fallback retry — got {n}"
    );
}

#[tokio::test]
async fn malformed_http_envelope_stops_before_assistant_content_parsing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        // This is not an OpenAI response envelope at all. It reproduces the
        // same boundary as the live `response decode failed` log entries.
        .respond_with(ResponseTemplate::new(200).set_body_string("{not-json"))
        .mount(&server)
        .await;

    let provider = OpenAiCompatibleProvider::new(cfg_for(&server)).expect("provider");
    let err = provider
        .generate_turn(turn_request(), CancellationToken::new())
        .await
        .expect_err("invalid envelope must fail before assistant content parsing");
    assert!(
        matches!(err, ProviderError::Protocol(ref message) if message.starts_with("response decode failed:")),
        "expected envelope decode error, got {err:?}"
    );
}

#[tokio::test]
async fn auto_fallback_retries_without_response_format_on_parse_failure() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(envelope("totally unparseable content")),
        )
        .mount(&server)
        .await;

    let mut c = cfg_for(&server);
    c.structured_output = StructuredOutputMode::Auto;
    let provider = OpenAiCompatibleProvider::new(c).expect("provider");
    let mut req = turn_request();
    req.json_schema = Some(serde_json::json!({"type": "object"}));

    let err = provider
        .generate_turn(req, CancellationToken::new())
        .await
        .expect_err("both attempts should fail to parse");
    assert!(
        matches!(err, ProviderError::InvalidGeneration(_)),
        "expected InvalidGeneration, got {err:?}"
    );
    let n = server.received_requests().await.expect("recorded").len();
    assert_eq!(
        n, 2,
        "Auto MUST retry once without response_format — expected 2 requests, got {n}"
    );
    assert!(
        !provider.capabilities().supports_structured_output,
        "capability MUST be disabled for the session after the fallback"
    );
}

#[tokio::test]
async fn auto_retry_keeps_response_format_for_contract_failure() {
    let server = MockServer::start().await;
    // An unknown `risk` value is valid JSON but violates a game-contract enum.
    // It must receive one lower-temperature retry while constrained decoding
    // remains enabled.
    let contract_invalid = VALID_TURN.replace("\"risk\":\"safe\"", "\"risk\":\"unknown\"");
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(envelope(&contract_invalid)))
        .mount(&server)
        .await;

    let mut c = cfg_for(&server);
    c.structured_output = StructuredOutputMode::Auto;
    let provider = OpenAiCompatibleProvider::new(c).expect("provider");
    let mut req = turn_request();
    req.json_schema = Some(serde_json::json!({"type": "object"}));

    let _ = provider
        .generate_turn(req, CancellationToken::new())
        .await
        .expect_err("both contract-invalid replies must fail");
    let requests = server.received_requests().await.expect("recorded");
    assert_eq!(requests.len(), 2, "one bounded contract retry is expected");
    for request in &requests {
        let body: serde_json::Value = serde_json::from_slice(&request.body).expect("JSON request");
        assert!(
            body.get("response_format").is_some(),
            "contract retry must retain constrained decoding"
        );
    }
    assert!(
        provider.capabilities().supports_structured_output,
        "a contract error does not prove the server lacks structured-output support"
    );
}

// --- non-loopback warning (ADR-002) ----------------------------------------

#[test]
fn non_loopback_predicate_and_flag() {
    // The decision predicate driving the tracing::warn!.
    assert!(is_loopback_base_url("http://127.0.0.1:8080"));
    assert!(is_loopback_base_url("http://localhost:8080"));
    assert!(is_loopback_base_url("http://127.1.2.3:80"));
    assert!(!is_loopback_base_url("http://192.168.1.5:8080"));
    assert!(!is_loopback_base_url("http://10.0.0.1:8080"));
    assert!(!is_loopback_base_url("http://example.com"));

    // A non-loopback provider still constructs (warning, NOT block) and records
    // its non-loopback status for UI surfacing.
    let mut non_loopback = ProviderConfig::defaults();
    non_loopback.base_url = "http://192.168.1.5:8080".to_string();
    let provider = OpenAiCompatibleProvider::new(non_loopback).expect("constructs");
    assert!(!provider.is_loopback(), "non-loopback must be recorded");

    // The loopback default records loopback = true.
    let loopback = OpenAiCompatibleProvider::new(ProviderConfig::defaults()).expect("constructs");
    assert!(loopback.is_loopback(), "loopback default must be recorded");
}

#[test]
fn chat_and_health_urls_join_correctly() {
    let provider = OpenAiCompatibleProvider::new(ProviderConfig::defaults()).expect("provider");
    assert_eq!(
        provider.chat_url(),
        "http://127.0.0.1:8080/v1/chat/completions"
    );
    assert_eq!(provider.health_url(), "http://127.0.0.1:8080/health");
    assert_eq!(provider.models_url(), "http://127.0.0.1:8080/v1/models");
}
