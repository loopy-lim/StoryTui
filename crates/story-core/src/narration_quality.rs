//! Deterministic narration repetition and progression checks.
//!
//! This module is deliberately heuristic and dependency-free. It is not a
//! replacement for semantic evaluation; it is a last, engine-owned guard that
//! prevents a weak local model from committing an exact replay, a heavily
//! copied scene, or a second consecutive turn with no observable state/story
//! progress.

use std::collections::{BTreeMap, BTreeSet};

use story_model::{GameState, HistoryEntry, StatePatch, TurnSummary};

const HISTORY_WINDOW: usize = 4;
const MAX_MOTIFS: usize = 8;
const MIN_REUSED_MOTIFS: usize = 3;
const MIN_REUSED_MOTIF_FREQUENCY: usize = 3;
/// Question classes that are "same conversation loop" when repeated — not
/// story props (envelope/watch) which legitimately recur while advancing.
const LOOP_QUESTION_BEATS: &[&str] = &["why_contact", "why_left", "answer_trap"];
const MIN_DIALOGUE_FINGERPRINT_CHARS: usize = 20;

/// A narration defect that must be repaired before the turn can be committed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NarrationQualityIssue {
    /// The candidate is the same prose as a recent committed turn after
    /// whitespace/punctuation normalization.
    ExactDuplicate { turn: u32 },
    /// A long sentence or most character trigrams were copied from a recent
    /// committed turn.
    HighSimilarity { turn: u32 },
    /// A dialogue line (speaker-labeled or quoted) matches a recent turn's
    /// dialogue after fingerprinting, or is highly similar to one.
    RepeatedDialogue { turn: u32 },
    /// The candidate reuses a dramatic question *type* (beat id) that appeared
    /// in one of the last two history turns, even when the wording differs.
    RepeatedQuestionBeat { beat: String, turn: u32 },
    /// Several already-recurring content motifs were reused yet again.
    ReusedMotifs { motifs: Vec<String> },
    /// The previous and candidate turns both carry no meaningful patch or
    /// structured summary progress.
    StalledProgress,
    /// The bounded repair returned essentially the same rejected prose.
    UnchangedRepair,
    /// The candidate narration is empty or whitespace only — nothing a player
    /// can read. Checked before every other heuristic: an empty prose
    /// trivially passes the fingerprint/similarity scans below, so without
    /// this variant it would commit unconditionally (P1: the 2-stage Writer
    /// path has no provider-side empty guard).
    Empty,
}

/// Result of the pre-commit narration quality gate.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NarrationQualityReport {
    pub issues: Vec<NarrationQualityIssue>,
}

impl NarrationQualityReport {
    pub fn is_ok(&self) -> bool {
        self.issues.is_empty()
    }

    /// Issues that must still reject a turn after the allowed narration
    /// repair attempts (up to two).
    ///
    /// Hard (still block): empty prose, exact duplicates, near-copy repairs,
    /// high sentence similarity, repeated dialogue lines, and repeated
    /// question beats — these are the ones players notice as "같은 장면 반복".
    /// Soft (may commit after repair): motif reuse and stalled progress alone,
    /// which weak local models re-trigger forever on shared setting vocabulary
    /// without meaning the scene is identical.
    pub fn blocks_commit_after_repair(&self) -> bool {
        self.issues.iter().any(|issue| {
            matches!(
                issue,
                NarrationQualityIssue::Empty
                    | NarrationQualityIssue::ExactDuplicate { .. }
                    | NarrationQualityIssue::HighSimilarity { .. }
                    | NarrationQualityIssue::RepeatedDialogue { .. }
                    | NarrationQualityIssue::RepeatedQuestionBeat { .. }
                    | NarrationQualityIssue::UnchangedRepair
            )
        })
    }

    /// Concise repair instructions suitable for the provider prompt.
    pub fn repair_instructions(&self) -> String {
        let mut lines = Vec::new();
        for issue in &self.issues {
            match issue {
                NarrationQualityIssue::ExactDuplicate { turn } => lines.push(format!(
                    "턴 {turn}의 본문과 사실상 동일하다. 장면의 행동, 대화, 감각 묘사를 새로 쓴다."
                )),
                NarrationQualityIssue::HighSimilarity { turn } => lines.push(format!(
                    "턴 {turn}의 긴 문장이나 문장 구조를 과도하게 재사용했다. 같은 문장을 변형하지 말고 다른 사건 단위로 전진한다."
                )),
                NarrationQualityIssue::RepeatedDialogue { turn } => lines.push(format!(
                    "턴 {turn}의 대사와 같거나 거의 같다. NPC의 질문/발화를 새로 창작해 장면을 전진시켜라(새 정보, 결정, 또는 거절). 이전 대사를 가볍게 바꿔 쓰지 말고 극적인 요구 자체를 바꿔라."
                )),
                NarrationQualityIssue::RepeatedQuestionBeat { beat, turn } => lines.push(format!(
                    "턴 {turn}과 같은 질문 유형({beat})을 다시 하지 말고, 새 사실 공개·결정·거절·장면 전환 중 하나로 전진한다."
                )),
                NarrationQualityIssue::ReusedMotifs { motifs } => lines.push(format!(
                    "최근 반복 소재({})를 이번 장면의 중심 묘사로 다시 쓰지 않는다.",
                    motifs.join(", ")
                )),
                NarrationQualityIssue::StalledProgress => lines.push(
                    "직전 턴에 이어 변화가 없다. 기존 상태 패치를 바꾸지 않는 범위에서 새로운 관찰 가능한 행동, 정보, 압력 또는 질문을 본문에 발생시킨다."
                        .to_string(),
                ),
                NarrationQualityIssue::UnchangedRepair => lines.push(
                    "재작성 결과가 거절된 원문과 거의 같다. 문장 치환이 아니라 새로운 장면 전개로 완전히 다시 쓴다."
                        .to_string(),
                ),
                NarrationQualityIssue::Empty => lines.push(
                    "본문이 비어 있다. 플레이어에게 보여 줄 한국어 산문을 처음부터 새로 쓴다."
                        .to_string(),
                ),
            }
        }
        lines.join("\n- ")
    }
}

