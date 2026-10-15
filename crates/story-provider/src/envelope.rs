//! OpenAI-compatible chat-completions envelope (ST-017, Technical Spec §10.3).
//!
//! Request body shape sent to `/v1/chat/completions`. The response shape is
//! parsed serde-tolerantly (unknown fields are IGNORED, never rejected) so the
//! provider survives minor server drift (MLX-LM / MLX-VLM add a few extra
//! top-level keys).
//!
//! The assistant message body extracted from a response is fed to the existing
//! [`crate::recovery::parse_generated_turn`] — parsing is NOT reimplemented here.

use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};

use crate::config::MaxTokensField;
use story_model::{ChatMessage, DirectorRequest, TokenUsage, TurnRequest, WriterRequest};

/// OpenAI chat-completions request body.
///
/// Serialization is hand-rolled because the output token cap is emitted under
/// a configurable key: local OpenAI-compatible servers expect `max_tokens`,
/// while the OpenAI cloud API expects `max_completion_tokens`
/// (see [`MaxTokensField`]). Key order mirrors the derive it replaces.
pub struct ChatCompletionRequest {
    /// Model id to target.
    pub model: String,
    /// Ordered chat messages forming the prompt.
    pub messages: Vec<ChatMessage>,
    /// Sampling temperature.
    pub temperature: f32,
    /// Nucleus sampling probability mass.
    pub top_p: f32,
    /// Top-k sampling cutoff. Omitted from the wire body when `0` because some
    /// servers reject the field.
    pub top_k: u32,
    /// Maximum output tokens.
    pub max_tokens: u32,
    /// Wire key selection for the output token cap.
    pub max_tokens_field: MaxTokensField,
    /// Streaming flag (always `false` for the W6 MVP).
    pub stream: bool,
    /// Structured-output constraint, included only when the provider's
    /// capability confirms support AND the request carries a schema.
    pub response_format: Option<ResponseFormat>,
}

impl Serialize for ChatCompletionRequest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("model", &self.model)?;
        map.serialize_entry("messages", &self.messages)?;
        map.serialize_entry("temperature", &self.temperature)?;
        map.serialize_entry("top_p", &self.top_p)?;
        if self.top_k != 0 {
            map.serialize_entry("top_k", &self.top_k)?;
        }
        match self.max_tokens_field {
            MaxTokensField::MaxTokens => map.serialize_entry("max_tokens", &self.max_tokens)?,
            MaxTokensField::MaxCompletionTokens => {
                map.serialize_entry("max_completion_tokens", &self.max_tokens)?;
            }
        }
        map.serialize_entry("stream", &self.stream)?;
        if let Some(response_format) = &self.response_format {
            map.serialize_entry("response_format", response_format)?;
        }
        map.end()
    }
}

/// `response_format` payload (OpenAI structured-output shape).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ResponseFormat {
    /// Discriminator; `"json_schema"` when a schema is attached.
    #[serde(rename = "type")]
    pub kind: String,
    /// Optional JSON Schema spec (present when `kind == "json_schema"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_schema: Option<JsonSchemaSpec>,
}

/// The `json_schema` details object.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JsonSchemaSpec {
    /// Name for the schema (OpenAI requires this field).
    pub name: String,
    /// Strict-mode flag (only one `GeneratedTurn` shape is permitted).
    pub strict: bool,
    /// The JSON Schema itself.
    pub schema: serde_json::Value,
}

impl ChatCompletionRequest {
    /// Build the envelope from a [`TurnRequest`]. `send_response_format` is the
    /// provider's resolved decision (mode + capability + presence of a schema);
    /// when `false`, `response_format` is omitted entirely.
    pub fn from_turn(req: &TurnRequest, model: &str, send_response_format: bool) -> Self {
        let response_format = if send_response_format {
            req.json_schema.as_ref().map(|schema| ResponseFormat {
                kind: "json_schema".to_string(),
                json_schema: Some(JsonSchemaSpec {
                    name: "generated_turn".to_string(),
                    strict: true,
                    schema: schema.clone(),
                }),
            })
        } else {
            None
        };
        Self {
            model: model.to_string(),
            messages: req.messages.clone(),
            temperature: req.temperature,
            top_p: req.top_p,
            top_k: req.top_k,
            max_tokens: req.max_tokens,
            max_tokens_field: MaxTokensField::MaxTokens,
            stream: false,
            response_format,
        }
    }

