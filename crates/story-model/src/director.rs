//! Director / Writer split DTOs (ST-033 / ST-034, W10).
//!
//! These pure-data types are the vocabulary of the optional two-stage turn
//! path: a non-streaming **Director** call that returns a validated
//! [`DirectorPlan`] (structural planning, conflict checks, the proposed state
//! patch + choices), followed by a streaming **Writer** call that realizes
//! the plan as Korean scene prose. The Writer has NO authority to emit
//! patches/choices/routes/endings — those come exclusively from the
//! Director's validated plan.
//!
//! As with [`crate::provider_dto`], these DTOs live in `story-model` so neither
//! `story-provider` nor `story-core` has to depend on the other for the wire
//! shape, and they stay free of UI/HTTP/FS (ADR-003).
//!
//! # Serde policy
//!
//! - Struct fields serialize as snake_case (serde default).
//! - [`WriterEvent`] uses `#[serde(tag = "kind", rename_all = "snake_case")]`
//!   so it can cross the provider <-> host boundary as data (e.g. for logging
//!   a streamed chunk).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{CharacterId, Choice, Emotion, StatePatch, TokenUsage};

// ---------------------------------------------------------------------------
// Director plan (output of the non-streaming Director call)
// ---------------------------------------------------------------------------

/// The Director's structural plan for one turn (Technical Spec §10.5).
///
/// Carries everything the engine needs to validate + dry-run the turn BEFORE
/// the Writer is invoked: scene-level intent, per-character emotion arcs, the
/// facts this turn may / must not reveal, and the proposed [`StatePatch`] +
/// [`Choice`]s. The Writer receives a digest of the validated plan and emits
/// ONLY narration prose; it can never alter this plan.
///
/// `proposed_patch` is run through the W3 validator and reducer dry-run; if
/// either fails the whole plan is rejected and the Writer is NOT called
/// (ADR-005).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DirectorPlan {
    /// One-sentence scene-level goal the Writer must realize.
    pub scene_goal: String,
    /// Story beats the narration must hit (in order).
    #[serde(default)]
    pub required_beats: Vec<String>,
    /// Per-character emotion arcs the Writer should voice.
    #[serde(default)]
    pub emotion_arcs: BTreeMap<CharacterId, EmotionArc>,
    /// Per-character current clothing/outfit/visual design notes for this turn.
    #[serde(default)]
    pub character_outfits: BTreeMap<CharacterId, String>,
    /// Detailed ambient situation / environment / weather / atmospheric notes for this turn.
    #[serde(default)]
    pub ambient_situation: Option<String>,
    /// Fact ids the Director authorizes revealing this turn.
    #[serde(default)]
    pub reveal_fact_ids: Vec<String>,
    /// Fact ids the Director forbids revealing this turn. Must be disjoint
    /// from `reveal_fact_ids`; the validator rejects a plan that contradicts
    /// this.
    #[serde(default)]
    pub must_not_reveal_fact_ids: Vec<String>,
    /// The proposed state mutation. Validated + dry-run before the Writer is
    /// called; the Writer can never change it.
    #[serde(default)]
    pub proposed_patch: StatePatch,
    /// Player-facing choices offered this turn (shape-validated by the
    /// Director validator; final authority lives here, not in the Writer).
    pub choices: Vec<Choice>,
    /// Director-declared conflict checks the Writer should be mindful of.
    #[serde(default)]
    pub conflict_checks: Vec<ConflictCheck>,
    /// Human-readable description of the end-condition this turn is steering
    /// toward (non-empty; surfaced to the engine for ending evaluation).
    #[serde(default)]
    pub end_condition: String,
}

/// A single character's emotional trajectory across the turn.
///
/// `character` mirrors the [`BTreeMap`] key in [`DirectorPlan::emotion_arcs`]
/// for ergonomic serialization / per-arc self-description.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmotionArc {
    /// The character this arc applies to.
    pub character: CharacterId,
    /// Emotion at the start of the turn.
    pub start: Emotion,
    /// Emotion at the end of the turn.
    pub end: Emotion,
    /// Optional rationale ( surfaced to the Writer as a cue).
    pub reason: Option<String>,
}

/// A Director-declared conflict to be aware of (resolved or open).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictCheck {
    /// Pack-stable id of this conflict.
    pub id: String,
    /// Human-readable description.
    pub description: String,
    /// Whether the Director considers it resolved by this turn.
    pub resolved: bool,
}

// ---------------------------------------------------------------------------
// Provider request DTOs
// ---------------------------------------------------------------------------