/// A repair must not be a punctuation-only edit or a near-copy of the
/// rejected candidate.
pub fn repair_is_distinct(rejected: &str, repaired: &str) -> bool {
    prose_fingerprint(rejected) != prose_fingerprint(repaired)
        && !highly_similar(repaired, rejected)
}

/// Evaluate a candidate before it reaches history/save.
///
/// `enforce_progress` is disabled for the one allowed narration repair: its
/// authoritative patch/summary are intentionally preserved, while the repair
/// itself must still pass all repetition checks.
pub fn assess_narration(
    game: &GameState,
    narration: &str,
    patch: &StatePatch,
    summary: Option<&TurnSummary>,
    enforce_progress: bool,
) -> NarrationQualityReport {
    let mut issues = Vec::new();
    // Empty prose gate first (see the `Empty` docs): the scans below skip
    // empty candidates, so this is the only check that ever fires for one.
    if narration.trim().is_empty() {
        issues.push(NarrationQualityIssue::Empty);
    }
    let recent_start = game.history.len().saturating_sub(HISTORY_WINDOW);
    let candidate_fingerprint = prose_fingerprint(narration);

    for entry in &game.history[recent_start..] {
        let previous_fingerprint = prose_fingerprint(&entry.narration);
        if !candidate_fingerprint.is_empty() && candidate_fingerprint == previous_fingerprint {
            issues.push(NarrationQualityIssue::ExactDuplicate { turn: entry.turn });
            break;
        }
        if highly_similar(narration, &entry.narration) {
            issues.push(NarrationQualityIssue::HighSimilarity { turn: entry.turn });
            break;
        }
    }

    let candidate_dialogue = dialogue_fingerprints(narration);
    if !candidate_dialogue.is_empty() {
        let mut offending_turn = None;
        for entry in game.history[recent_start..].iter().rev() {
            let previous_dialogue = dialogue_fingerprints(&entry.narration);
            if previous_dialogue.is_empty() {
                continue;
            }
            let hit = candidate_dialogue.iter().any(|candidate_line| {
                previous_dialogue.iter().any(|previous_line| {
                    candidate_line == previous_line
                        || highly_similar(candidate_line, previous_line)
                })
            });
            if hit {
                offending_turn = Some(entry.turn);
                break;
            }
        }
        if let Some(turn) = offending_turn {
            issues.push(NarrationQualityIssue::RepeatedDialogue { turn });
        }
    }

    // Only meta-loop question classes hard-block. Props like 봉투/시계 must be
    // allowed to develop across turns; banning any mention trapped the model on
    // object_tease while required_lead asked it to open the envelope.
    let candidate_beats = classify_question_beats(narration);
    if !candidate_beats.is_empty() {
        let recent_beat_start = game.history.len().saturating_sub(2);
        for beat in &candidate_beats {
            if !LOOP_QUESTION_BEATS.contains(&beat.as_str()) {
                continue;
            }
            let offending_turn = game.history[recent_beat_start..]
                .iter()
                .rev()
                .find_map(|entry| {
                    classify_question_beats(&entry.narration)
                        .into_iter()
                        .find(|previous| previous == beat)
                        .map(|_| entry.turn)
                });
            if let Some(turn) = offending_turn {
                issues.push(NarrationQualityIssue::RepeatedQuestionBeat {
                    beat: beat.clone(),
                    turn,
                });
                break;
            }
        }
    }

    let has_progress = patch_changes_game(game, patch) || summary.is_some_and(summary_has_progress);
    let motifs = reused_motifs(&game.history, narration);
    if !has_progress && motifs.len() >= MIN_REUSED_MOTIFS {
        issues.push(NarrationQualityIssue::ReusedMotifs { motifs });
    }

    if enforce_progress
        && !game.history.is_empty()
        && !has_progress
        && game
            .history
            .last()
            .is_some_and(|entry| !patch_carries_progress(&entry.applied_patch))
    {
        issues.push(NarrationQualityIssue::StalledProgress);
    }

    NarrationQualityReport { issues }
}

/// Fingerprint-normalized dialogue lines extracted from narration.
///
/// Speaker-labeled lines (`이름:` / `Name:`) and quoted spans are kept when
/// their fingerprint is at least [`MIN_DIALOGUE_FINGERPRINT_CHARS`] characters.
pub fn extract_dialogue_lines(text: &str) -> Vec<String> {
    dialogue_fingerprints(text)
}

/// Same as [`extract_dialogue_lines`]: fingerprint strings used for comparison.
pub fn dialogue_fingerprints(text: &str) -> Vec<String> {
    let mut fingerprints = Vec::new();
    let mut seen = BTreeSet::new();
    for span in raw_dialogue_spans(text) {
        let fingerprint = prose_fingerprint(&span);
        if fingerprint.chars().count() < MIN_DIALOGUE_FINGERPRINT_CHARS {
            continue;
        }
        if seen.insert(fingerprint.clone()) {
            fingerprints.push(fingerprint);
        }
    }
    fingerprints
}

