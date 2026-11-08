//! HTTP contract tests for `generate_turn_stream` (issue #2): the MVP
//! single-call path streamed over SSE against a wiremock server.
//!
//! Covers: narration-only chunk surfacing (JSON tokens never leak), final
//! draft through the existing strict parser, mid-stream cancellation, and the
//! non-streaming fallback when the stream fails before the first chunk.

use std::time::Duration;

use story_model::{ChatMessage, ProviderError, TurnRequest, WriterEvent};
use story_provider::{
    LlmProvider, OpenAiCompatibleProvider, ProviderConfig, StructuredOutputMode,
};
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

/// A valid `GeneratedTurn` JSON (same shape as the openai_contract fixture).
const VALID_TURN: &str = r#"{
  "narration": "비가 그쳤다. 노란 우산이 멈춰 섰다.",
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

const NARRATION: &str = "비가 그쳤다. 노란 우산이 멈춰 섰다.";

/// Build a 200 SSE response from raw bytes. NOTE: wiremock serves the body
/// in one piece, so a UTF-8 char fractured across *network* chunks cannot be
/// forced here — that boundary is covered by the sse.rs byte-split unit
/// tests plus the combined parser→extractor test in narration_stream.rs;
/// this helper only lets tests shape exact SSE event payloads.
fn sse_response(body: Vec<u8>) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
}

/// One SSE `data:` event carrying a content delta.
fn delta_event(content: &str) -> String {
    let payload = serde_json::json!({
        "choices": [{"delta": {"content": content}}]
    });
    format!("data: {payload}\n\n")
}

/// Split `text` into `n` roughly-equal pieces for delta streaming.
fn split_pieces(text: &str, n: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut pieces = Vec::new();
    let mut at = 0;
    for i in 0..n {
        let end = if i + 1 == n {
            chars.len()
        } else {
            chars.len() * (i + 1) / n
        };
        pieces.push(chars[at..end].iter().collect());
        at = end;
    }
    pieces
}

#[tokio::test]
async fn turn_stream_surfaces_only_decoded_narration_and_returns_draft() {
    // Stream the whole GeneratedTurn JSON in deltas, then usage + [DONE].
    let mut sse = String::new();
    for piece in split_pieces(VALID_TURN, 7) {
        sse.push_str(&delta_event(&piece));
    }
    sse.push_str(
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5,\"total_tokens\":15}}\n\n",
    );
    sse.push_str("data: [DONE]\n\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(sse.into_bytes()))
        .mount(&server)
        .await;

    let provider = OpenAiCompatibleProvider::new(cfg_for(&server)).expect("provider");
    let (tx, mut rx) = tokio::sync::mpsc::channel::<WriterEvent>(64);
    let draft = provider
        .generate_turn_stream(turn_request(), tx, CancellationToken::new())
        .await
        .expect("ok");

    // The committed turn came from the strict parser on the complete text.
    assert_eq!(draft.narration, NARRATION);
    assert_eq!(draft.choices.len(), 1);
    let usage = draft.usage.expect("usage from stream");
    assert_eq!(usage.total_tokens, 15);

    // Drain the sink: only narration prose, never JSON tokens.
    let mut narration = String::new();
    while let Some(ev) = rx.recv().await {
        match ev {
            WriterEvent::Chunk { text } => narration.push_str(&text),
            WriterEvent::Done { .. } => break,
            WriterEvent::Error { .. } => {}
        }
    }
    assert_eq!(narration, NARRATION);
    for token in ["{\"", "\"choices\"", "\"narration\"", "state_patch"] {
        assert!(
            !narration.contains(token),
            "JSON token {token} leaked into the narration stream: {narration:?}"
        );
    }
}

#[tokio::test]
async fn turn_stream_key_split_across_deltas_still_extracts() {
    // One delta ends mid-`"narration"` key; the next completes it. The body
    // is intentionally NOT a full GeneratedTurn, so the final strict parse
    // fails AFTER a chunk was surfaced — which also pins the contract-
    // failure semantics (one non-streaming repair attempt, then the error
    // surfaces; see the dedicated repair test above).
    let sse = format!(
        "{}{}{}data: [DONE]\n\n",
        delta_event("{\"summary\": 1, \"nar"),
        delta_event("ration\": \"비가"),
        delta_event(" 그쳤다.\"}"),
    );

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(sse.into_bytes()))
        .mount(&server)
        .await;

    // max_retries=0: this test pins "one repair attempt", decoupled from the
    // fallback ladder's own transient-retry budget.
    let mut cfg = cfg_for(&server);
    cfg.max_retries = 0;
    let provider = OpenAiCompatibleProvider::new(cfg).expect("provider");
    let (tx, mut rx) = tokio::sync::mpsc::channel::<WriterEvent>(64);
    let result = provider
        .generate_turn_stream(turn_request(), tx, CancellationToken::new())
        .await;

    let mut narration = String::new();
    while let Ok(Some(ev)) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
        match ev {
            WriterEvent::Chunk { text } => narration.push_str(&text),
            WriterEvent::Done { .. } => break,
            WriterEvent::Error { .. } => {}
        }
    }
    // The extractor reassembled the key across deltas and streamed prose only.
    assert_eq!(narration, "비가 그쳤다.");
    // Incomplete contract after surfaced chunks → one non-streaming repair
    // attempt (display-only text is replaced by the committed view). The
    // mock answers the repair with the same SSE body, so the envelope
    // decode fails — the error surfaces and the engine restores the
    // checkpoint; no infinite retries.
    assert!(
        matches!(result, Err(ProviderError::Protocol(_))),
        "expected the failed repair to surface, got {result:?}"
    );
    let n = server.received_requests().await.expect("recorded").len();
    assert_eq!(n, 2, "stream + one repair attempt — got {n}");
}

