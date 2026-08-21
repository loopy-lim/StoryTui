//! Writer finalize + lint pass (ST-034, W10 Part B).
//!
//! [`finalize_writer_output`] is the ONLY entry point that converts the
//! Writer's streamed narration text into a [`WriterResult`] safe to commit to
//! history. It enforces the load-bearing ADR-005 invariant:
//!
//! - The Writer has NO authority. Its text becomes `narration` ONLY — never a
//!   patch, never choices, never routes/endings. Those come exclusively from
//!   the confirmed [`crate::director::ValidatedPlan`].
//! - Embedded fenced JSON blocks in the Writer text are lint-flagged and
//!   STRIPPED — they are NEVER applied. A misbehaving Writer that tries to
//!   smuggle a patch or choice list through prose cannot mutate state.
//! - Partial / unconfirmed stream text never reaches this function (the
//!   provider only returns a [`WriterFinish`] on `[DONE]`); and the result of
//!   this function is the only text eligible for history/save.
//!
//! The lint pass is intentionally light (MVP): it warns on ANY fenced JSON
//! block. Non-JSON fenced blocks (e.g. a legitimate code sample in prose) are
//! preserved untouched.

use story_model::WriterLintIssue;
use story_model::uuid::Uuid;

use crate::director::ValidatedPlan;
use crate::error::CoreError;

/// The sanitized output of [`finalize_writer_output`].
///
/// `narration` is the only text eligible for the engine's history append and
/// the save payload; `lint` carries non-fatal issues for diagnostics. The
/// confirmed Director plan (patch + choices) is NOT repeated here — the engine
/// reads it from the [`ValidatedPlan`] it already holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriterResult {
    /// Cleaned narration prose. Embedded fenced JSON blocks have been stripped.
    pub narration: String,
    /// Non-fatal lint issues (e.g. an embedded-patch attempt was stripped).
    pub lint: Vec<WriterLintIssue>,
}

/// Lint issue code surfaced when the Writer tried to embed a JSON block
/// (smuggling a patch / choice list through prose). The block is stripped
/// before the narration reaches history.
pub const LINT_EMBEDDED_PATCH: &str = "embedded_patch_attempt";

/// Finalize the Writer's streamed text into a commit-safe [`WriterResult`].
///
/// `plan` is the confirmed Director plan (the engine already holds it; we take
/// it by reference to emphasize the Writer cannot mutate it). `generation_id`
/// is the stable id of this turn; it is carried for attestation / diagnostics
/// (a future step may stamp it into a lint detail).
///
/// Always returns `Ok` — the lint pass never rejects a turn (the Writer's
/// misbehavior is stripped, not punished). The [`CoreError`] return type is
/// retained for forward compatibility (e.g. an explicit allow-list of
/// forbidden substrings that would hard-reject).
pub fn finalize_writer_output(
    text: String,
    _plan: &ValidatedPlan,
    _generation_id: Uuid,
) -> Result<WriterResult, CoreError> {
    let (narration, lint) = strip_fenced_json_blocks(&text);
    Ok(WriterResult { narration, lint })
}

/// Strip fenced code blocks whose content (after the info-string, e.g.
/// ` ```json `) starts with `{` or `[`. Each stripped block appends one
/// [`WriterLintIssue`] with code [`LINT_EMBEDDED_PATCH`]. Non-JSON fenced
/// blocks are preserved verbatim.
///
/// Fence markers recognized: three-or-more backticks (` ``` `) or tildes
/// (`~~~`). The opener's info-string (e.g. `json`, `patch`) is ignored — we
/// decide by the first non-whitespace char of the block body. Line-wise scan
/// keeps the parser UTF-8-safe (Rust `&str` never splits a multi-byte char).
fn strip_fenced_json_blocks(text: &str) -> (String, Vec<WriterLintIssue>) {
    let mut out = String::with_capacity(text.len());
    let mut lint = Vec::new();
    // Collect lines first so the fence-body collector can advance the same
    // cursor without aliasing a `Peekable` (which trips
    // `clippy::while_let_on_iterator`).
    let all_lines: Vec<&str> = text.lines().collect();
    let mut idx = 0usize;
    let mut pending_newline = false;

    while let Some(&line) = all_lines.get(idx) {
        idx += 1;
        let trimmed = line.trim_start();
        if let Some(marker) = fence_opener(trimmed) {
            // Pull body lines until a matching closer; decide strip-vs-keep by
            // the first non-whitespace body char.
            let (body, closed) = collect_fenced_body(&all_lines, &mut idx, marker);
            if looks_like_json(&body) {
                lint.push(WriterLintIssue {
                    code: LINT_EMBEDDED_PATCH.to_string(),
                    detail: format!(
                        "embedded fenced JSON block ({} chars) stripped — Writer has no patch authority",
                        body.chars().count()
                    ),
                });
                // Drop opener + body (+ closer if present). Emit nothing.
            } else {
                // Preserve verbatim: opener, body, closer (if any).
                push_line(&mut out, &mut pending_newline, line);
                for body_line in body.split('\n') {
                    push_line(&mut out, &mut pending_newline, body_line);
                }
                if closed {
                    let fence: String = std::iter::repeat_n(marker.char, marker.width).collect();
                    push_line(&mut out, &mut pending_newline, &fence);
                }
            }
        } else {
            push_line(&mut out, &mut pending_newline, line);
        }
    }

    (out, lint)
}

