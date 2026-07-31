//! Round-trip tests for newtype IDs (transparent) and core enums (snake_case).

use serde_json::{from_str, to_string};
use story_model::{
    ActionType, CharacterId, CompareOp, Emotion, EventId, FlagId, ItemId, LocationId, MemoryKind,
    RelationshipStage, RiskLevel, TimeSlot,
};

fn round_trip<T>(value: &T, expected_json: &str)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let serialized = to_string(value).expect("serialize");
    assert_eq!(
        serialized, expected_json,
        "serialized JSON does not match expected"
    );
    let deserialized: T = from_str(&serialized).expect("deserialize");
    assert_eq!(&deserialized, value, "round-trip not equal");
}

/// Round-trip a `Copy` enum variant whose snake_case JSON form is a bare
/// string token, comparing against the unquoted name.
fn round_trip_enum<T>(value: &T, expected_snake: &str)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let expected_json = format!("\"{expected_snake}\"");
    round_trip(value, &expected_json);
}

#[test]
fn ids_serialize_as_transparent_bare_strings() {
    // Transparent newtypes MUST render as the bare inner string, never as an
    // object like {"0":"..."}.
    let cid = CharacterId("c-namu".to_string());
    assert_eq!(serde_json::to_string(&cid).unwrap(), "\"c-namu\"");
    // Confirm the JSON is a bare string, not an object.
    let parsed: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&cid).unwrap()).unwrap();
    assert!(
        parsed.is_string(),
        "transparent ID must serialize as a JSON string, got: {parsed}"
    );
}

#[test]
fn all_id_types_round_trip_individually() {
    round_trip(&CharacterId("c-namu".to_string()), "\"c-namu\"");
    round_trip(&LocationId("loc-hall".to_string()), "\"loc-hall\"");
    round_trip(&FlagId("f-open".to_string()), "\"f-open\"");
    round_trip(&EventId("e-bell".to_string()), "\"e-bell\"");
    round_trip(&ItemId("i-key".to_string()), "\"i-key\"");
}

#[test]
fn relationship_stage_serializes_snake_case() {
    let cases = [
        (RelationshipStage::Stranger, "stranger"),
        (RelationshipStage::Acquaintance, "acquaintance"),
        (RelationshipStage::Friend, "friend"),
        (RelationshipStage::CloseFriend, "close_friend"),
        (RelationshipStage::RomanticInterest, "romantic_interest"),
        (RelationshipStage::Partner, "partner"),
        (RelationshipStage::Estranged, "estranged"),
    ];
    for (variant, expected) in cases {
        round_trip_enum(&variant, expected);
    }
}

#[test]
fn emotion_serializes_snake_case() {
    let cases = [
        (Emotion::Neutral, "neutral"),
        (Emotion::Happy, "happy"),
        (Emotion::Sad, "sad"),
        (Emotion::Angry, "angry"),
        (Emotion::Anxious, "anxious"),
        (Emotion::Surprised, "surprised"),
        (Emotion::Affectionate, "affectionate"),
        (Emotion::Cold, "cold"),
        (Emotion::Embarrassed, "embarrassed"),
        (Emotion::Guilty, "guilty"),
    ];
    for (variant, expected) in cases {
        round_trip_enum(&variant, expected);
    }
}

#[test]
fn time_slot_serializes_snake_case() {
    let cases = [
        (TimeSlot::Dawn, "dawn"),
        (TimeSlot::Morning, "morning"),
        (TimeSlot::Afternoon, "afternoon"),
        (TimeSlot::Evening, "evening"),
        (TimeSlot::Night, "night"),
    ];
    for (variant, expected) in cases {
        round_trip_enum(&variant, expected);
    }
}

#[test]
fn risk_level_serializes_snake_case() {
    let cases = [
        (RiskLevel::Safe, "safe"),
        (RiskLevel::Low, "low"),
        (RiskLevel::Medium, "medium"),
        (RiskLevel::High, "high"),
        (RiskLevel::Severe, "severe"),
    ];
    for (variant, expected) in cases {
        round_trip_enum(&variant, expected);
    }
}

#[test]
fn compare_op_serializes_snake_case() {
    let cases = [
        (CompareOp::Eq, "eq"),
        (CompareOp::Ne, "ne"),
        (CompareOp::Lt, "lt"),
        (CompareOp::Le, "le"),
        (CompareOp::Gt, "gt"),
        (CompareOp::Ge, "ge"),
    ];
    for (variant, expected) in cases {
        round_trip_enum(&variant, expected);
    }
}

#[test]
fn action_type_serializes_snake_case() {
    let cases = [
        (ActionType::Choose, "choose"),
        (ActionType::Dialogue, "dialogue"),
        (ActionType::Action, "action"),
        (ActionType::Move, "move"),
        (ActionType::GeneralAction, "general_action"),
    ];
    for (variant, expected) in cases {
        round_trip_enum(&variant, expected);
    }
}

#[test]
fn memory_kind_serializes_snake_case() {
    let cases = [
        (MemoryKind::Promise, "promise"),
        (MemoryKind::SecretReveal, "secret_reveal"),
        (MemoryKind::RelationshipChange, "relationship_change"),
        (MemoryKind::KeyItem, "key_item"),
        (MemoryKind::FirstMeeting, "first_meeting"),
        (MemoryKind::Confession, "confession"),
        (MemoryKind::Betrayal, "betrayal"),
        (MemoryKind::Other, "other"),
    ];
    for (variant, expected) in cases {
        round_trip_enum(&variant, expected);
    }
}

#[test]
fn bare_string_variant_ids_load_back_directly() {
    // A save written by an older tool can hand us a bare JSON string; the
    // transparent newtype must accept it without a wrapper object.
    let c: CharacterId = serde_json::from_str("\"c-namu\"").expect("from bare string");
    assert_eq!(c.0, "c-namu");
    let l: LocationId = serde_json::from_str("\"loc-roof\"").expect("from bare string");
    assert_eq!(l.0, "loc-roof");
}
