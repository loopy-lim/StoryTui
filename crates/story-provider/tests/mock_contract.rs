//! MockProvider contract tests (ST-016).
//!
//! These exercise the transport-level behavior of `MockProvider`:
//! success, latency (`Delayed`), `Timeout`, `Error`, and the critical
//! cancellation contract: cancelling during a `Delayed`/`Late` response MUST
//! return `ProviderError::Cancelled` and the late draft MUST NEVER surface.
//!
//! No network / no reqwest — `MockProvider` is purely in-process (it sleeps
//! against `tokio::time` and selects against a `CancellationToken`).

use std::time::Duration;

use story_model::uuid::Uuid;
use story_model::{
    ChatMessage, GeneratedTurnDraft, ProviderCapabilities, ProviderError, ProviderHealth,
    StatePatch, TurnRequest, TurnSummary,
};
use story_provider::{LlmProvider, MockProvider, MockResponse};
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn sample_request() -> TurnRequest {
    TurnRequest {
        generation_id: Uuid::nil(),
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: "hi".to_string(),
        }],
        model: "mock-model".to_string(),
        temperature: 0.5,
        top_p: 1.0,
        top_k: 0,
        max_tokens: 256,
        json_schema: None,
    }
}

fn draft_with_marker(marker: &str) -> GeneratedTurnDraft {
    GeneratedTurnDraft {
        generation_id: Uuid::nil(),
        narration: marker.to_string(),
        choices: vec![],
        proposed_patch: StatePatch::default(),
        summary: TurnSummary {
            facts: vec![],
            unresolved: vec![],
            relationship_changes: vec![],
            knowledge_changes: vec![],
            interpretation: None,
            address_form_changes: vec![],
            important_player_actions: vec![],
            source_history_ids: vec![],
        },
        usage: None,
    }
}

fn mvp_caps() -> ProviderCapabilities {
    ProviderCapabilities {
        supports_structured_output: false,
        supports_director: false,
        supports_streaming: false,
    }
}

// ---------------------------------------------------------------------------
// identity / capabilities / health
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mock_id_and_capabilities_reflect_configuration() {
    let p = MockProvider::new("mock-1");
    assert_eq!(p.id(), "mock-1");
    // MVP defaults: all capabilities false.
    assert_eq!(p.capabilities(), mvp_caps());
    assert_eq!(p.health().await, Ok(ProviderHealth::Ready));
}

#[tokio::test]
async fn mock_health_reflects_configured_state() {
    let p = MockProvider::new("mock").with_health(ProviderHealth::Degraded);
    assert_eq!(p.health().await, Ok(ProviderHealth::Degraded));
}

// ---------------------------------------------------------------------------
// success / delay
// ---------------------------------------------------------------------------

#[tokio::test]
async fn draft_response_is_returned_immediately() {
    let expected = draft_with_marker("OK");
    let p = MockProvider::new("mock").with(MockResponse::Draft(expected.clone()));
    let got = p
        .generate_turn(sample_request(), CancellationToken::new())
        .await;
    assert_eq!(got, Ok(expected));
}

#[tokio::test]
async fn delayed_response_succeeds_after_latency() {
    let expected = draft_with_marker("DELAYED-OK");
    let p = MockProvider::new("mock").with(MockResponse::Delayed {
        draft: expected.clone(),
        ms: 20,
    });
    let got = p
        .generate_turn(sample_request(), CancellationToken::new())
        .await;
    assert_eq!(got, Ok(expected));
}