/// Recognized fence opener: ` ``` ` or `~~~` (≥ 3 of the same marker char),
/// possibly preceded by indentation (already trimmed by the caller).
fn fence_opener(line: &str) -> Option<FenceMarker> {
    let first = line.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let width = line.chars().take_while(|&c| c == first).count();
    if width >= 3 {
        Some(FenceMarker { char: first, width })
    } else {
        None
    }
}

/// A matched fence opener.
#[derive(Clone, Copy)]
struct FenceMarker {
    char: char,
    width: usize,
}

/// Pull lines until a matching closing fence (same marker char, same-or-greater
/// width, only whitespace besides the marker) is found. Returns the body text
/// (lines joined with `\n`) and whether a closer was seen before end-of-input.
/// Advances `idx` past the consumed lines (including the closer, if any).
fn collect_fenced_body(lines: &[&str], idx: &mut usize, marker: FenceMarker) -> (String, bool) {
    let mut body = String::new();
    let mut first = true;
    while let Some(&line) = lines.get(*idx) {
        *idx += 1;
        let t = line.trim_start();
        let same_marker_run = t.chars().take_while(|&c| c == marker.char).count();
        let only_marker_and_ws = t.chars().all(|c| c == marker.char || c.is_whitespace());
        if same_marker_run >= marker.width && only_marker_and_ws && same_marker_run > 0 {
            return (body, true);
        }
        if !first {
            body.push('\n');
        }
        body.push_str(line);
        first = false;
    }
    (body, false)
}

/// `true` when the first non-whitespace char of `body` is `{` or `[` — the
/// cheap JSON-object / JSON-array detector that drives the lint.
fn looks_like_json(body: &str) -> bool {
    matches!(body.trim_start().chars().next(), Some('{') | Some('['))
}