/// Classify dramatic question *beats* in dialogue spans (not full prose).
///
/// Returns unique beat ids in first-seen order. Matching is substring-based
/// (no regex crate) over Korean dialogue text.
pub fn classify_question_beats(text: &str) -> Vec<String> {
    let mut beats = Vec::new();
    let mut seen = BTreeSet::new();
    for span in raw_dialogue_spans(text) {
        for beat in beats_in_span(&span) {
            if seen.insert(beat) {
                beats.push(beat.to_string());
            }
        }
    }
    beats
}

fn beats_in_span(span: &str) -> Vec<&'static str> {
    let compact: String = span.chars().filter(|c| !c.is_whitespace()).collect();
    let mut found = Vec::new();

    // why_contact — 왜.*(불렀|만나|연락|나타|지금)|오늘.*부른|만나자고
    if compact.contains("만나자고")
        || (compact.contains("오늘") && compact.contains("부른"))
        || contains_why_then(span, &["불렀", "만나", "연락", "나타", "지금"])
    {
        found.push("why_contact");
    }

    // why_left — 왜.*(떠났|사라졌)|떠난 이유 (bare 떠나 is too broad)
    if contains_why_then(span, &["떠났", "사라졌"]) || compact.contains("떠난이유") {
        found.push("why_left");
    }

    // answer_trap — 정해\s*두고|확인하고\s*싶|시험하
    if compact.contains("정해두고")
        || compact.contains("확인하고싶")
        || span.contains("시험하")
    {
        found.push("answer_trap");
    }

    // watch_symbol — 시계
    if span.contains("시계") {
        found.push("watch_symbol");
    }

    // object_tease — 봉투|메모|편지
    if span.contains("봉투") || span.contains("메모") || span.contains("편지") {
        found.push("object_tease");
    }

    // leave_or_walk — 끝낼|카페로|걷|여기서\s*끝내|돌아가
    if span.contains("끝낼")
        || span.contains("카페로")
        || span.contains("걷")
        || compact.contains("여기서끝내")
        || span.contains("돌아가")
    {
        found.push("leave_or_walk");
    }

    // self_meaning — 어떤\s*의미|지워|기억하
    if compact.contains("어떤의미") || span.contains("지워") || span.contains("기억하") {
        found.push("self_meaning");
    }

    found
}

fn contains_why_then(text: &str, keywords: &[&str]) -> bool {
    let Some(pos) = text.find('왜') else {
        return false;
    };
    let after = &text[pos..];
    keywords.iter().any(|keyword| after.contains(keyword))
}

fn raw_dialogue_spans(text: &str) -> Vec<String> {
    let mut spans = Vec::new();
    for line in text.lines() {
        if let Some(rest) = speaker_labeled_rest(line) {
            let trimmed = rest.trim();
            if let Some(inner) = unwrap_full_quotes(trimmed) {
                spans.push(inner);
            } else if !trimmed.is_empty() {
                spans.push(trimmed.to_string());
            }
        }
        spans.extend(quoted_spans(line));
    }
    spans
}

fn speaker_labeled_rest(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let colon = trimmed.find(':')?;
    let name = trimmed[..colon].trim();
    if name.is_empty() || !name.chars().all(char::is_alphabetic) {
        return None;
    }
    Some(&trimmed[colon + 1..])
}

fn unwrap_full_quotes(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() < 2 {
        return None;
    }
    let pairs = [('"', '"'), ('“', '”'), ('「', '」')];
    for &(open, close) in &pairs {
        if chars[0] == open && chars[chars.len() - 1] == close {
            return Some(chars[1..chars.len() - 1].iter().collect());
        }
    }
    None
}

fn quoted_spans(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let pairs = [('"', '"'), ('“', '”'), ('「', '」')];
    let mut spans = Vec::new();
    for &(open, close) in &pairs {
        let mut index = 0;
        while index < chars.len() {
            if chars[index] != open {
                index += 1;
                continue;
            }
            if let Some(end) = chars[index + 1..]
                .iter()
                .position(|&character| character == close)
            {
                let inner: String = chars[index + 1..index + 1 + end].iter().collect();
                if !inner.is_empty() {
                    spans.push(inner);
                }
                index = index + 1 + end + 1;
            } else {
                break;
            }
        }
    }
    spans
}

/// Repeated full-prose motifs used by prompt construction. Unlike the old
/// digest/first-clause scan, this inspects every narration paragraph.
pub fn recurring_motifs(history: &[HistoryEntry]) -> Vec<String> {
    motif_document_frequency(history)
        .into_iter()
        .filter(|(_, frequency)| *frequency >= 2)
        .take(MAX_MOTIFS)
        .map(|(motif, _)| motif)
        .collect()
}

fn reused_motifs(history: &[HistoryEntry], candidate: &str) -> Vec<String> {
    let candidate_tokens = content_tokens(candidate);
    motif_document_frequency(history)
        .into_iter()
        .filter(|(motif, frequency)| {
            *frequency >= MIN_REUSED_MOTIF_FREQUENCY && candidate_tokens.contains(motif)
        })
        .take(MAX_MOTIFS)
        .map(|(motif, _)| motif)
        .collect()
}