/// A non-streaming Director generation request.
///
/// Mirrors [`crate::TurnRequest`]'s sampling fields so the provider can build
/// the same chat-completions envelope; the host assembles `messages` from the
/// Director prompt fragment + the live prompt context.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DirectorRequest {
    /// Unique id for this generation; stable across retries of the same turn.
    pub generation_id: uuid::Uuid,
    /// Ordered chat messages forming the Director prompt.
    pub messages: Vec<crate::ChatMessage>,
    /// Provider-specific model id to target.
    pub model: String,
    /// Sampling temperature.
    pub temperature: f32,
    /// Nucleus sampling probability mass.
    pub top_p: f32,
    /// Top-k sampling cutoff (0 = provider default / disabled).
    pub top_k: u32,
    /// Maximum output tokens for this turn.
    pub max_tokens: u32,
    /// Optional JSON Schema for the [`DirectorPlan`] shape, sent as
    /// `response_format` to providers that support structured output. The
    /// provider decides whether to attach it (mode + capability); Rust still
    /// strict-parses + validates the plan afterward.
    #[serde(default)]
    pub json_schema: Option<crate::serde_json::Value>,
}

/// A streaming Writer generation request.
///
/// `plan_digest` is a stable digest (e.g. SHA-256 hex) of the validated
/// [`DirectorPlan`]; the Writer prompt includes it so the host can later
/// attest the streamed narration belongs to a confirmed plan. `stream` is
/// always `true` on the wire; it is implicit in the `write_stream` trait
/// method and therefore not modeled here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WriterRequest {
    /// Unique id for this generation; matches the Director's `generation_id`.
    pub generation_id: uuid::Uuid,
    /// Ordered chat messages forming the Writer prompt.
    pub messages: Vec<crate::ChatMessage>,
    /// Provider-specific model id to target.
    pub model: String,
    /// Sampling temperature.
    pub temperature: f32,
    /// Nucleus sampling probability mass.
    pub top_p: f32,
    /// Top-k sampling cutoff (0 = provider default / disabled).
    pub top_k: u32,
    /// Maximum output tokens for this turn.
    pub max_tokens: u32,
    /// Hex-encoded digest of the validated [`DirectorPlan`] this Writer turn
    /// realizes. Surfaced for attestation; not parsed by the engine.
    pub plan_digest: String,
}

// ---------------------------------------------------------------------------
// Stream events + finish
// ---------------------------------------------------------------------------

/// A single event on the Writer's stream.
///
/// The provider yields [`WriterEvent::Chunk`] per SSE data line; the host
/// coalesces them for the renderer. [`WriterEvent::Done`] carries the
/// provider-reported token usage when available. [`WriterEvent::Error`] is for
/// in-stream soft errors that do not tear down the connection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WriterEvent {
    /// A decoded text chunk (UTF-8 whole; never a partial multi-byte split).
    Chunk {
        /// The decoded text fragment.
        text: String,
    },
    /// Stream terminated normally. `usage` is the provider's accounting when
    /// reported.
    Done {
        /// Optional final token usage.
        usage: Option<TokenUsage>,
    },
    /// Recoverable mid-stream error surfaced to the host (the stream may
    /// continue after this).
    Error {
        /// Human-readable error detail.
        detail: String,
    },
}

/// Final aggregate of a completed Writer stream, returned by the provider once
/// `[DONE]` is observed. The host hands this back to the engine as
/// `EngineCommand::AcceptWriterFinish`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WriterFinish {
    /// Echoed generation id (matches the request).
    pub generation_id: uuid::Uuid,
    /// The full reassembled narration text. The engine runs finalize/lint on
    /// this; partial / unconfirmed stream text never reaches this struct.
    pub full_text: String,
    /// Provider-reported token usage, if any.
    pub usage: Option<TokenUsage>,
}

// ---------------------------------------------------------------------------
// Writer lint issue (surfaced by the engine's finalize pass)
// ---------------------------------------------------------------------------