/// Append `line` to `out`, inserting a `\n` separator between lines.
fn push_line(out: &mut String, pending_newline: &mut bool, line: &str) {
    if *pending_newline {
        out.push('\n');
    }
    out.push_str(line);
    *pending_newline = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::director::validate_plan;
    use crate::tests_support::sample_pack;
    use story_model::{
        CharacterState, DirectorPlan, Emotion, GameState, RelationshipStage, RiskLevel, StatePatch,
    };

    /// Build a minimal validated plan to satisfy the `&ValidatedPlan` parameter
    /// of [`finalize_writer_output`]. The content of the plan is irrelevant to
    /// the lint pass; we only need the type.
    fn validated_plan_fixture() -> ValidatedPlan {
        let mut chars = std::collections::BTreeMap::new();
        chars.insert(
            story_model::CharacterId("c-namu".to_string()),
            CharacterState {
                affection: 0,
                trust: 0,
                tension: 0,
                relationship_stage: RelationshipStage::Acquaintance,
                current_emotion: Emotion::Neutral,
                known_facts: Default::default(),
                address_form_overrides: Default::default(),
            },
        );
        let state = GameState {
            save_schema_version: story_model::SAVE_SCHEMA_VERSION,
            story_id: "demo".to_string(),
            story_version: semver::Version::new(0, 1, 0),
            pack_schema_version: semver::Version::new(1, 0, 0),
            session_id: uuid::Uuid::nil(),
            current_chapter: "chapter-1".to_string(),
            current_scene_id: "scene-1".to_string(),
            turn: 0,
            location: story_model::LocationId("loc-classroom".to_string()),
            time_slot: story_model::TimeSlot::Morning,
            active_route: None,
            characters: chars,
            flags: Default::default(),
            numbers: Default::default(),
            strings: Default::default(),
            inventory: Default::default(),
            occurred_events: Default::default(),
            recent_summaries: Default::default(),
            memories: Vec::new(),
            history: Vec::new(),
            last_choice: None,
            revision: 0,
            stable_generation_id: None,
        };
        let pack = sample_pack();
        let mut patch = StatePatch::default();
        patch
            .affection_delta
            .insert(story_model::CharacterId("c-namu".to_string()), 3);
        let plan = DirectorPlan {
            scene_goal: "s".to_string(),
            required_beats: vec!["b".to_string()],
            emotion_arcs: Default::default(),
            character_outfits: Default::default(),
            ambient_situation: None,
            reveal_fact_ids: vec!["f".to_string()],
            must_not_reveal_fact_ids: vec![],
            proposed_patch: patch,
            choices: vec![story_model::Choice {
                id: "c1".to_string(),
                label: "l".to_string(),
                intent: "i".to_string(),
                tone: None,
                target: None,
                risk: RiskLevel::Safe,
                hidden_requirements: vec![],
                preview: None,
            }],
            conflict_checks: vec![],
            end_condition: "cont".to_string(),
        };
        validate_plan(&plan, &state, &pack).expect("fixture plan validates")
    }

    #[test]
    fn plain_prose_passes_through_unchanged() {
        let plan = validated_plan_fixture();
        let text = "비가 그쳤다. 나무가 웃는다.".to_string();
        let result = finalize_writer_output(text.clone(), &plan, uuid::Uuid::nil()).expect("ok");
        assert!(result.lint.is_empty());
        assert_eq!(result.narration, text);
    }

    #[test]
    fn embedded_json_patch_is_stripped_and_flagged_not_applied() {
        let plan = validated_plan_fixture();
        // The Writer tries to smuggle a patch through prose. The finalize pass
        // MUST strip it and flag a lint; the engine never applies this patch.
        let text = "비가 그쳤다.\n```json\n{\"state_patch\":{\"affection_delta\":{\"c-namu\":99}}}\n```\n나무가 웃는다.".to_string();
        let result = finalize_writer_output(text, &plan, uuid::Uuid::nil()).expect("ok");
        assert_eq!(result.lint.len(), 1, "lint issues: {:?}", result.lint);
        assert_eq!(result.lint[0].code, LINT_EMBEDDED_PATCH);
        assert!(
            !result.narration.contains("affection_delta"),
            "smuggled patch must be stripped: {}",
            result.narration
        );
        assert!(
            !result.narration.contains("```"),
            "fence markers must be gone: {}",
            result.narration
        );
        assert!(result.narration.contains("비가 그쳤다."));
        assert!(result.narration.contains("나무가 웃는다."));
    }

    #[test]
    fn embedded_choice_list_array_is_stripped() {
        let plan = validated_plan_fixture();
        let text = "prose before\n```\n[{\"id\":\"c1\"}]\n```\nprose after".to_string();
        let result = finalize_writer_output(text, &plan, uuid::Uuid::nil()).expect("ok");
        assert_eq!(result.lint.len(), 1);
        assert!(
            !result.narration.contains("\"id\":\"c1\""),
            "smuggled choices stripped: {}",
            result.narration
        );
        assert!(result.narration.contains("prose before"));
        assert!(result.narration.contains("prose after"));
    }

    #[test]
    fn non_json_fenced_block_is_preserved() {
        let plan = validated_plan_fixture();
        let text = "코드 예시:\n```\nlet x = 1;\n```\n끝.".to_string();
        let result = finalize_writer_output(text, &plan, uuid::Uuid::nil()).expect("ok");
        assert!(
            result.lint.is_empty(),
            "non-JSON fenced block must not be flagged: {:?}",
            result.lint
        );
        assert!(result.narration.contains("let x = 1;"));
    }

    #[test]
    fn korean_multi_byte_chars_in_prose_are_preserved() {
        let plan = validated_plan_fixture();
        let text = "안녕하세요 — 한글 테스트.".to_string();
        let result = finalize_writer_output(text.clone(), &plan, uuid::Uuid::nil()).expect("ok");
        assert_eq!(result.narration, text);
    }

    #[test]
    fn result_does_not_carry_patch_or_choices() {
        // The WriterResult type itself cannot carry a patch/choices — only
        // narration + lint. If this compiles, the structural invariant holds.
        let plan = validated_plan_fixture();
        let result =
            finalize_writer_output("hi".to_string(), &plan, uuid::Uuid::nil()).expect("ok");
        let _ = (result.narration, result.lint);
    }
}
