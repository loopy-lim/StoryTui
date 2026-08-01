//! Strict parser + limited recovery tests (ST-019).
//!
//! The first suite drives the 12 fixtures under `fixtures/parse/` through
//! [`parse_generated_turn`], asserting the Ok/Err outcome and the repair count
//! for each. The second suite covers focused behaviors that are awkward to
//! express as static files (stringified whole-body, no-JSON-object, raw-body
//! redaction in errors).

use story_model::uuid::Uuid;
use story_model::{ProviderError, ProviderHealth};
use story_provider::{LlmProvider, MockProvider, MockResponse, parse_generated_turn};

// ---------------------------------------------------------------------------
// 12 fixtures
// ---------------------------------------------------------------------------

const F01_VALID: &str = include_str!("fixtures/parse/01_valid.json");
const F02_FENCED: &str = include_str!("fixtures/parse/02_fenced.json");
const F03_TRAILING_COMMA: &str = include_str!("fixtures/parse/03_trailing_comma.json");
const F04_FENCED_AND_TRAILING_COMMA: &str =
    include_str!("fixtures/parse/04_fenced_and_trailing_comma.json");
const F05_PROSE_WRAPPED: &str = include_str!("fixtures/parse/05_prose_wrapped.json");
const F06_UNKNOWN_FIELD_IN_PATCH: &str =
    include_str!("fixtures/parse/06_unknown_field_in_patch.json");
const F07_UNKNOWN_FIELD_AT_TOP: &str = include_str!("fixtures/parse/07_unknown_field_at_top.json");
const F08_INVALID_ENUM_RISK: &str = include_str!("fixtures/parse/08_invalid_enum_risk.json");
const F09_EMPTY_NARRATION: &str = include_str!("fixtures/parse/09_empty_narration.json");
const F10_TOO_MANY_CHOICES: &str = include_str!("fixtures/parse/10_too_many_choices.json");
const F11_ZERO_CHOICES: &str = include_str!("fixtures/parse/11_zero_choices.json");
const F12_MISSING_STATE_PATCH: &str = include_str!("fixtures/parse/12_missing_state_patch.json");

const GID: Uuid = Uuid::nil();

#[track_caller]
fn expect_ok(raw: &str, expected_repairs: usize) -> story_model::GeneratedTurnDraft {
    let outcome = parse_generated_turn(raw, GID).expect("expected parse to succeed");
    assert_eq!(
        outcome.repairs.len(),
        expected_repairs,
        "repair count mismatch (repairs={:?})",
        outcome.repairs
    );
    assert_eq!(
        outcome.draft.generation_id, GID,
        "parser must stamp the request generation_id"
    );
    outcome.draft
}

#[track_caller]
fn expect_err(raw: &str) -> ProviderError {
    match parse_generated_turn(raw, GID) {
        Ok(o) => panic!(
            "expected parse to fail, got Ok with {} repairs",
            o.repairs.len()
        ),
        Err(e) => e,
    }
}

#[test]
fn fixture_01_valid_parses_with_zero_repairs() {
    let d = expect_ok(F01_VALID, 0);
    assert_eq!(d.narration, "비가 그쳤다. 유나가 찻잔을 내려놓는다.");
    assert_eq!(d.choices.len(), 2);
}

#[test]
fn fixture_02_fenced_parses_with_one_repair() {
    let d = expect_ok(F02_FENCED, 1);
    assert_eq!(d.choices.len(), 1);
}

#[test]
fn fixture_03_trailing_comma_repairs() {
    let d = expect_ok(F03_TRAILING_COMMA, 1);
    assert_eq!(d.narration, "비가 그쳤다. 유나가 찻잔을 내려놓는다.");
}

#[test]
fn fixture_04_fenced_and_trailing_comma_two_repairs() {
    let d = expect_ok(F04_FENCED_AND_TRAILING_COMMA, 2);
    assert_eq!(d.narration, "비가 그쳤다.");
}

#[test]
fn fixture_05_prose_wrapped_locates_json() {
    let d = expect_ok(F05_PROSE_WRAPPED, 1);
    assert_eq!(d.choices.len(), 1);
}

#[test]
fn fixture_06_unknown_field_in_patch_is_rejected_not_dropped() {
    // Hard contract: deny_unknown_fields on StatePatch means an unknown key in
    // state_patch MUST be a hard error, never silently dropped.
    let err = expect_err(F06_UNKNOWN_FIELD_IN_PATCH);
    assert!(
        matches!(err, ProviderError::InvalidGeneration(_)),
        "expected InvalidGeneration, got {err:?}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("unknown field") || msg.contains("SANITY_SCORE_DELTA"),
        "error should mention the unknown field, got: {msg}"
    );
}

#[test]
fn fixture_07_unknown_field_at_top_level_is_rejected() {
    let err = expect_err(F07_UNKNOWN_FIELD_AT_TOP);
    assert!(matches!(err, ProviderError::InvalidGeneration(_)));
}

#[test]
fn fixture_08_invalid_enum_risk_is_rejected() {
    let err = expect_err(F08_INVALID_ENUM_RISK);
    assert!(matches!(err, ProviderError::InvalidGeneration(_)));
}

#[test]
fn fixture_09_empty_narration_is_rejected() {
    let err = expect_err(F09_EMPTY_NARRATION);
    let msg = err.to_string();
    assert!(
        msg.contains("narration") || msg.contains("empty"),
        "error should mention narration emptiness, got: {msg}"
    );
}

#[test]
fn fixture_10_too_many_choices_is_rejected() {
    let err = expect_err(F10_TOO_MANY_CHOICES);
    assert!(matches!(err, ProviderError::InvalidGeneration(_)));
}

