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
use story_model::{Choice, DirectorPlan, GeneratedTurnDraft, ProviderError, StatePatch, TokenUsage, TurnSummary};

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

    // (1b) Strip hybrid-reasoning chain-of-thought that local LLMs (Qwen3,
    //      DeepSeek-R1, …) prepend: `<think>…</think>` / `<thinking>…</thinking>`.
    //      These blocks routinely discuss the JSON shape and contain bare
    //      braces, which would otherwise corrupt [`locate_json`] (the first `{`
    //      would land inside the reasoning). Removed before extraction; the CoT
    //      is never parsed or stored. Trailing reasoning is handled later by the
    //      brace-balanced locator, which stops at the answer's closing `}`.
    let stripped = strip_reasoning(&work);
    if stripped != work {
        repairs.push("stripped reasoning/thinking block".to_string());
    }
    let work = stripped;

    // (2) Locate the JSON object: first '{' .. last '}'. Strips markdown fences
    //     and leading/trailing prose. Records a repair only if it trimmed
    //     something.
    let trimmed = work.trim();
    let located = locate_json(trimmed)
        .ok_or_else(|| InvalidGeneration(detail("json_extract: no JSON object found")))?;
    if located != trimmed {
        repairs.push("located JSON object (stripped prose/markdown fence)".to_string());
    }

    // (3) First strict parse. Migrate only the known Director-era top-level
    // alias; all other unknown fields remain hard errors.
    let renamed = rename_legacy_patches(located);
    if renamed != located {
        repairs.push("renamed legacy top-level patches to state_patch".to_string());
    }
    let aliased = normalize_common_wire_aliases(&renamed);
    if aliased != renamed {
        repairs.push("normalized compatible response fields".to_string());
    }
    let mut parsed = match serde_json::from_str::<RawGeneratedTurn>(&aliased) {
        Ok(p) => p,
        Err(first_err) => {
            let quote_repaired = escape_unescaped_quotes(&aliased);
            let ctrl_repaired = sanitize_json_control_characters(&quote_repaired);
            if quote_repaired != aliased || ctrl_repaired != quote_repaired {
                repairs.push("escaped bare quotes and raw control chars inside JSON strings".to_string());
            }
            let repaired = strip_trailing_commas(&ctrl_repaired);
            if repaired != ctrl_repaired {
                repairs.push("stripped trailing commas".to_string());
            }
            let repaired = normalize_common_wire_aliases(&rename_legacy_patches(&repaired));
            match serde_json::from_str::<RawGeneratedTurn>(&repaired) {
                Ok(p) => p,
                Err(second_err) => {
                    let _ = first_err;
                    let stage = if serde_json::from_str::<Value>(&repaired).is_ok() {
                        "schema"
                    } else {
                        "json_syntax"
                    };
                    return Err(InvalidGeneration(detail(&format!("{stage}: {second_err}"))));
                }
            }
        }
    };

    // Sanitize choice ids and intents.
    for (i, choice) in parsed.choices.iter_mut().enumerate() {
        if choice.id.trim().is_empty() {
            choice.id = format!("next-{i}");
        }
        if choice.intent.trim().is_empty() {
            choice.intent = choice.label.clone();
        }
    }

    // (5) Validate.
    let n = parsed.choices.len();
    if !(1..=6).contains(&n) {
        return Err(InvalidGeneration(detail(&format!(
            "validation: choice count out of range [1, 6]: got {n}"
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
            "validation: duplicate choice labels; choices must offer different directions"
                .to_string(),
        ));
    }
    if parsed.narration.trim().is_empty() {
        return Err(InvalidGeneration(detail(
            "validation: narration must be non-empty",
        )));
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

/// Strict Director plan JSON parser with resilience and single-pass recovery.
///
/// Strips CoT `<think>` blocks, extracts JSON objects from surrounding prose/markdown,
/// unwraps double-encoded string literals, and repairs trailing commas before parsing
/// into a [`DirectorPlan`].
pub fn parse_director_plan(raw: &str) -> Result<DirectorPlan, ProviderError> {
    let work = {
        let lead = raw.trim_start();
        if lead.starts_with('"') {
            match serde_json::from_str::<String>(lead) {
                Ok(inner) => inner,
                Err(_) => raw.to_string(),
            }
        } else {
            raw.to_string()
        }
    };

    let stripped = strip_reasoning(&work);
    let trimmed = stripped.trim();
    let located = locate_json(trimmed)
        .ok_or_else(|| InvalidGeneration(detail("json_extract: no JSON object found in director plan")))?;

    let aliased = normalize_director_aliases(located);
    let normalized = normalize_common_wire_aliases(&aliased);

    let mut plan = match serde_json::from_str::<DirectorPlan>(&normalized) {
        Ok(plan) => plan,
        Err(_) => {
            let quote_repaired = escape_unescaped_quotes(&normalized);
            let ctrl_repaired = sanitize_json_control_characters(&quote_repaired);
            let repaired = strip_trailing_commas(&ctrl_repaired);
            let repaired_aliased = normalize_director_aliases(&repaired);
            let repaired_normalized = normalize_common_wire_aliases(&repaired_aliased);
            serde_json::from_str::<DirectorPlan>(&repaired_normalized).map_err(|e| {
                let stage = if serde_json::from_str::<Value>(&repaired_normalized).is_ok() {
                    "schema"
                } else {
                    "json_syntax"
                };
                InvalidGeneration(detail(&format!("director plan {stage}: {e}")))
            })?
        }
    };

    if plan.choices.is_empty() {
        plan.choices = vec![
            Choice {
                id: "choice-1".to_string(),
                label: "솔직하게 대화를 이어간다".to_string(),
                intent: "솔직하게 대화를 이어간다".to_string(),
                tone: None,
                target: None,
                risk: story_model::RiskLevel::Safe,
                hidden_requirements: Vec::new(),
                preview: None,
            },
            Choice {
                id: "choice-2".to_string(),
                label: "상대방의 표정을 살피며 거리를 둔다".to_string(),
                intent: "상대방의 표정을 살피며 거리를 둔다".to_string(),
                tone: None,
                target: None,
                risk: story_model::RiskLevel::Low,
                hidden_requirements: Vec::new(),
                preview: None,
            },
            Choice {
                id: "choice-3".to_string(),
                label: "대범하게 도발적인 질문을 던진다".to_string(),
                intent: "대범하게 도발적인 질문을 던진다".to_string(),
                tone: None,
                target: None,
                risk: story_model::RiskLevel::Medium,
                hidden_requirements: Vec::new(),
                preview: None,
            },
        ];
    }

    sanitize_choices(&mut plan.choices);
    Ok(plan)
}

/// Sanitize raw control characters (unescaped newlines/tabs) inside JSON string values.
pub fn sanitize_json_control_characters(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_string = false;
    let mut escaped = false;

    for ch in input.chars() {
        if in_string {
            if escaped {
                escaped = false;
                out.push(ch);
            } else if ch == '\\' {
                escaped = true;
                out.push(ch);
            } else if ch == '"' {
                in_string = false;
                out.push(ch);
            } else if ch == '\n' {
                out.push_str("\\n");
            } else if ch == '\r' {
                out.push_str("\\r");
            } else if ch == '\t' {
                out.push_str("\\t");
            } else {
                out.push(ch);
            }
        } else {
            if ch == '"' {
                in_string = true;
            }
            out.push(ch);
        }
    }
    out
}

/// Auto-fill missing choice ids/intents and deduplicate duplicate choice labels.
pub fn sanitize_choices(choices: &mut [Choice]) {
    let mut seen_labels = std::collections::BTreeSet::new();
    for (i, choice) in choices.iter_mut().enumerate() {
        if choice.id.trim().is_empty() {
            choice.id = format!("next-{i}");
        }
        if choice.label.trim().is_empty() {
            choice.label = format!("선택지 {}", i + 1);
        }
        if choice.intent.trim().is_empty() {
            choice.intent = choice.label.clone();
        }

        let normalized = choice
            .label
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if seen_labels.contains(&normalized) {
            choice.label = format!("{} ({})", choice.label.trim(), i + 1);
            if choice.intent == normalized {
                choice.intent = choice.label.clone();
            }
        }
        seen_labels.insert(
            choice
                .label
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
}

fn normalize_director_aliases(input: &str) -> String {
    let Ok(Value::Object(mut map)) = serde_json::from_str::<Value>(input) else {
        return input.to_string();
    };

    if !map.contains_key("scene_goal") && map.contains_key("goal") {
        if let Some(v) = map.remove("goal") {
            map.insert("scene_goal".to_string(), v);
        }
    }
    if !map.contains_key("proposed_patch") {
        if let Some(v) = map
            .remove("proposed_state_patch")
            .or_else(|| map.remove("state_patch"))
            .or_else(|| map.remove("patch"))
        {
            map.insert("proposed_patch".to_string(), v);
        }
    }
    if !map.contains_key("choices") {
        if let Some(v) = map.remove("options").or_else(|| map.remove("choice_list")) {
            map.insert("choices".to_string(), v);
        }
    }

    serde_json::to_string(&Value::Object(map)).unwrap_or_else(|_| input.to_string())
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
        // History provenance helps later recall, but it cannot affect the
        // current scene. Local models sometimes emit aliases such as
        // `turn-1`; remove those instead of rejecting otherwise valid prose
        // and state proposals because the DTO stores only UUIDs.
        if let Some(Value::Array(ids)) = summary.get_mut("source_history_ids") {
            ids.retain(|id| id.as_str().is_some_and(|id| Uuid::parse_str(id).is_ok()));
        }
    }
    if root == original {
        input.to_string()
    } else {
        serde_json::to_string(&root).unwrap_or_else(|_| input.to_string())
    }
}

/// Remove a leading hybrid-reasoning block (`<think>…</think>` /
/// `<thinking>…</thinking>`, case-insensitive, attrs tolerated) that some local
/// LLMs emit before the answer. Multiple leading blocks are stripped in turn.
/// A leading block with no closer means the model hit the token cap mid-thought
/// and never produced JSON — the remainder is dropped so its CoT prose (which
/// often contains bare braces) never reaches [`locate_json`]. Trailing reasoning
/// is intentionally NOT stripped here: the brace-balanced locator stops at the
/// answer's closing brace and ignores anything after it.
pub fn strip_reasoning(input: &str) -> String {
    let mut work = input.to_string();
    loop {
        let trimmed = work.trim_start();
        let lower = trimmed.to_ascii_lowercase();
        if !lower.starts_with("<think") {
            return work;
        }
        // Find the FIRST matching closer (the block is leading).
        let close = ["</thinking>", "</think>"]
            .iter()
            .filter_map(|tag| lower.find(tag))
            .min();
        match close {
            Some(start) => {
                let len = ["</thinking>", "</think>"]
                    .iter()
                    .find_map(|tag| lower[start..].starts_with(tag).then_some(tag.len()))
                    .unwrap_or(0);
                work = trimmed[start + len..].to_string();
                // Loop: a second leading block may follow before the JSON.
            }
            None => {
                // Truncated CoT, no JSON follows.
                return String::new();
            }
        }
    }
}

/// Locate the first balanced JSON object in `s`, starting at the first `{`.
/// Scans brace/bracket depth with string-literal awareness and stops at the
/// matching `}`, so braces inside trailing prose or surviving reasoning text
/// cannot extend the slice past the real object. Returns `None` when no
/// balanced object is found (truncated body, top-level array, or no object).
fn locate_json(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let mut depth: usize = 0;
    let mut in_string = false;
    let mut escaped = false;
    for (idx, ch) in s.char_indices() {
        if idx < start {
            continue;
        }
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' | '[' => depth = depth.saturating_add(1),
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(&s[start..idx + ch.len_utf8()]);
                }
            }
            ']' => {
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
    }
    // No balanced object — typically a truncated body missing its final `}`.
    // Fall back to the outermost brace span so serde reports a `json_syntax`
    // error (a real truncation) rather than a generic "no object found". This
    // only fires when a `}` actually follows the opening `{`.
    let end = s.rfind('}')?;
    (end >= start).then(|| &s[start..=end])
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
