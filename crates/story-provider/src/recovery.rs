//! Strict `GeneratedTurn` JSON parser with limited, single-pass recovery
//! (ST-019).
//!
//! The model emits a JSON object matching the [`RawGeneratedTurn`] shape. This
//! module parses it strictly (unknown fields are REJECTED, never silently
//! dropped, via `deny_unknown_fields` on both the outer DTO and the embedded
//! [`StatePatch`]) and applies at most ONE repair pass:
//!
//! 1. Optionally unwrap a stringified whole-body (the model double-encoded the
//!    JSON as an escaped string literal).
//! 2. Locate the JSON object via the first `{` .. last `}`, which strips
//!    markdown code fences and surrounding prose.
//! 3. Strict parse. On failure, run ONE repair pass (string-aware trailing
//!    comma removal) and parse a second time. If that also fails, return
//!    [`ProviderError::InvalidGeneration`] with a SANITIZED detail (the raw
//!    body is never embedded in the error message).
//!
//! Then validate (choice count 1..=6, non-empty narration) and map into a
//! [`GeneratedTurnDraft`], stamping the request's `generation_id`.

use story_model::serde_json::Value;
use story_model::uuid::Uuid;
use story_model::{Choice, GeneratedTurnDraft, ProviderError, StatePatch, TokenUsage, TurnSummary};

use ProviderError::InvalidGeneration;

/// The result of a successful parse: the draft plus a list of human-readable
/// repair descriptions (empty when the input was already well-formed).
#[derive(Clone, Debug, PartialEq)]
pub struct ParseOutcome {
    /// The parsed draft, with `generation_id` stamped from the request.
    pub draft: GeneratedTurnDraft,
    /// Repairs applied, in order. Empty when no recovery was needed.
    pub repairs: Vec<String>,
}

/// Wire shape the model emits. `deny_unknown_fields` so an unknown top-level
/// key is a hard error (mirrors [`StatePatch`]'s own policy).
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGeneratedTurn {
    narration: String,
    choices: Vec<Choice>,
    state_patch: StatePatch,
    summary: TurnSummary,
    #[serde(default)]
    usage: Option<TokenUsage>,
}