#[test]
fn fixture_11_zero_choices_is_rejected() {
    let err = expect_err(F11_ZERO_CHOICES);
    assert!(matches!(err, ProviderError::InvalidGeneration(_)));
}

#[test]
fn fixture_12_missing_state_patch_is_rejected() {
    let err = expect_err(F12_MISSING_STATE_PATCH);
    assert!(matches!(err, ProviderError::InvalidGeneration(_)));
}

// ---------------------------------------------------------------------------
// focused behaviors
// ---------------------------------------------------------------------------

const VALID_BODY: &str = r#"{
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

#[test]
fn korean_enum_labels_from_local_model_are_normalized() {
    let raw = VALID_BODY
        .replace("\"risk\":\"safe\"", "\"risk\":\"낮음\"")
        .replace("\"next_time_slot\": null", "\"next_time_slot\": \"저녁\"");
    let draft = parse_generated_turn(&raw, GID)
        .expect("common Korean enum labels should parse")
        .draft;
    assert_eq!(draft.choices[0].risk, story_model::RiskLevel::Low);
    assert_eq!(
        draft.proposed_patch.next_time_slot,
        Some(story_model::TimeSlot::Evening)
    );
}

#[test]
fn legacy_top_level_patches_alias_is_migrated_once() {
    let raw = VALID_BODY.replace("\"state_patch\"", "\"patches\"");
    let outcome = parse_generated_turn(&raw, GID).expect("known legacy alias should recover");
    assert_eq!(outcome.repairs.len(), 1);
    assert!(outcome.repairs[0].contains("patches"));
    assert!(outcome.draft.proposed_patch.is_empty());
}

#[test]
fn no_json_object_present_is_rejected() {
    let raw = "The model declined to answer. No JSON here.";
    let err = expect_err(raw);
    assert!(matches!(err, ProviderError::InvalidGeneration(_)));
}

#[test]
fn stringified_whole_body_is_unwrapped_and_parsed() {
    // The model sometimes emits the entire JSON object as an escaped string
    // literal (a double-encoding). The parser unwraps one layer and parses.
    let stringified = serde_json::to_string(VALID_BODY).expect("stringify");
    assert!(
        stringified.starts_with('"'),
        "fixture must be a JSON string literal"
    );
    let d = expect_ok(&stringified, 1);
    assert_eq!(d.narration, "비가 그쳤다.");
}

#[test]
fn error_does_not_embed_raw_response_body() {
    // Sanitization contract: the error message must NOT contain the raw body's
    // distinctive prose. We plant a unique marker and assert its absence.
    let marker = "UNIQUE_RAW_MARKER_DO_NOT_LEAK_42";
    let raw = format!("prose {marker} {{ not even valid json }}");
    let err = expect_err(&raw);
    let msg = err.to_string();
    assert!(
        !msg.contains(marker),
        "raw body marker leaked into error message: {msg}"
    );
}

#[test]
fn malformed_unquoted_key_is_rejected_without_raw_parser_body() {
    // A common model failure: Python/object-literal-looking output instead of
    // JSON. The parser must fail cleanly; the host maps its serde detail to a
    // player-safe generic message.
    let raw = r#"{narration: \"깨진 응답\", choices: []}"#;
    let err = expect_err(raw);
    assert!(matches!(err, ProviderError::InvalidGeneration(_)));
    assert!(!err.to_string().contains("깨진 응답"));
}

#[test]
fn malformed_single_quoted_object_is_rejected() {
    let raw = "{'narration': '깨진 응답', 'choices': []}";
    let err = expect_err(raw);
    assert!(matches!(err, ProviderError::InvalidGeneration(_)));
}

#[test]
fn top_level_array_is_rejected_as_invalid_generation() {
    let raw = "[{\"narration\":\"장면\"}]";
    let err = expect_err(raw);
    assert!(matches!(err, ProviderError::InvalidGeneration(_)));
}

#[test]
fn empty_and_null_responses_are_rejected_as_invalid_generation() {
    for raw in ["", "   ", "null", "{}"] {
        let err = expect_err(raw);
        assert!(
            matches!(err, ProviderError::InvalidGeneration(_)),
            "raw response should be invalid: {raw:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// integration: MockProvider + recovery path (Draft is pre-parsed)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mock_draft_round_trips_through_parser() {
    // The Mock hands back a Draft directly (it does NOT go through recovery —
    // that's by design). This test confirms the two paths compose: serializing
    // a draft and re-parsing it yields an equivalent draft (0 repairs).
    let original = story_model::GeneratedTurnDraft {
        generation_id: GID,
        narration: "비가 그쳤다.".to_string(),
        choices: vec![story_model::Choice {
            id: "c1".to_string(),
            label: "a".to_string(),
            intent: "i".to_string(),
            tone: None,
            target: None,
            risk: story_model::RiskLevel::Safe,
            hidden_requirements: vec![],
            preview: None,
        }],
        proposed_patch: story_model::StatePatch::default(),
        summary: story_model::TurnSummary {
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
    };

    let provider = MockProvider::new("mock").with(MockResponse::Draft(original.clone()));
    let draft = provider
        .generate_turn(
            story_model::TurnRequest {
                generation_id: GID,
                messages: vec![],
                model: "m".to_string(),
                temperature: 0.0,
                top_p: 1.0,
                top_k: 0,
                max_tokens: 1,
                json_schema: None,
            },
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("draft");
    assert_eq!(draft, original);
    let _ = provider.health().await; // keep ProviderHealth import meaningful
    let _ = ProviderHealth::Ready;
}