#[tokio::test]
async fn turn_stream_cancel_mid_stream_returns_cancelled() {
    let sse = format!(
        "{}data: [DONE]\n\n",
        delta_event(&VALID_TURN.chars().take(40).collect::<String>())
    );
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(sse.into_bytes()).set_delay(Duration::from_secs(30)))
        .mount(&server)
        .await;

    let provider = OpenAiCompatibleProvider::new(cfg_for(&server)).expect("provider");
    let (tx, _rx) = tokio::sync::mpsc::channel::<WriterEvent>(16);
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    let handle = tokio::spawn(async move {
        provider
            .generate_turn_stream(turn_request(), tx, cancel_clone)
            .await
    });

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
    // Cancellation is player intent — the fallback must NOT have fired.
    let n = server.received_requests().await.expect("recorded").len();
    assert_eq!(n, 1, "no non-streaming fallback after cancel — got {n}");
}

#[tokio::test]
async fn turn_stream_pre_chunk_failure_falls_back_to_non_streaming() {
    let server = MockServer::start().await;
    // Streaming request -> 500 (single attempt, no retry).
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("\"stream\":true"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    // Non-streaming fallback -> valid turn.
    let content = serde_json::json!(VALID_TURN);
    let envelope = serde_json::json!({
        "id": "chatcmpl-x",
        "model": "test",
        "choices": [
            {"index": 0, "message": {"role": "assistant", "content": content}, "finish_reason": "stop"}
        ],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
    });
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("\"stream\":false"))
        .respond_with(ResponseTemplate::new(200).set_body_json(envelope))
        .mount(&server)
        .await;

    let provider = OpenAiCompatibleProvider::new(cfg_for(&server)).expect("provider");
    let (tx, mut rx) = tokio::sync::mpsc::channel::<WriterEvent>(16);
    let draft = provider
        .generate_turn_stream(turn_request(), tx, CancellationToken::new())
        .await
        .expect("fallback draft");

    assert_eq!(draft.narration, NARRATION);
    // No narration chunk was surfaced before the fallback.
    assert!(
        rx.try_recv().is_err(),
        "no chunks expected before fallback"
    );
    // Exactly one stream attempt + one non-streaming fallback.
    let n = server.received_requests().await.expect("recorded").len();
    assert_eq!(n, 2, "stream attempt + non-streaming fallback — got {n}");
}