#[tokio::test]
async fn queue_is_drained_in_fifo_order() {
    let p = MockProvider::new("mock")
        .with(MockResponse::Draft(draft_with_marker("a")))
        .with(MockResponse::Draft(draft_with_marker("b")));
    let r1 = p
        .generate_turn(sample_request(), CancellationToken::new())
        .await
        .unwrap();
    let r2 = p
        .generate_turn(sample_request(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(r1.narration, "a");
    assert_eq!(r2.narration, "b");
}

#[tokio::test]
async fn empty_queue_returns_unavailable_error() {
    let p = MockProvider::new("mock");
    let res = p
        .generate_turn(sample_request(), CancellationToken::new())
        .await;
    assert!(matches!(res, Err(ProviderError::Unavailable(_))), "{res:?}");
}

// ---------------------------------------------------------------------------
// timeout / error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn timeout_response_returns_timeout_error() {
    let p = MockProvider::new("mock").with(MockResponse::Timeout { ms: 10 });
    let res = p
        .generate_turn(sample_request(), CancellationToken::new())
        .await;
    assert_eq!(res, Err(ProviderError::Timeout { ms: 10 }));
}

#[tokio::test]
async fn error_response_propagates_verbatim() {
    let err = ProviderError::Protocol("bad envelope".to_string());
    let p = MockProvider::new("mock").with(MockResponse::Error(err.clone()));
    let res = p
        .generate_turn(sample_request(), CancellationToken::new())
        .await;
    assert_eq!(res, Err(err));
}

// ---------------------------------------------------------------------------
// cancellation — the critical contract
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cancel_during_delayed_response_returns_cancelled() {
    let expected = draft_with_marker("SHOULD-STILL-SURFACE-WITHOUT-CANCEL");
    let p = MockProvider::new("mock").with(MockResponse::Delayed {
        draft: expected,
        ms: 200,
    });
    let cancel = CancellationToken::new();

    // Fire cancel shortly after the request starts; generate_turn's internal
    // select! should observe it and return Cancelled.
    let cancel2 = cancel.clone();
    let (res, _) = tokio::join!(p.generate_turn(sample_request(), cancel), async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel2.cancel();
    });

    assert_eq!(res, Err(ProviderError::Cancelled));
}

#[tokio::test]
async fn cancel_during_late_response_returns_cancelled_and_never_surfaces_draft() {
    // The "late" draft completes only after a long delay. When the caller
    // cancels during the wait, the draft MUST be discarded and never returned.
    let late_draft = draft_with_marker("LATE-DRAFT-MUST-NEVER-SURFACE");
    let p = MockProvider::new("mock").with(MockResponse::Late {
        draft: late_draft.clone(),
        ms: 5_000,
    });
    let cancel = CancellationToken::new();

    let cancel2 = cancel.clone();
    let (res, _) = tokio::join!(p.generate_turn(sample_request(), cancel), async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        cancel2.cancel();
    });

    // Hard contract: cancelled, not the draft.
    assert_eq!(res, Err(ProviderError::Cancelled));
    // And defensively: if the result had been Ok, its narration would equal the
    // late marker — this assertion documents what we must never observe.
    if let Ok(d) = res {
        panic!("late draft surfaced after cancel: {:?}", d.narration);
    }
    // Reference `late_draft` so the marker stays meaningful for readers; it is
    // exactly the value that must never be returned.
    assert_eq!(late_draft.narration, "LATE-DRAFT-MUST-NEVER-SURFACE");
}

// ---------------------------------------------------------------------------
// unsupported capability paths (MVP)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn direct_and_write_stream_are_unsupported_in_mvp() {
    let p = MockProvider::new("mock");
    let cancel = CancellationToken::new();
    let direct = p.direct((), cancel.clone()).await;
    assert!(
        matches!(direct, Err(ProviderError::UnsupportedCapability(ref s)) if s == "director"),
        "{direct:?}"
    );
    let stream = p.write_stream((), (), cancel).await;
    assert!(
        matches!(stream, Err(ProviderError::UnsupportedCapability(ref s)) if s == "streaming"),
        "{stream:?}"
    );
}

// ---------------------------------------------------------------------------
// capability advertised via the trait matches MVP defaults
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mock_advertises_no_structured_output_by_default() {
    let p = MockProvider::new("mock");
    assert!(!p.capabilities().supports_structured_output);
    assert!(!p.capabilities().supports_director);
    assert!(!p.capabilities().supports_streaming);
}