fn motif_document_frequency(history: &[HistoryEntry]) -> Vec<(String, usize)> {
    let start = history.len().saturating_sub(HISTORY_WINDOW);
    let mut frequencies: BTreeMap<String, usize> = BTreeMap::new();
    for entry in &history[start..] {
        for token in content_tokens(&entry.narration) {
            *frequencies.entry(token).or_default() += 1;
        }
    }
    let mut ranked: Vec<_> = frequencies.into_iter().collect();
    ranked.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    ranked
}

fn highly_similar(candidate: &str, previous: &str) -> bool {
    let candidate_chars = prose_fingerprint(candidate).chars().count();
    let candidate_sentences = sentence_fingerprints(candidate);
    let previous_sentences = sentence_fingerprints(previous);
    let mut matched_long_count = 0usize;
    let mut matched_long_chars = 0usize;
    for sentence in &candidate_sentences {
        let len = sentence.chars().count();
        if len >= 32 && previous_sentences.contains(sentence) {
            matched_long_count += 1;
            matched_long_chars += len;
        }
    }
    if matched_long_count >= 2
        || (matched_long_chars > 0
            && (matched_long_chars * 4 >= candidate_chars || matched_long_chars >= 64))
    {
        return true;
    }

    let candidate_grams = character_ngrams(candidate, 3);
    let previous_grams = character_ngrams(previous, 3);
    if candidate_grams.len().min(previous_grams.len()) < 50 {
        return false;
    }
    let shared = candidate_grams.intersection(&previous_grams).count();
    let union = candidate_grams.union(&previous_grams).count();
    union > 0 && shared * 100 >= union * 82
}

fn sentence_fingerprints(text: &str) -> BTreeSet<String> {
    text.split(['.', '。', '!', '?', '\n'])
        .map(prose_fingerprint)
        .filter(|sentence| !sentence.is_empty())
        .collect()
}

fn character_ngrams(text: &str, width: usize) -> BTreeSet<String> {
    let chars: Vec<char> = prose_fingerprint(text).chars().collect();
    if chars.len() < width {
        return BTreeSet::new();
    }
    chars
        .windows(width)
        .map(|window| window.iter().collect())
        .collect()
}