#[tokio::test]
async fn turn_stream_timeout_does_not_fall_back() {
    // A slow model times out; re-asking non-streaming would just re-wait
    // the full deadline (do_generate's timeout policy), so the Timeout
    // must surface directly — exactly one request.
    let sse = format!("{}data: [DONE]\n\n", delta_event(VALID_TURN));
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(sse.into_bytes()).set_delay(Duration::from_secs(30)))
        .mount(&server)
        .await;

    let mut cfg = cfg_for(&server);
    cfg.request_timeout_ms = 300;
    let provider = OpenAiCompatibleProvider::new(cfg).expect("provider");
    let (tx, _rx) = tokio::sync::mpsc::channel::<WriterEvent>(16);
    let err = provider
        .generate_turn_stream(turn_request(), tx, CancellationToken::new())
        .await
        .expect_err("timeout must surface");
    assert!(
        matches!(err, ProviderError::Timeout { .. }),
        "expected Timeout, got {err:?}"
    );
    let n = server.received_requests().await.expect("recorded").len();
    assert_eq!(n, 1, "timeout must not trigger a fallback request — got {n}");
}

#[tokio::test]
async fn turn_stream_missing_done_with_complete_json_is_accepted() {
    // Some servers/proxies close the body after the last delta without the
    // `[DONE]` sentinel. A complete, contract-valid text must still commit
    // (streaming is the default path — this must not hard-fail every turn).
    let mut sse = String::new();
    for piece in split_pieces(VALID_TURN, 5) {
        sse.push_str(&delta_event(&piece));
    }
    // No `data: [DONE]`.

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(sse.into_bytes()))
        .mount(&server)
        .await;

    let provider = OpenAiCompatibleProvider::new(cfg_for(&server)).expect("provider");
    let (tx, _rx) = tokio::sync::mpsc::channel::<WriterEvent>(16);
    let draft = provider
        .generate_turn_stream(turn_request(), tx, CancellationToken::new())
        .await
        .expect("complete text without [DONE] is accepted");
    assert_eq!(draft.narration, NARRATION);
    let n = server.received_requests().await.expect("recorded").len();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn turn_stream_contract_failure_after_chunks_retries_non_streaming() {
    // max_tokens truncation after the narration streamed used to self-heal
    // via the non-streaming ladder; the streamed text is display-only and
    // replaced by the committed view, so the retry duplicates nothing.
    let streamed = "{\"narration\": \"잘려 나간 본문\", \"choices\": []}";
    let mut sse = delta_event(streamed);
    sse.push_str("data: [DONE]\n\n");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("\"stream\":true"))
        .respond_with(sse_response(sse.into_bytes()))
        .mount(&server)
        .await;
    let envelope = serde_json::json!({
        "id": "chatcmpl-x",
        "model": "test",
        "choices": [
            {"index": 0, "message": {"role": "assistant", "content": VALID_TURN}, "finish_reason": "stop"}
        ],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
    });
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("\"stream\":false"))
        .respond_with(ResponseTemplate::new(200).set_body_json(envelope))
        .mount(&server)
        .await;

    let provider = OpenAiCompatibleProvider::new(cfg_for(&server)).expect("provider");
    let (tx, _rx) = tokio::sync::mpsc::channel::<WriterEvent>(16);
    let draft = provider
        .generate_turn_stream(turn_request(), tx, CancellationToken::new())
        .await
        .expect("non-streaming repair must succeed");
    assert_eq!(draft.narration, NARRATION);
    let n = server.received_requests().await.expect("recorded").len();
    assert_eq!(n, 2, "stream attempt + one non-streaming repair — got {n}");
}

#[tokio::test]
async fn turn_stream_late_failure_after_chunk_does_not_fall_back() {
    // A stream that surfaces a narration chunk and then dies without [DONE]
    // must surface the protocol error (engine checkpoint recovery), never a
    // silent second request whose text would differ from what was shown.
    let mut body = delta_event("{\"narration\": \"부분").into_bytes();
    body.extend_from_slice(" 텍스트만 흘러간 뒤\"}}]\n\n".as_bytes());
    // No `data: [DONE]` — the stream just ends.

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse_response(body))
        .mount(&server)
        .await;

    let provider = OpenAiCompatibleProvider::new(cfg_for(&server)).expect("provider");
    let (tx, _rx) = tokio::sync::mpsc::channel::<WriterEvent>(16);
    let err = provider
        .generate_turn_stream(turn_request(), tx, CancellationToken::new())
        .await
        .expect_err("stream without [DONE] must fail");
    assert!(
        matches!(err, ProviderError::Protocol(ref m) if m.contains("[DONE]")),
        "expected Protocol([DONE]), got {err:?}"
    );
    let n = server.received_requests().await.expect("recorded").len();
    assert_eq!(n, 1, "no fallback after a surfaced chunk — got {n}");
}