    /// Build a non-streaming envelope for a Director generation request (W10).
    /// Mirrors [`Self::from_turn`]: when `send_response_format` is true and the
    /// request carries a schema, the [`DirectorPlan`] JSON shape is attached as
    /// `response_format` so guided-decoding servers emit the required fields
    /// under the canonical keys. Rust still strict-parses + validates after.
    pub fn from_director(req: &DirectorRequest, model: &str, send_response_format: bool) -> Self {
        let response_format = if send_response_format {
            req.json_schema.as_ref().map(|schema| ResponseFormat {
                kind: "json_schema".to_string(),
                json_schema: Some(JsonSchemaSpec {
                    name: "director_plan".to_string(),
                    // Non-strict: the plan has dynamic-keyed maps and many
                    // optional fields that do not fit OpenAI strict's
                    // all-properties-required rule.
                    strict: false,
                    schema: schema.clone(),
                }),
            })
        } else {
            None
        };
        Self {
            model: model.to_string(),
            messages: req.messages.clone(),
            temperature: req.temperature,
            top_p: req.top_p,
            top_k: req.top_k,
            max_tokens: req.max_tokens,
            max_tokens_field: MaxTokensField::MaxTokens,
            stream: false,
            response_format,
        }
    }

    /// Build a streaming envelope for a Writer generation request (W10). Forces
    /// `stream: true` so the server returns an SSE body; no `response_format`
    /// — the Writer emits prose, not JSON.
    pub fn from_writer(req: &WriterRequest, model: &str) -> Self {
        Self {
            model: model.to_string(),
            messages: req.messages.clone(),
            temperature: req.temperature,
            top_p: req.top_p,
            top_k: req.top_k,
            max_tokens: req.max_tokens,
            max_tokens_field: MaxTokensField::MaxTokens,
            stream: true,
            response_format: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Response (serde-tolerant: unknown fields are allowed / ignored)
// ---------------------------------------------------------------------------

/// Top-level chat-completions response. Only the keys we consume are typed;
/// everything else (server-specific fields) is ignored.
#[derive(Deserialize, Debug)]
pub struct ChatCompletionResponse {
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize, Debug)]
struct Choice {
    #[serde(default)]
    message: Option<ResponseMessage>,
    /// OpenAI-compatible servers use this to distinguish a normal completion
    /// (`stop`) from an output cap (`length`). It is diagnostic metadata only;
    /// the game contract still validates the assistant content independently.
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Debug)]
struct ResponseMessage {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct Usage {
    #[serde(default)]
    prompt_tokens: Option<u32>,
    #[serde(default)]
    completion_tokens: Option<u32>,
    #[serde(default)]
    total_tokens: Option<u32>,
}

impl ChatCompletionResponse {
    /// Extract the first choice's assistant content, or an empty string when
    /// the server omitted it. The caller runs the result through the recovery
    /// parser, which rejects empty bodies.
    pub fn content(&self) -> String {
        self.choices
            .first()
            .and_then(|c| c.message.as_ref())
            .and_then(|m| m.content.clone())
            .unwrap_or_default()
    }

    /// Map the server-reported usage into a [`TokenUsage`] when at least
    /// `prompt_tokens` is present. Missing `total_tokens` is derived; missing
    /// `completion_tokens` defaults to `0`.
    pub fn usage(&self) -> Option<TokenUsage> {
        let u = self.usage.as_ref()?;
        let prompt_tokens = u.prompt_tokens?;
        let completion_tokens = u.completion_tokens.unwrap_or(0);
        let total_tokens = u.total_tokens.unwrap_or(prompt_tokens + completion_tokens);
        Some(TokenUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens,
        })
    }

