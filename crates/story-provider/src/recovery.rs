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

    // (3) First strict parse.
    let parsed = match serde_json::from_str::<RawGeneratedTurn>(located) {
        Ok(p) => p,
        Err(first_err) => {
            // (4) ONE repair pass: string-aware trailing-comma removal.
            let repaired = strip_trailing_commas(located);
            if repaired != located {
                repairs.push("stripped trailing commas".to_string());
            }
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