/// Parse a raw model response into a validated [`GeneratedTurnDraft`], applying
/// limited recovery. See the module docs for the full pipeline.
pub fn parse_generated_turn(raw: &str, generation_id: Uuid) -> Result<ParseOutcome, ProviderError> {
    let mut repairs: Vec<String> = Vec::new();

    // (1) Unwrap a stringified whole-body double-encoding, if present.
    let work = {
        let lead = raw.trim_start();
        if lead.starts_with('"') {
            match serde_json::from_str::<String>(lead) {
                Ok(inner) => {
                    repairs.push("unwrapped stringified JSON body".to_string());
                    inner
                }
                Err(_) => raw.to_string(),
            }
        } else {
            raw.to_string()
        }
    };

    // (2) Locate the JSON object: first '{' .. last '}'. Strips markdown fences
    //     and leading/trailing prose. Records a repair only if it trimmed
    //     something.
    let trimmed = work.trim();
    let located =
        locate_json(trimmed).ok_or_else(|| InvalidGeneration(detail("no JSON object found")))?;
    if located != trimmed {
        repairs.push("located JSON object (stripped prose/markdown fence)".to_string());
    }

    // (3) First strict parse. Migrate only the known Director-era top-level
    // alias; all other unknown fields remain hard errors.
    let aliased = normalize_common_wire_aliases(&rename_legacy_patches(located));
    if aliased != located {
        repairs.push("renamed legacy top-level patches to state_patch".to_string());
    }
    let parsed = match serde_json::from_str::<RawGeneratedTurn>(&aliased) {
        Ok(p) => p,
        Err(first_err) => {
            // (4) ONE repair pass: repair a common local-model JSON mistake
            // (bare dialogue quotes inside a JSON string), then remove
            // trailing commas. Both operations are string-aware.
            let quote_repaired = escape_unescaped_quotes(&aliased);
            if quote_repaired != aliased {
                repairs.push("escaped bare quotes inside JSON strings".to_string());
            }
            let repaired = strip_trailing_commas(&quote_repaired);
            if repaired != quote_repaired {
                repairs.push("stripped trailing commas".to_string());
            }
            let repaired = normalize_common_wire_aliases(&rename_legacy_patches(&repaired));
            match serde_json::from_str::<RawGeneratedTurn>(&repaired) {
                Ok(p) => p,
                Err(second_err) => {
                    // Sanitized detail: serde errors are position/description
                    // based and do not echo the raw body; we cap the length as
                    // a defensive measure. The raw response is never embedded.
                    let _ = first_err;
                    return Err(InvalidGeneration(detail(&second_err.to_string())));
                }
            }
        }
    };

    // (5) Validate.
    let n = parsed.choices.len();
    if !(1..=6).contains(&n) {
        return Err(InvalidGeneration(detail(&format!(
            "choice count out of range [1, 6]: got {n}"
        ))));
    }
    let unique_labels = parsed
        .choices
        .iter()
        .map(|choice| {
            choice
                .label
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<std::collections::BTreeSet<_>>();
    if unique_labels.len() != n {
        return Err(InvalidGeneration(
            "duplicate choice labels; choices must offer different directions".to_string(),
        ));
    }
    if parsed.narration.trim().is_empty() {
        return Err(InvalidGeneration(detail("narration must be non-empty")));
    }

    // (6) Map to draft, stamping the request generation_id (the model's echo
    //     is never trusted).
    let draft = GeneratedTurnDraft {
        generation_id,
        narration: parsed.narration,
        choices: parsed.choices,
        proposed_patch: parsed.state_patch,
        summary: parsed.summary,
        usage: parsed.usage,
    };
    Ok(ParseOutcome { draft, repairs })
}

/// Rename one known legacy top-level field without weakening strict parsing.
/// Objects containing both names are intentionally left untouched and reject.
fn rename_legacy_patches(input: &str) -> String {
    let Ok(Value::Object(mut object)) = serde_json::from_str(input) else {
        return input.to_string();
    };
    if object.contains_key("state_patch") || !object.contains_key("patches") {
        return input.to_string();
    }
    let patches = object.remove("patches").expect("checked above");
    object.insert("state_patch".to_string(), patches);
    serde_json::to_string(&Value::Object(object)).unwrap_or_else(|_| input.to_string())
}

/// Normalize a small set of unambiguous Korean enum labels emitted by local
/// models. Missing optional patch/summary fields are filled with their empty
/// defaults; unknown fields still fail strict parsing and engine validation.
fn normalize_common_wire_aliases(input: &str) -> String {
    let Ok(mut root) = serde_json::from_str::<Value>(input) else {
        return input.to_string();
    };
    let original = root.clone();
    let Value::Object(object) = &mut root else {
        return input.to_string();
    };
    if let Some(Value::Array(choices)) = object.get_mut("choices") {
        for (index, choice) in choices.iter_mut().enumerate() {
            if let Value::Object(choice) = choice {
                // Qwen-family models commonly return the older
                // `{text, on_select}` choice shape. `on_select` can carry an
                // arbitrary per-choice patch, which is never part of this
                // engine's contract, so it is deliberately discarded. The
                // trusted, validated top-level `state_patch` remains the only
                // source of state mutation.
                let legacy_text = choice
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if let Some(text) = legacy_text {
                    choice
                        .entry("label".to_string())
                        .or_insert_with(|| Value::String(text.clone()));
                    choice
                        .entry("intent".to_string())
                        .or_insert_with(|| Value::String(text));
                    choice.remove("text");
                    choice.remove("on_select");
                }
                choice
                    .entry("id".to_string())
                    .or_insert_with(|| Value::String(format!("choice-{}", index + 1)));
                choice.entry("tone".to_string()).or_insert(Value::Null);
                choice.entry("target".to_string()).or_insert(Value::Null);
                choice
                    .entry("risk".to_string())
                    .or_insert_with(|| Value::String("low".to_string()));
                choice
                    .entry("hidden_requirements".to_string())
                    .or_insert_with(|| Value::Array(Vec::new()));
                choice.entry("preview".to_string()).or_insert(Value::Null);

                if let Some(Value::String(risk)) = choice.get_mut("risk") {
                    *risk = match risk.as_str() {
                        "안전" => "safe",
                        "낮음" => "low",
                        "보통" => "medium",
                        "높음" => "high",
                        "심각" => "severe",
                        _ => risk,
                    }
                    .to_string();
                }
            }
        }
    }
    if let Some(Value::Object(patch)) = object.get_mut("state_patch") {
        for key in [
            "affection_delta",
            "trust_delta",
            "tension_delta",
            "relationship_transitions",
            "set_flags",
            "number_delta",
            "set_strings",
            "inventory_delta",
        ] {
            patch
                .entry(key.to_string())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
        }
        patch
            .entry("add_events".to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        for key in ["next_location", "next_time_slot", "proposed_route"] {
            patch.entry(key.to_string()).or_insert(Value::Null);
        }
        if let Some(Value::String(slot)) = patch.get_mut("next_time_slot") {
            *slot = match slot.as_str() {
                "새벽" => "dawn",
                "오전" => "morning",
                "오후" => "afternoon",
                "저녁" => "evening",
                "밤" => "night",
                _ => slot,
            }
            .to_string();
        }
        if let Some(Value::Object(transitions)) = patch.get_mut("relationship_transitions") {
            transitions.retain(|_, value| {
                let Value::String(stage) = value else {
                    return false;
                };
                let normalized = match stage.as_str() {
                    "낯선 사이" => Some("stranger"),
                    "아는 사이" => Some("acquaintance"),
                    "친구" => Some("friend"),
                    "가까운 친구" => Some("close_friend"),
                    "연애 관심" => Some("romantic_interest"),
                    "연인" => Some("partner"),
                    "소원한 사이" => Some("estranged"),
                    "stranger" | "acquaintance" | "friend" | "close_friend"
                    | "romantic_interest" | "partner" | "estranged" => Some(stage.as_str()),
                    _ => None,
                };
                if let Some(normalized) = normalized {
                    *stage = normalized.to_string();
                    true
                } else {
                    false
                }
            });
        }
    }
    if let Some(Value::Object(summary)) = object.get_mut("summary") {
        for key in [
            "facts",
            "unresolved",
            "relationship_changes",
            "knowledge_changes",
            "address_form_changes",
            "important_player_actions",
            "source_history_ids",
        ] {
            summary
                .entry(key.to_string())
                .or_insert_with(|| Value::Array(Vec::new()));
        }
        summary
            .entry("interpretation".to_string())
            .or_insert(Value::Null);
        if let Some(Value::Array(items)) = summary.get_mut("interpretation") {
            let text = items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" ");
            *summary.get_mut("interpretation").expect("checked above") = Value::String(text);
        }
    }
    if root == original {
        input.to_string()
    } else {
        serde_json::to_string(&root).unwrap_or_else(|_| input.to_string())
    }
}

/// Locate the JSON object in `s` as the slice from the first `{` to the last
/// `}`. Returns `None` when no object can be located.
fn locate_json(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end < start {
        return None;
    }
    Some(&s[start..=end])
}

/// Escape bare `"` characters a model placed inside a JSON string, such as
/// `"narration":"그가 "안녕"이라고 말했다"`. A quote closes a string only
/// when the subsequent JSON punctuation makes that unambiguous; otherwise it
/// is preserved as a literal quote. This is deliberately syntax-only: it does
/// not add, remove, or reinterpret any state fields.
fn escape_unescaped_quotes(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut in_string = false;
    let mut escaped = false;

    for (index, ch) in chars.iter().copied().enumerate() {
        if !in_string {
            if ch == '"' {
                in_string = true;
            }
            out.push(ch);
            continue;
        }
        if escaped {
            out.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            out.push(ch);
            escaped = true;
            continue;
        }
        if ch != '"' {
            out.push(ch);
            continue;
        }

        if is_json_string_end(&chars[index + 1..]) {
            out.push(ch);
            in_string = false;
        } else {
            out.push('\\');
            out.push(ch);
        }
    }
    out
}

fn is_json_string_end(rest: &[char]) -> bool {
    let Some((first_index, first)) = rest.iter().enumerate().find(|(_, ch)| !ch.is_whitespace())
    else {
        return true;
    };
    match first {
        ':' | '}' | ']' => true,
        ',' => rest[first_index + 1..]
            .iter()
            .find(|ch| !ch.is_whitespace())
            .is_none_or(|next| matches!(next, '"' | '}' | ']')),
        _ => false,
    }
}

/// Remove commas that immediately precede a closing `}` or `]` (allowing
/// whitespace between), while leaving commas inside string literals untouched.
/// Used for the single repair pass only.
fn strip_trailing_commas(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            escaped = false;
            out.push(c);
            i += 1;
            continue;
        }
        if c == ',' {
            // Look ahead past whitespace; drop the comma if a close follows.
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && (chars[j] == '}' || chars[j] == ']') {
                i += 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Sanitize an error detail: cap its length (char-based, so UTF-8 safe) so the
/// error surface stays small. The raw response body is never embedded here.
fn detail(msg: &str) -> String {
    const MAX: usize = 200;
    let count = msg.chars().count();
    if count <= MAX {
        return msg.to_string();
    }
    let truncated: String = msg.chars().take(MAX).collect();
    format!("{truncated}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_json_finds_outermost_object() {
        assert_eq!(locate_json("prose {\"a\":1} tail"), Some("{\"a\":1}"));
        assert_eq!(locate_json("no braces"), None);
        assert_eq!(locate_json("} before {"), None);
    }

    #[test]
    fn strip_trailing_commas_leaves_strings_alone() {
        let input = r#"{"a":"hello, world","b":1,}"#;
        let out = strip_trailing_commas(input);
        // The comma inside the string literal is preserved; the trailing comma
        // before the closing brace is removed.
        assert!(
            out.contains("hello, world"),
            "string comma must remain: {out}"
        );
        assert!(
            !out.ends_with(",}"),
            "trailing comma must be removed: {out}"
        );
    }

    #[test]
    fn detail_truncates_long_utf8_without_panicking() {
        let long = "비".repeat(500);
        let d = detail(&long);
        assert!(d.ends_with('…'));
    }

    #[test]
    fn detail_passes_short_messages_through() {
        assert_eq!(detail("ok"), "ok");
    }
}