    /// The first choice's server-reported termination reason, when present.
    pub fn finish_reason(&self) -> Option<&str> {
        self.choices
            .first()
            .and_then(|c| c.finish_reason.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(top_k: u32, schema: Option<serde_json::Value>) -> TurnRequest {
        TurnRequest {
            generation_id: story_model::uuid::Uuid::nil(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: "hi".to_string(),
            }],
            model: "m".to_string(),
            temperature: 0.5,
            top_p: 0.9,
            top_k,
            max_tokens: 100,
            json_schema: schema,
        }
    }

    #[test]
    fn max_tokens_is_the_default_wire_key() {
        let req = turn(0, None);
        let env = ChatCompletionRequest::from_turn(&req, "m", false);
        let json = serde_json::to_string(&env).expect("serialize");
        assert!(
            json.contains("\"max_tokens\":100"),
            "max_tokens must be the default wire key: {json}"
        );
        assert!(
            !json.contains("max_completion_tokens"),
            "max_completion_tokens must be absent by default: {json}"
        );
    }

    #[test]
    fn max_completion_tokens_wire_key_when_selected() {
        let req = turn(0, None);
        let mut env = ChatCompletionRequest::from_turn(&req, "m", false);
        env.max_tokens_field = MaxTokensField::MaxCompletionTokens;
        let json = serde_json::to_string(&env).expect("serialize");
        assert!(
            json.contains("\"max_completion_tokens\":100"),
            "selected field must serialize under max_completion_tokens: {json}"
        );
        assert!(
            !json.contains("\"max_tokens\""),
            "legacy max_tokens key must be absent when MaxCompletionTokens is selected: {json}"
        );
    }

    #[test]
    fn top_k_omitted_when_zero() {
        let req = turn(0, None);
        let env = ChatCompletionRequest::from_turn(&req, "m", false);
        let json = serde_json::to_string(&env).expect("serialize");
        assert!(
            !json.contains("\"top_k\""),
            "top_k must be absent when 0: {json}"
        );
    }

    #[test]
    fn top_k_present_when_nonzero() {
        let req = turn(40, None);
        let env = ChatCompletionRequest::from_turn(&req, "m", false);
        let json = serde_json::to_string(&env).expect("serialize");
        assert!(
            json.contains("\"top_k\":40"),
            "top_k must be present when >0: {json}"
        );
    }

    #[test]
    fn response_format_omitted_when_not_supported() {
        let req = turn(40, Some(serde_json::json!({"type": "object"})));
        let env = ChatCompletionRequest::from_turn(&req, "m", false);
        let json = serde_json::to_string(&env).expect("serialize");
        assert!(
            !json.contains("response_format"),
            "response_format must be absent when not supported: {json}"
        );
    }

    #[test]
    fn response_format_present_when_supported_with_schema() {
        let req = turn(40, Some(serde_json::json!({"type": "object"})));
        let env = ChatCompletionRequest::from_turn(&req, "m", true);
        let json = serde_json::to_string(&env).expect("serialize");
        assert!(
            json.contains("\"response_format\""),
            "response_format must be present: {json}"
        );
        assert!(
            json.contains("\"json_schema\""),
            "json_schema block must be present: {json}"
        );
        assert!(
            json.contains("\"strict\":true"),
            "strict must be true: {json}"
        );
        // The OpenAI discriminator uses "type", not "kind".
        assert!(
            json.contains("\"type\":\"json_schema\""),
            "discriminator must be type=json_schema: {json}"
        );
        assert!(json.contains("\"stream\":false"));
    }

    #[test]
    fn response_format_suppressed_when_supported_but_no_schema() {
        // Capability on, but the request carries no schema -> omit the block.
        let req = turn(40, None);
        let env = ChatCompletionRequest::from_turn(&req, "m", true);
        let json = serde_json::to_string(&env).expect("serialize");
        assert!(
            !json.contains("response_format"),
            "no response_format without a schema: {json}"
        );
    }

    #[test]
    fn response_tolerates_unknown_fields_and_extracts_content() {
        // Server includes extra top-level keys + a usage block; serde must
        // ignore the unknowns.
        let body = r#"{
            "id": "chatcmpl-x",
            "object": "chat.completion",
            "created": 1,
            "model": "mlx",
            "system_fingerprint": "fp",
            "choices": [
                {"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}
            ],
            "usage": {"prompt_tokens": 12, "completion_tokens": 3, "total_tokens": 15}
        }"#;
        let parsed: ChatCompletionResponse = serde_json::from_str(body).expect("decode");
        assert_eq!(parsed.content(), "hello");
        let usage = parsed.usage().expect("usage");
        assert_eq!(usage.prompt_tokens, 12);
        assert_eq!(usage.completion_tokens, 3);
        assert_eq!(usage.total_tokens, 15);
    }

    #[test]
    fn response_derives_total_tokens_when_server_omits_it() {
        let body = r#"{
            "choices": [{"message":{"content":"x"}}],
            "usage": {"prompt_tokens": 7, "completion_tokens": 4}
        }"#;
        let parsed: ChatCompletionResponse = serde_json::from_str(body).expect("decode");
        let usage = parsed.usage().expect("usage");
        assert_eq!(usage.total_tokens, 11);
    }

    #[test]
    fn response_empty_choices_yield_empty_content_and_no_usage() {
        let body = r#"{"choices": []}"#;
        let parsed: ChatCompletionResponse = serde_json::from_str(body).expect("decode");
        assert_eq!(parsed.content(), "");
        assert!(parsed.usage().is_none());
    }
}