/// A non-fatal issue the engine's Writer-finalize pass flagged.
///
/// These never reject the turn; they are recorded for diagnostics and (in the
/// case of embedded-patch attempts) cause the offending text to be stripped
/// before it reaches history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriterLintIssue {
    /// Stable machine-readable code (e.g. `embedded_patch_attempt`).
    pub code: String,
    /// Human-readable detail.
    pub detail: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CharacterId;

    fn sample_plan() -> DirectorPlan {
        let mut emotion_arcs = BTreeMap::new();
        emotion_arcs.insert(
            CharacterId("c-namu".to_string()),
            EmotionArc {
                character: CharacterId("c-namu".to_string()),
                start: Emotion::Neutral,
                end: Emotion::Happy,
                reason: Some("player kept a promise".to_string()),
            },
        );
        let mut patch = StatePatch::default();
        patch
            .affection_delta
            .insert(CharacterId("c-namu".to_string()), 5);
        DirectorPlan {
            scene_goal: "나무와 카페에서 재회한다".to_string(),
            required_beats: vec!["비가 그친다".to_string(), "나무가 웃는다".to_string()],
            emotion_arcs,
            character_outfits: Default::default(),
            ambient_situation: None,
            reveal_fact_ids: vec!["fact-rain-stopped".to_string()],
            must_not_reveal_fact_ids: vec!["fact-namu-secret".to_string()],
            proposed_patch: patch,
            choices: vec![Choice {
                id: "c1".to_string(),
                label: "우산을 건넨다".to_string(),
                intent: "kindness".to_string(),
                tone: Some("warm".to_string()),
                target: Some(CharacterId("c-namu".to_string())),
                risk: crate::RiskLevel::Safe,
                hidden_requirements: vec![],
                preview: None,
            }],
            conflict_checks: vec![ConflictCheck {
                id: "conflict-umbrella".to_string(),
                description: "who keeps the umbrella".to_string(),
                resolved: true,
            }],
            end_condition: "scene continues".to_string(),
        }
    }

    #[test]
    fn director_plan_round_trips_through_json() {
        let plan = sample_plan();
        let json = serde_json::to_string(&plan).expect("serialize");
        let back: DirectorPlan = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(plan, back);
    }

    #[test]
    fn director_plan_serializes_with_expected_keys() {
        let plan = sample_plan();
        let json = serde_json::to_string(&plan).expect("serialize");
        // Snake_case wire shape.
        assert!(json.contains("\"scene_goal\""));
        assert!(json.contains("\"required_beats\""));
        assert!(json.contains("\"emotion_arcs\""));
        assert!(json.contains("\"reveal_fact_ids\""));
        assert!(json.contains("\"must_not_reveal_fact_ids\""));
        assert!(json.contains("\"proposed_patch\""));
        assert!(json.contains("\"conflict_checks\""));
        assert!(json.contains("\"end_condition\""));
        // CharacterId serializes transparently as the inner string.
        assert!(json.contains("\"c-namu\""));
    }

    #[test]
    fn default_plan_is_empty() {
        let p = DirectorPlan::default();
        assert!(p.scene_goal.is_empty());
        assert!(p.required_beats.is_empty());
        assert!(p.emotion_arcs.is_empty());
        assert!(p.reveal_fact_ids.is_empty());
        assert!(p.must_not_reveal_fact_ids.is_empty());
        assert!(p.proposed_patch.is_empty());
        assert!(p.choices.is_empty());
        assert!(p.conflict_checks.is_empty());
        assert!(p.end_condition.is_empty());
    }

    #[test]
    fn director_and_writer_requests_round_trip() {
        let director = DirectorRequest {
            generation_id: uuid::Uuid::nil(),
            messages: vec![crate::ChatMessage {
                role: "system".to_string(),
                content: "plan".to_string(),
            }],
            model: "mlx".to_string(),
            temperature: 0.7,
            top_p: 0.9,
            top_k: 40,
            max_tokens: 1024,
            json_schema: None,
        };
        let json = serde_json::to_string(&director).expect("serialize");
        let back: DirectorRequest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(director, back);

        let writer = WriterRequest {
            generation_id: uuid::Uuid::nil(),
            messages: vec![crate::ChatMessage {
                role: "system".to_string(),
                content: "write".to_string(),
            }],
            model: "mlx".to_string(),
            temperature: 0.85,
            top_p: 0.92,
            top_k: 40,
            max_tokens: 1800,
            plan_digest: "abcdef".to_string(),
        };
        let json = serde_json::to_string(&writer).expect("serialize");
        let back: WriterRequest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(writer, back);
        assert!(json.contains("\"plan_digest\""));
    }

    #[test]
    fn writer_event_tagged_serde_round_trips() {
        let chunk = WriterEvent::Chunk {
            text: "비가".to_string(),
        };
        let json = serde_json::to_string(&chunk).expect("serialize");
        assert!(
            json.contains("\"kind\":\"chunk\""),
            "chunk tagged incorrectly: {json}"
        );
        let back: WriterEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(chunk, back);

        let done = WriterEvent::Done {
            usage: Some(TokenUsage {
                prompt_tokens: 1,
                completion_tokens: 2,
                total_tokens: 3,
            }),
        };
        let json = serde_json::to_string(&done).expect("serialize");
        assert!(
            json.contains("\"kind\":\"done\""),
            "done tagged incorrectly: {json}"
        );
        let back: WriterEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(done, back);

        let err = WriterEvent::Error {
            detail: "oops".to_string(),
        };
        let json = serde_json::to_string(&err).expect("serialize");
        assert!(
            json.contains("\"kind\":\"error\""),
            "error tagged incorrectly: {json}"
        );
        let back: WriterEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(err, back);
    }

    #[test]
    fn writer_finish_round_trips() {
        let finish = WriterFinish {
            generation_id: uuid::Uuid::nil(),
            full_text: "비가 그쳤다.".to_string(),
            usage: None,
        };
        let json = serde_json::to_string(&finish).expect("serialize");
        let back: WriterFinish = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(finish, back);
    }

    #[test]
    fn writer_lint_issue_round_trips() {
        let issue = WriterLintIssue {
            code: "embedded_patch_attempt".to_string(),
            detail: "fenced JSON block stripped".to_string(),
        };
        let json = serde_json::to_string(&issue).expect("serialize");
        let back: WriterLintIssue = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(issue, back);
    }
}