fn prose_fingerprint(text: &str) -> String {
    text.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn content_tokens(text: &str) -> BTreeSet<String> {
    text.split_whitespace()
        .filter_map(normalize_token)
        .collect()
}

const PARTICLES: &[&str] = &[
    "에서는",
    "으로는",
    "에게는",
    "까지는",
    "부터는",
    "들은",
    "에게",
    "에서",
    "으로",
    "처럼",
    "보다",
    "마저",
    "조차",
    "이가",
    "은",
    "는",
    "이",
    "가",
    "을",
    "를",
    "에",
    "의",
    "도",
    "와",
    "과",
    "로",
    "만",
];

const STOPWORDS: &[&str] = &[
    "그리고",
    "그러나",
    "하지만",
    "그래서",
    "그런데",
    "그러면",
    "그러자",
    "아니면",
    "아니라",
    "아닌",
    "다시",
    "조금",
    "조금씩",
    "잠시",
    "잠깐",
    "지금",
    "이미",
    "아직",
    "그대로",
    "여전히",
    "그냥",
    "자신",
    "서로",
    "정도",
    "사이",
    "사이로",
    "순간",
    "동안",
    "말",
    "말을",
    "말이",
    "대답",
    "대신",
    "목소리",
    "유나",
    "나무",
    "당신",
    "그녀",
    "그가",
    "그는",
    "그의",
    "나는",
    "내가",
    "했다",
    "있다",
    "있고",
    "있는",
    "있어",
    "있던",
    "있었다",
    "없다",
    "없고",
    "없는",
    "없어",
    "없던",
    "없었다",
    "한다",
    "하는",
    "하고",
    "하며",
    "된다",
    "됐다",
    "위해",
    "어떤",
    "어떻게",
    "어느",
    "무슨",
    "그런",
    "이런",
    "저런",
    "모든",
    "다른",
    "같은",
    "마치",
    "결국",
    "계속",
    "먼저",
    "바로",
    "사실",
    "혹시",
    "아마",
    "갑자기",
    "점점",
    "서서히",
    "살짝",
    "조용히",
    "천천히",
    "가만히",
    "건지",
    "건가",
    "것을",
    "것이",
    "것은",
    "것도",
    "거야",
    "거다",
    "앞에",
    "뒤로",
    "위로",
    "아래",
    "속에",
    "안에서",
    "밖으로",
    "그",
    "이",
    "저",
    "등",
    "및",
    "수",
    "것",
    "때",
    "임",
    "됨",
];

fn normalize_token(word: &str) -> Option<String> {
    let mut normalized = word
        .trim_matches(|character: char| !character.is_alphanumeric())
        .to_lowercase();
    for particle in PARTICLES {
        if let Some(stem) = normalized.strip_suffix(particle)
            && stem.chars().count() >= 2
        {
            normalized = stem.to_string();
            break;
        }
    }
    normalized = normalized
        .trim_matches(|character: char| !character.is_alphanumeric())
        .to_string();
    if normalized.chars().count() < 2 || STOPWORDS.contains(&normalized.as_str()) {
        return None;
    }
    Some(normalized)
}

fn patch_changes_game(game: &GameState, patch: &StatePatch) -> bool {
    patch.affection_delta.values().any(|delta| *delta != 0)
        || patch.trust_delta.values().any(|delta| *delta != 0)
        || patch.tension_delta.values().any(|delta| *delta != 0)
        || patch.number_delta.values().any(|delta| *delta != 0)
        || patch.inventory_delta.values().any(|delta| *delta != 0)
        || patch.relationship_transitions.iter().any(|(id, stage)| {
            game.characters
                .get(id)
                .is_none_or(|state| state.relationship_stage != *stage)
        })
        || patch
            .set_flags
            .iter()
            .any(|(id, value)| game.flags.get(id) != Some(value))
        || patch
            .set_strings
            .iter()
            .any(|(id, value)| game.strings.get(id) != Some(value))
        || patch
            .add_events
            .iter()
            .any(|event| !game.occurred_events.contains(event))
        || patch
            .next_location
            .as_ref()
            .is_some_and(|next| *next != game.location)
        || patch
            .next_time_slot
            .is_some_and(|next| next != game.time_slot)
        || patch.proposed_route.as_ref() != game.active_route.as_ref()
            && patch.proposed_route.is_some()
}

fn patch_carries_progress(patch: &StatePatch) -> bool {
    patch.affection_delta.values().any(|delta| *delta != 0)
        || patch.trust_delta.values().any(|delta| *delta != 0)
        || patch.tension_delta.values().any(|delta| *delta != 0)
        || patch.number_delta.values().any(|delta| *delta != 0)
        || patch.inventory_delta.values().any(|delta| *delta != 0)
        || !patch.relationship_transitions.is_empty()
        || !patch.set_flags.is_empty()
        || !patch.set_strings.is_empty()
        || !patch.add_events.is_empty()
        || patch.next_location.is_some()
        || patch.next_time_slot.is_some()
        || patch.proposed_route.is_some()
}

fn summary_has_progress(summary: &TurnSummary) -> bool {
    [
        &summary.facts,
        &summary.unresolved,
        &summary.relationship_changes,
        &summary.knowledge_changes,
        &summary.address_form_changes,
        &summary.important_player_actions,
    ]
    .into_iter()
    .any(|items| items.iter().any(|item| !item.trim().is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use story_model::{HistoryEntry, LocationId};

    fn entry(turn: u32, narration: &str) -> HistoryEntry {
        HistoryEntry {
            id: story_model::uuid::Uuid::new_v4(),
            turn,
            scene_id: "cafe-reunion".to_string(),
            created_at: story_model::chrono::Utc::now(),
            narration: narration.to_string(),
            player_action: None,
            choices: Vec::new(),
            applied_patch: StatePatch::default(),
            generation_id: story_model::uuid::Uuid::new_v4(),
        }
    }

    #[test]
    fn full_prose_motifs_include_words_after_the_first_sentence() {
        let history = vec![
            entry(
                1,
                "비가 잦아들었다. 유나는 시계를 보고 창밖으로 시선을 돌렸다.",
            ),
            entry(
                2,
                "문이 닫혔다. 시계 초침 아래에서 창밖의 빛과 시선을 피했다.",
            ),
            entry(
                3,
                "재즈가 흐른다. 손가락이 시계 옆을 두드리고 시선은 창밖에 머문다.",
            ),
        ];
        let motifs = recurring_motifs(&history);
        assert!(motifs.contains(&"시계".to_string()), "{motifs:?}");
        assert!(motifs.contains(&"창밖".to_string()), "{motifs:?}");
        assert!(motifs.contains(&"시선".to_string()), "{motifs:?}");
    }

    #[test]
    fn exact_replay_from_repetitive_save_fixture_is_rejected() {
        let pack = crate::tests_support::sample_pack();
        let mut game = crate::engine::EngineState::initial(&pack).game;
        // Captured from the real autosave regression where turns 3 and 4 were
        // byte-identical. Keep it in a fixture so future heuristic changes are
        // checked against the failure players actually saw.
        let repeated = include_str!("../tests/fixtures/repeated_narration_ko.txt");
        game.history.push(entry(3, repeated));
        let report = assess_narration(&game, repeated, &StatePatch::default(), None, true);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| matches!(issue, NarrationQualityIssue::ExactDuplicate { turn: 3 }))
        );
    }

    #[test]
    fn two_consecutive_no_op_turns_are_stalled() {
        let pack = crate::tests_support::sample_pack();
        let mut game = crate::engine::EngineState::initial(&pack).game;
        game.location = LocationId("loc-classroom".to_string());
        game.history.push(entry(1, "첫 장면은 조용히 끝났다."));
        let report = assess_narration(
            &game,
            "두 번째 장면도 별다른 변화 없이 끝났다.",
            &StatePatch::default(),
            None,
            true,
        );
        assert!(
            report
                .issues
                .contains(&NarrationQualityIssue::StalledProgress)
        );
    }

    #[test]
    fn copied_long_sentence_is_rejected_even_when_the_rest_changes() {
        let pack = crate::tests_support::sample_pack();
        let mut game = crate::engine::EngineState::initial(&pack).game;
        let copied = "유나는 접어 둔 초대장을 테이블 위에 천천히 펴고 오늘 저녁 전시회의 정확한 주소를 가리켰다";
        game.history
            .push(entry(1, &format!("문이 열렸다. {copied}.")));
        let report = assess_narration(
            &game,
            &format!("전화가 울렸다. {copied}. 바리스타가 다가왔다."),
            &StatePatch::default(),
            None,
            false,
        );
        assert!(
            report
                .issues
                .iter()
                .any(|issue| matches!(issue, NarrationQualityIssue::HighSimilarity { turn: 1 }))
        );
    }

    #[test]
    fn three_recurring_full_prose_motifs_are_rejected() {
        let pack = crate::tests_support::sample_pack();
        let mut game = crate::engine::EngineState::initial(&pack).game;
        game.history.extend([
            entry(1, "문이 닫혔다. 시계를 보고 창밖으로 시선을 돌렸다."),
            entry(2, "재즈가 멎었다. 시계 아래에서 창밖의 빛과 시선을 피했다."),
            entry(3, "주문표가 놓였다. 시계를 두드리며 시선은 창밖에 머문다."),
        ]);
        let report = assess_narration(
            &game,
            "휴대전화가 울렸다. 또다시 시계를 만지며 시선은 창밖으로 향했다.",
            &StatePatch::default(),
            None,
            false,
        );
        assert!(report.issues.iter().any(|issue| matches!(
            issue,
            NarrationQualityIssue::ReusedMotifs { motifs }
                if motifs.contains(&"시계".to_string())
                    && motifs.contains(&"시선".to_string())
                    && motifs.contains(&"창밖".to_string())
        )));
    }

    #[test]
    fn structured_summary_progress_allows_a_quiet_patch() {
        let pack = crate::tests_support::sample_pack();
        let mut game = crate::engine::EngineState::initial(&pack).game;
        game.history.push(entry(1, "첫 장면은 조용히 끝났다."));
        let summary = TurnSummary {
            facts: vec!["유나가 오늘 서울에 있다는 사실을 확인했다".to_string()],
            unresolved: Vec::new(),
            relationship_changes: Vec::new(),
            knowledge_changes: Vec::new(),
            interpretation: None,
            address_form_changes: Vec::new(),
            important_player_actions: Vec::new(),
            source_history_ids: Vec::new(),
        };
        let report = assess_narration(
            &game,
            "유나는 근무지가 서울이라고 처음으로 구체적으로 답했다.",
            &StatePatch::default(),
            Some(&summary),
            true,
        );
        assert!(
            !report
                .issues
                .contains(&NarrationQualityIssue::StalledProgress)
        );
    }

    #[test]
    fn punctuation_only_repair_is_not_distinct() {
        assert!(!repair_is_distinct(
            "유나는 초대장을 내밀었다.",
            "유나는, 초대장을 내밀었다!"
        ));
    }

    #[test]
    fn soft_issues_do_not_block_commit_after_repair() {
        let soft = NarrationQualityReport {
            issues: vec![
                NarrationQualityIssue::ReusedMotifs {
                    motifs: vec!["시계".into(), "시선".into(), "창밖".into()],
                },
                NarrationQualityIssue::StalledProgress,
            ],
        };
        assert!(!soft.is_ok());
        assert!(!soft.blocks_commit_after_repair());

        let hard_similarity = NarrationQualityReport {
            issues: vec![NarrationQualityIssue::HighSimilarity { turn: 4 }],
        };
        assert!(hard_similarity.blocks_commit_after_repair());

        let hard_duplicate = NarrationQualityReport {
            issues: vec![
                NarrationQualityIssue::ReusedMotifs {
                    motifs: vec!["시계".into()],
                },
                NarrationQualityIssue::ExactDuplicate { turn: 3 },
            ],
        };
        assert!(hard_duplicate.blocks_commit_after_repair());

        let hard_dialogue = NarrationQualityReport {
            issues: vec![NarrationQualityIssue::RepeatedDialogue { turn: 2 }],
        };
        assert!(hard_dialogue.blocks_commit_after_repair());

        let hard_beat = NarrationQualityReport {
            issues: vec![NarrationQualityIssue::RepeatedQuestionBeat {
                beat: "why_contact".into(),
                turn: 1,
            }],
        };
        assert!(hard_beat.blocks_commit_after_repair());
    }

    #[test]
    fn empty_narration_is_flagged_even_when_history_is_empty() {
        let pack = crate::tests_support::sample_pack();
        let game = crate::engine::EngineState::initial(&pack).game;
        // Empty and whitespace-only prose must be flagged — the
        // fingerprint/similarity scans silently skip them.
        for candidate in ["", "   \n\t "] {
            let report = assess_narration(&game, candidate, &StatePatch::default(), None, true);
            assert!(
                report.issues.contains(&NarrationQualityIssue::Empty),
                "{candidate:?} must be flagged Empty, got {:?}",
                report.issues
            );
        }
        let report = assess_narration(
            &game,
            "첫 번째 장면이 조용히 시작된다.",
            &StatePatch::default(),
            None,
            true,
        );
        assert!(
            !report.issues.contains(&NarrationQualityIssue::Empty),
            "readable prose is not Empty: {:?}",
            report.issues
        );
    }

    #[test]
    fn common_korean_function_words_are_excluded_from_motifs() {
        let history = vec![
            entry(
                1,
                "유나는 여전히 대답 없이 창밖만 보고 있고 어떤 말도 건네지 않았다.",
            ),
            entry(
                2,
                "그냥 앉아 있는 시간 속에서 아니면 먼저 말을 꺼내야 하는 건지 고민했다.",
            ),
            entry(
                3,
                "여전히 침묵이 있고 어떤 대답 없이 아니면 그냥 커피만 바라보았다.",
            ),
        ];
        let motifs = recurring_motifs(&history);
        for word in ["여전히", "대답", "있고", "어떤", "그냥", "아니면", "건지"] {
            assert!(
                !motifs.contains(&word.to_string()),
                "function word {word:?} leaked into motifs: {motifs:?}"
            );
        }
    }

    #[test]
    fn two_turn_setting_overlap_does_not_hard_reject_turn_three() {
        let pack = crate::tests_support::sample_pack();
        let mut game = crate::engine::EngineState::initial(&pack).game;
        game.history.extend([
            entry(
                1,
                "카페 창밖으로 가로등 불빛이 번지고 은색 시계가 반짝인다.",
            ),
            entry(
                2,
                "유나는 은색 시계를 만지작거리며 창밖 가로등을 바라보았다.",
            ),
        ]);
        let report = assess_narration(
            &game,
            "커피잔을 밀어둔 채 은색 시계와 창밖 가로등을 스치듯 보고 질문을 던졌다.",
            &StatePatch::default(),
            None,
            false,
        );
        assert!(
            !report
                .issues
                .iter()
                .any(|issue| matches!(issue, NarrationQualityIssue::ReusedMotifs { .. })),
            "2 turns of overlap must not hard-reject turn 3: {:?}",
            report.issues
        );
    }

    #[test]
    fn reused_setting_motifs_with_real_progress_are_not_hard_rejected() {
        let pack = crate::tests_support::sample_pack();
        let mut game = crate::engine::EngineState::initial(&pack).game;
        game.history.extend([
            entry(1, "카페 창밖 빗소리 사이로 재즈 선율이 흐른다."),
            entry(2, "카페 안의 재즈가 잦아들고 창밖 불빛이 흔들린다."),
            entry(3, "창밖을 등진 채 카페 테이블 위의 재즈 팸플릿을 넘겼다."),
        ]);
        let mut patch = StatePatch::default();
        patch
            .tension_delta
            .insert(story_model::CharacterId("c-namu".to_string()), 1);
        let report = assess_narration(
            &game,
            "새 손님이 들어선 카페 문 너머로 창밖 우산과 재즈 LP가 나란히 놓였다.",
            &patch,
            None,
            true,
        );
        assert!(
            report.is_ok(),
            "novel prose with state progress in the same setting must pass: {:?}",
            report.issues
        );
    }

    #[test]
    fn single_callback_sentence_in_long_novel_turn_is_allowed() {
        let pack = crate::tests_support::sample_pack();
        let mut game = crate::engine::EngineState::initial(&pack).game;
        // Keep the shared callback under the dialogue fingerprint floor so a
        // brief echo alone does not hard-gate; longer identical lines are
        // covered by RepeatedDialogue tests below.
        let callback = "말해줄래요";
        game.history.push(entry(
            1,
            &format!("비 오는 카페 창가에서 두 사람이 마주 앉았다. {callback}?"),
        ));
        let candidate = format!(
            "유나는 이미 가방을 고쳐 멘 채 자리에서 일어날 준비를 마쳤다. \
             카페 안의 공기는 폐점을 앞둔 서늘한 정적과 빗소리로 가득 차 대화의 여백을 메운다. \
             창밖으로 시선을 던진 그녀의 옆얼굴에는 피로감이 짙게 배어 있고 손목 시계가 차갑게 빛난다. \
             {callback}? \
             카운터 너머 주인은 의자를 정리하며 마감 준비를 서두르고 가로등 불빛이 유리창에 길게 번졌다. \
             이 어정쩡한 재회가 흐지부지 끝나기 전에 무언가 분명한 대답을 꺼내야 할 순간이 다가오고 있다."
        );
        let mut patch = StatePatch::default();
        patch
            .tension_delta
            .insert(story_model::CharacterId("c-namu".to_string()), 1);
        let report = assess_narration(&game, &candidate, &patch, None, true);
        assert!(
            report.is_ok(),
            "one short callback sentence inside long novel prose should pass: {:?}",
            report.issues
        );
        assert!(repair_is_distinct(
            &format!("유나는 다른 이야기를 이어갔다. {callback}."),
            &candidate,
        ));
    }

    #[test]
    fn identical_long_yuna_dialogue_hard_blocks_even_when_prose_differs() {
        let pack = crate::tests_support::sample_pack();
        let mut game = crate::engine::EngineState::initial(&pack).game;
        let yuna = "유나: \"지금 이 자리에서 내일 전시회에 같이 갈지 말지 확실히 정해줘요\"";
        assert!(
            dialogue_fingerprints(yuna)
                .first()
                .is_some_and(|fp| fp.chars().count() >= 20),
            "fixture dialogue must clear the fingerprint floor"
        );
        // Surrounding prose differs and shares no long sentence / high trigram
        // overlap, so full-prose HighSimilarity must not fire — only dialogue.
        game.history.push(entry(
            1,
            &format!(
                "빗줄기가 처마를 두드리고 카페 안은 낮은 재즈로 가득했다 {yuna} 바리스타가 젖은 트레이를 닦았다"
            ),
        ));
        let candidate = format!(
            "가로등이 유리창에 길게 번지고 폐점 종소리가 짧게 울렸다 {yuna} 주인이 의자를 테이블 위로 올리기 시작했다"
        );
        let report = assess_narration(&game, &candidate, &StatePatch::default(), None, false);
        assert!(
            !report
                .issues
                .iter()
                .any(|issue| matches!(issue, NarrationQualityIssue::HighSimilarity { .. })),
            "surrounding prose must differ enough to avoid HighSimilarity: {:?}",
            report.issues
        );
        assert!(
            report.issues.iter().any(|issue| {
                matches!(issue, NarrationQualityIssue::RepeatedDialogue { turn: 1 })
            }),
            "identical long Yuna dialogue must hard-gate: {:?}",
            report.issues
        );
        assert!(
            report.blocks_commit_after_repair(),
            "RepeatedDialogue must block commit after repair"
        );
    }

    #[test]
    fn completely_new_dialogue_does_not_trigger_repeated_dialogue() {
        let pack = crate::tests_support::sample_pack();
        let mut game = crate::engine::EngineState::initial(&pack).game;
        game.history.push(entry(
            1,
            "유나: \"지금 이 자리에서 내일 전시회에 같이 갈지 말지 확실히 정해줘요\"",
        ));
        let report = assess_narration(
            &game,
            "유나: \"가방 안의 초대장 주소가 맞는지 먼저 확인하고 출발 시간을 다시 잡자\"",
            &StatePatch::default(),
            None,
            false,
        );
        assert!(
            !report
                .issues
                .iter()
                .any(|issue| matches!(issue, NarrationQualityIssue::RepeatedDialogue { .. })),
            "novel NPC ask must not raise RepeatedDialogue: {:?}",
            report.issues
        );
    }

    #[test]
    fn short_shared_callback_under_fingerprint_floor_does_not_alone_trigger() {
        let pack = crate::tests_support::sample_pack();
        let mut game = crate::engine::EngineState::initial(&pack).game;
        let short = "유나: \"뭐라고?\"";
        assert!(
            dialogue_fingerprints(short).is_empty(),
            "short interjection must stay under the dialogue floor"
        );
        game.history.push(entry(
            1,
            &format!("창가 자리에 앉은 채 커피잔을 만지작거렸다. {short}"),
        ));
        let report = assess_narration(
            &game,
            &format!("폐점을 알리는 종소리가 울리자 그녀가 고개를 들었다. {short}"),
            &StatePatch::default(),
            None,
            false,
        );
        assert!(
            !report
                .issues
                .iter()
                .any(|issue| matches!(issue, NarrationQualityIssue::RepeatedDialogue { .. })),
            "sub-floor callback alone must not raise RepeatedDialogue: {:?}",
            report.issues
        );
    }

    #[test]
    fn multiple_copied_long_sentences_in_long_turn_are_rejected() {
        let pack = crate::tests_support::sample_pack();
        let mut game = crate::engine::EngineState::initial(&pack).game;
        let s1 = "유나는 접어 둔 초대장을 테이블 위에 천천히 펴고 오늘 저녁 전시회의 정확한 주소를 가리켰다";
        let s2 = "창밖으로 굵은 빗줄기가 쏟아지는 가운데 카페 안의 재즈 선율이 낮게 바닥을 울리며 흘렀다";
        game.history
            .push(entry(1, &format!("{s1}. 이어서 잠시 숨을 골랐다. {s2}.")));
        let candidate = format!(
            "새로운 손님이 문을 열고 들어섰다. {s1}. \
             바리스타가 주문표를 정리하며 카운터 뒤로 물러났다. {s2}. \
             유나는 계산서를 집어 들며 자리에서 일어날 채비를 했다."
        );
        let report = assess_narration(&game, &candidate, &StatePatch::default(), None, false);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| matches!(issue, NarrationQualityIssue::HighSimilarity { turn: 1 })),
            "two copied long sentences must still trigger HighSimilarity: {:?}",
            report.issues
        );
    }

    #[test]
    fn repeated_why_meet_question_beat_hard_blocks_despite_new_wording() {
        let pack = crate::tests_support::sample_pack();
        let mut game = crate::engine::EngineState::initial(&pack).game;
        let turn1 = "유나: \"왜 나를 불렀어? 오늘 갑자기 만나자고 한 이유가 뭐야?\"";
        let turn2 = "유나: \"왜 지금 나타난 거야? 왜 연락한 거지?\"";
        assert_eq!(
            classify_question_beats(turn1),
            vec!["why_contact".to_string()],
            "turn1 must classify as why_contact"
        );
        assert_eq!(
            classify_question_beats(turn2),
            vec!["why_contact".to_string()],
            "turn2 must classify as why_contact despite different wording"
        );
        game.history.push(entry(1, turn1));
        let report = assess_narration(&game, turn2, &StatePatch::default(), None, false);
        assert!(
            report.issues.iter().any(|issue| {
                matches!(
                    issue,
                    NarrationQualityIssue::RepeatedQuestionBeat {
                        beat,
                        turn: 1
                    } if beat == "why_contact"
                )
            }),
            "same why-meet beat with new wording must hard-gate: {:?}",
            report.issues
        );
        assert!(
            report.blocks_commit_after_repair(),
            "RepeatedQuestionBeat must block commit after repair"
        );
        let instructions = report.repair_instructions();
        assert!(
            instructions.contains("why_contact") && instructions.contains("전진"),
            "repair text must name the beat and push forward: {instructions}"
        );
    }

    #[test]
    fn why_meet_then_family_disclosure_without_repeat_ask_is_ok() {
        let pack = crate::tests_support::sample_pack();
        let mut game = crate::engine::EngineState::initial(&pack).game;
        game.history.push(entry(
            1,
            "유나: \"왜 나를 불렀어? 오늘 갑자기 만나자고 한 이유가 뭐야?\"",
        ));
        let disclosure = "유나: \"사실 어머니 병간호 때문에 서울에 남아 있었어. 그때는 말할 수 없었어.\"";
        assert!(
            classify_question_beats(disclosure).is_empty(),
            "disclosure must not carry the why_contact beat: {:?}",
            classify_question_beats(disclosure)
        );
        let report = assess_narration(&game, disclosure, &StatePatch::default(), None, false);
        assert!(
            !report.issues.iter().any(|issue| {
                matches!(issue, NarrationQualityIssue::RepeatedQuestionBeat { .. })
            }),
            "family disclosure without re-asking must pass beat gate: {:?}",
            report.issues
        );
    }
}
