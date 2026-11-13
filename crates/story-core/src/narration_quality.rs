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

/// A narration defect that must be repaired before the turn can be committed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NarrationQualityIssue {
    /// The candidate is the same prose as a recent committed turn after
    /// whitespace/punctuation normalization.
    ExactDuplicate { turn: u32 },
    /// A long sentence or most character trigrams were copied from a recent
    /// committed turn.
    HighSimilarity { turn: u32 },
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
        let callback =
            "아니면 당신이 오늘 나를 부른 진짜 이유가 뭔지 지금이라도 짧게라도 말해줄래요";
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
}
