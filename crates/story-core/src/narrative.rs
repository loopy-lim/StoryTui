//! Engine-owned narrative continuity.
//!
//! This module deliberately stores its small, deterministic ledger in the
//! existing `GameState` maps under a reserved namespace.  That makes it
//! backwards-compatible with old save files while keeping it out of every
//! player-facing screen and Pack rule.  Models only receive a compact read-only
//! brief through `prompt_context`.

use story_model::{Choice, GameState, StoryPack};

use crate::memory_selector::{SelectionContext, select};

const PREFIX: &str = "__storytui.narrative.";
const PHASE: &str = "__storytui.narrative.phase";
const PRESSURE: &str = "__storytui.narrative.pressure";
const DUE_TURN: &str = "__storytui.narrative.due_turn";
const RECENT_ROLES: &str = "__storytui.narrative.recent_choice_roles";
const SAME_SCENE_STREAK: &str = "__storytui.narrative.same_scene_streak";
const RECENT_CHOICE_LABELS: &str = "__storytui.narrative.recent_choice_labels";
const REQUIRED_LEAD: &str = "__storytui.narrative.required_lead";
const MAX_RECENT_ROLES: usize = 3;
const MAX_RECENT_LABELS: usize = 6;
const MAX_SCENE_STREAK: i64 = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LeadKind {
    DiscloseSecret,
    MakeDecision,
    PlaceTimeChange,
    PushEventFlags,
    HardDeadline,
}

impl LeadKind {
    fn id(self) -> &'static str {
        match self {
            Self::DiscloseSecret => "disclose_secret",
            Self::MakeDecision => "make_decision",
            Self::PlaceTimeChange => "place_time_change",
            Self::PushEventFlags => "push_event_flags",
            Self::HardDeadline => "hard_deadline",
        }
    }

    fn from_cycle(turn: u32) -> Self {
        match turn % 5 {
            0 => Self::DiscloseSecret,
            1 => Self::MakeDecision,
            2 => Self::PlaceTimeChange,
            3 => Self::PushEventFlags,
            _ => Self::HardDeadline,
        }
    }
}

/// After this many same-scene turns, choice design must force a place/time change.
pub const SCENE_STREAK_FORCE: u32 = 3;

const PHASES: [&str; 4] = ["arrival", "pressure", "complication", "aftermath"];

/// Advance internal story momentum after, and only after, an accepted turn.
/// No model-proposed fact is persisted here: the values are derived from the
/// stable turn number, selected choice, pack goals, and already-validated
/// state.
pub fn advance_after_accept(game: &mut GameState, pack: &StoryPack, selected: Option<&Choice>) {
    let phase = PHASES[(game.turn as usize) % PHASES.len()];
    game.strings.insert(PHASE.to_string(), phase.to_string());
    game.numbers
        .insert(DUE_TURN.to_string(), i64::from(game.turn + 1));

    let pressure = if let Some(memory) = game.memories.iter().rev().find(|memory| memory.unresolved)
    {
        memory
            .interpretation
            .as_deref()
            .filter(|text| !text.trim().is_empty())
            .or_else(|| memory.facts.first().map(|fact| fact.object.as_str()))
            .unwrap_or("an unresolved consequence")
            .to_string()
    } else if let Some(choice) = selected {
        format!("follow through on the player's {}", choice.intent.trim())
    } else {
        format!("move the current scene at {} forward", game.location.0)
    };
    game.strings.insert(PRESSURE.to_string(), pressure);

    if let Some(choice) = selected {
        let role = classify_choice(choice);
        let mut roles = recent_choice_roles(game);
        roles.push(role.to_string());
        if roles.len() > MAX_RECENT_ROLES {
            roles.drain(0..roles.len() - MAX_RECENT_ROLES);
        }
        game.strings
            .insert(RECENT_ROLES.to_string(), roles.join(","));

        let mut labels = recent_choice_labels(game);
        labels.push(choice.label.clone());
        if labels.len() > MAX_RECENT_LABELS {
            labels.drain(0..labels.len() - MAX_RECENT_LABELS);
        }
        game.strings
            .insert(RECENT_CHOICE_LABELS.to_string(), labels.join("|"));
    }

    let scene_changed = game
        .history
        .last()
        .is_some_and(|entry| {
            entry.applied_patch.next_location.is_some()
                || entry.applied_patch.next_time_slot.is_some()
        });
    let streak = if scene_changed {
        0
    } else {
        (same_scene_streak(game) as i64 + 1).min(MAX_SCENE_STREAK)
    };
    game.numbers.insert(SAME_SCENE_STREAK.to_string(), streak);

    let lead_kind = pick_lead_kind(game, pack);
    game.strings
        .insert(REQUIRED_LEAD.to_string(), lead_kind.id().to_string());
    let lead_theme = lead_agenda_theme(game, pack, lead_kind);

    for (id, character) in &pack.characters {
        if !game.characters.contains_key(id) || character.current_goal.trim().is_empty() {
            continue;
        }
        let key = agenda_key(&id.0);
        let agenda = match phase {
            "arrival" => format!("pursue required lead: {lead_theme}"),
            "pressure" => format!("press the required lead: {lead_theme}"),
            "complication" => format!("raise cost around required lead: {lead_theme}"),
            _ => format!("reassess consequence of required lead: {lead_theme}"),
        };
        game.strings.insert(key, agenda);
    }
}

/// Concrete mandatory beat so the model cannot stall on "why did you call?".
/// Rotates by turn and pack progress (flags / events / secrets).
pub fn required_lead(game: &GameState, pack: &StoryPack) -> String {
    let kind = pick_lead_kind(game, pack);
    format_lead(game, pack, kind)
}

/// Mark memories that the *same deterministic selector* recalled for the
/// current request.  The caller performs this only after a stable acceptance,
/// so cancelled/failed requests never alter save state.
pub fn mark_recalled(game: &mut GameState, pack: &StoryPack) {
    let trace = select(
        game,
        pack,
        &[],
        &SelectionContext {
            current_participants: game.characters.keys().cloned().collect(),
            current_location: game.location.clone(),
            active_route: game.active_route.clone(),
            mentioned_proper_nouns: Default::default(),
            budget_tokens: 6_000,
        },
    );
    let recalled_ids: Vec<_> = trace.included.into_iter().map(|memory| memory.id).collect();
    for memory in &mut game.memories {
        if recalled_ids.contains(&memory.id) {
            memory.last_recalled_turn = Some(game.turn);
        }
    }
}

pub fn phase(game: &GameState) -> &str {
    game.strings
        .get(PHASE)
        .map(String::as_str)
        .unwrap_or("arrival")
}

pub fn pressure(game: &GameState) -> Option<&str> {
    game.strings.get(PRESSURE).map(String::as_str)
}

pub fn due_turn(game: &GameState) -> u32 {
    game.numbers
        .get(DUE_TURN)
        .copied()
        .unwrap_or(i64::from(game.turn + 1))
        .max(0) as u32
}

pub fn agenda<'a>(game: &'a GameState, character_id: &str) -> Option<&'a str> {
    game.strings
        .get(&agenda_key(character_id))
        .map(String::as_str)
}

pub fn recent_choice_roles(game: &GameState) -> Vec<String> {
    game.strings
        .get(RECENT_ROLES)
        .map(|value| {
            value
                .split(',')
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub fn same_scene_streak(game: &GameState) -> u32 {
    game.numbers
        .get(SAME_SCENE_STREAK)
        .copied()
        .unwrap_or(0)
        .max(0) as u32
}

pub fn recent_choice_labels(game: &GameState) -> Vec<String> {
    game.strings
        .get(RECENT_CHOICE_LABELS)
        .map(|value| {
            value
                .split('|')
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

pub fn classify_choice(choice: &Choice) -> &'static str {
    let text = format!("{} {}", choice.intent, choice.label).to_lowercase();
    if [
        "ask",
        "trust",
        "boundary",
        "offer",
        "사과",
        "묻",
        "약속",
        "거절",
        "마음",
        "솔직",
        "진심",
        "답한다",
        "고백",
        "경계",
    ]
    .iter()
    .any(|needle| text.contains(needle))
    {
        "relationship"
    } else if [
        "look",
        "check",
        "investigate",
        "clue",
        "확인",
        "조사",
        "찾는다",
        "찾아본",
        "찾아낸",
        "읽",
        "대조",
        "꺼내",
        "보여",
        "편지",
        "메시지",
        "사진",
        "증거",
        "관찰",
    ]
    .iter()
    .any(|needle| text.contains(needle))
    {
        "evidence"
    } else {
        "situation"
    }
}

fn agenda_key(character_id: &str) -> String {
    format!("{PREFIX}agenda.{character_id}")
}

fn flag_is_true(game: &GameState, id: &str) -> bool {
    game.flags.get(id).copied().unwrap_or(false)
}

fn pack_tracks_flag(pack: &StoryPack, id: &str) -> bool {
    pack.rules.allowed_flags.iter().any(|flag| flag == id)
}

fn unfinished_progress_flags(game: &GameState, pack: &StoryPack) -> Vec<&'static str> {
    let mut out = Vec::new();
    if pack_tracks_flag(pack, "mentioned_returned_letters")
        && !flag_is_true(game, "mentioned_returned_letters")
    {
        out.push("mentioned_returned_letters");
    }
    if pack_tracks_flag(pack, "asked_about_departure") && !flag_is_true(game, "asked_about_departure")
    {
        out.push("asked_about_departure");
    }
    out
}

fn pick_lead_kind(game: &GameState, pack: &StoryPack) -> LeadKind {
    let mut kind = LeadKind::from_cycle(game.turn);
    let unfinished = unfinished_progress_flags(game, pack);
    let has_secrets = pack
        .characters
        .values()
        .any(|character| character.secrets.iter().any(|s| !s.trim().is_empty()));

    // Prefer unfinished pack progress over repeating disclose/decision loops.
    if !unfinished.is_empty()
        && matches!(
            kind,
            LeadKind::DiscloseSecret | LeadKind::MakeDecision | LeadKind::PushEventFlags
        )
        && (kind == LeadKind::PushEventFlags || game.turn % 2 == 1)
    {
        kind = LeadKind::PushEventFlags;
    }
    if kind == LeadKind::DiscloseSecret && !has_secrets {
        kind = if unfinished.is_empty() {
            LeadKind::MakeDecision
        } else {
            LeadKind::PushEventFlags
        };
    }
    if same_scene_streak(game) >= SCENE_STREAK_FORCE
        && matches!(kind, LeadKind::DiscloseSecret | LeadKind::MakeDecision)
    {
        kind = LeadKind::PlaceTimeChange;
    }
    kind
}

fn primary_secret<'a>(pack: &'a StoryPack, game: &GameState) -> Option<&'a str> {
    let turn = game.turn as usize;
    let secrets: Vec<&str> = pack
        .characters
        .values()
        .flat_map(|character| character.secrets.iter().map(String::as_str))
        .filter(|text| !text.trim().is_empty())
        .collect();
    if secrets.is_empty() {
        None
    } else {
        Some(secrets[turn % secrets.len()])
    }
}

fn lead_agenda_theme(game: &GameState, pack: &StoryPack, kind: LeadKind) -> String {
    match kind {
        LeadKind::DiscloseSecret => {
            let secret = primary_secret(pack, game).unwrap_or("pack secret");
            format!("disclose one secret in dialogue: {secret}")
        }
        LeadKind::MakeDecision => {
            "decide: show envelope contents / leave / demand yes-no".to_string()
        }
        LeadKind::PlaceTimeChange => {
            "force place or time change with new environmental pressure".to_string()
        }
        LeadKind::PushEventFlags => {
            let unfinished = unfinished_progress_flags(game, pack);
            if unfinished.is_empty() {
                "push toward unresolved event flags with concrete speech".to_string()
            } else {
                format!(
                    "push flags via concrete speech: {}",
                    unfinished.join("; ")
                )
            }
        }
        LeadKind::HardDeadline => {
            "set hard deadline (closing / last train) with consequence".to_string()
        }
    }
}

fn format_lead(game: &GameState, pack: &StoryPack, kind: LeadKind) -> String {
    match kind {
        LeadKind::DiscloseSecret => {
            let secret = primary_secret(pack, game).unwrap_or(
                "아직 밝히지 않은 팩 비밀을 하나 고른다 / pick one unrevealed pack secret",
            );
            format!(
                "[disclose_secret] 필수: NPC가 이번 턴 대사에서 비밀을 정확히 하나 밝힌다(팩 원문 인용). \
                 REQUIRED: Disclose ONE secret from character.secrets in dialogue this turn — cite pack text: 「{secret}」. \
                 Do not ask why the player called."
            )
        }
        LeadKind::MakeDecision => {
            "[make_decision] 필수: NPC가 결정을 내린다 — 봉투 내용을 보여주거나, 자리를 뜨거나, 예/아니오 답을 요구한다. \
             REQUIRED: Make a decision (show envelope contents / leave / demand yes-no). \
             Do not stall with another 'why did you ask to meet' question."
                .to_string()
        }
        LeadKind::PlaceTimeChange => {
            let adjacent = pack
                .rules
                .locations
                .get(&game.location)
                .map(|loc| {
                    loc.adjacent
                        .iter()
                        .map(|id| id.0.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| "an adjacent pack location".to_string());
            format!(
                "[place_time_change] 필수: state_patch로 장소 또는 시간대를 바꾸고 새 환경 압력(비·인파·마감 안내 등)을 넣는다. \
                 REQUIRED: Force place or time change in state_patch with new environmental pressure \
                 (current={}, options={adjacent}).",
                game.location.0
            )
        }
        LeadKind::PushEventFlags => {
            let mut targets = Vec::new();
            for flag in unfinished_progress_flags(game, pack) {
                match flag {
                    "mentioned_returned_letters" => targets.push(
                        "반송된 편지(mentioned_returned_letters)를 구체 대사로 꺼낸다 / speak the returned letters aloud",
                    ),
                    "asked_about_departure" => targets.push(
                        "떠난 이유(asked_about_departure)를 직접 묻거나 먼저 말한다 / ask or state the departure reason",
                    ),
                    _ => {}
                }
            }
            if targets.is_empty() {
                let events: Vec<&str> = pack
                    .rules
                    .events
                    .keys()
                    .filter(|id| !game.occurred_events.contains(&id.0))
                    .map(|id| id.0.as_str())
                    .take(2)
                    .collect();
                let hint = if events.is_empty() {
                    "unresolved pack progress".to_string()
                } else {
                    events.join(", ")
                };
                format!(
                    "[push_event_flags] 필수: 미발생 이벤트/플래그를 향한 구체 대사로 전진한다 ({hint}). \
                     REQUIRED: Push toward event flags with concrete speech — not meta-questions about why they met."
                )
            } else {
                format!(
                    "[push_event_flags] 필수: {}\nREQUIRED: Push toward event flags with concrete speech (returned letters / departure ask).",
                    targets.join(" | ")
                )
            }
        }
        LeadKind::HardDeadline => {
            "[hard_deadline] 필수: 마감/막차 등 하드 데드라인을 설정하고 지키지 못할 때의 결과를 말한다. \
             REQUIRED: Set a hard deadline (closing / last train) with a concrete consequence if ignored."
                .to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use story_model::{HistoryEntry, LocationId, StatePatch};

    #[test]
    fn classifier_uses_direction_not_exact_label_equality() {
        let choice = Choice {
            id: "ask".into(),
            label: "유나에게 솔직히 묻는다".into(),
            intent: "ask about the letter".into(),
            tone: None,
            target: None,
            risk: story_model::RiskLevel::Low,
            hidden_requirements: vec![],
            preview: None,
        };
        assert_eq!(classify_choice(&choice), "relationship");
    }

    fn history_with_patch(patch: StatePatch) -> HistoryEntry {
        HistoryEntry {
            id: story_model::uuid::Uuid::nil(),
            turn: 1,
            scene_id: "scene-1".to_string(),
            created_at: story_model::chrono::Utc::now(),
            narration: "비가 그쳤다.".to_string(),
            player_action: None,
            choices: Vec::new(),
            applied_patch: patch,
            generation_id: story_model::uuid::Uuid::nil(),
        }
    }

    #[test]
    fn same_scene_streak_increments_without_location_or_time_patch() {
        let pack = crate::tests_support::sample_pack();
        let mut game = crate::engine::EngineState::initial(&pack).game;
        game.history
            .push(history_with_patch(StatePatch::default()));
        let selected = Choice {
            id: "ask".into(),
            label: "유나에게 솔직히 묻는다".into(),
            intent: "ask about the letter".into(),
            tone: None,
            target: None,
            risk: story_model::RiskLevel::Low,
            hidden_requirements: vec![],
            preview: None,
        };

        advance_after_accept(&mut game, &pack, Some(&selected));
        assert_eq!(same_scene_streak(&game), 1);
        assert_eq!(
            recent_choice_labels(&game),
            vec!["유나에게 솔직히 묻는다".to_string()]
        );

        game.history
            .push(history_with_patch(StatePatch::default()));
        advance_after_accept(&mut game, &pack, Some(&selected));
        assert_eq!(same_scene_streak(&game), 2);
    }

    #[test]
    fn same_scene_streak_resets_when_last_history_moves_location() {
        let pack = crate::tests_support::sample_pack();
        let mut game = crate::engine::EngineState::initial(&pack).game;
        game.numbers
            .insert(SAME_SCENE_STREAK.to_string(), 4);
        let mut patch = StatePatch::default();
        patch.next_location = Some(LocationId("loc-rooftop".to_string()));
        game.history.push(history_with_patch(patch));

        advance_after_accept(&mut game, &pack, None);
        assert_eq!(same_scene_streak(&game), 0);
    }

    #[test]
    fn required_lead_is_non_empty_and_changes_across_turns() {
        let mut pack = crate::tests_support::sample_pack();
        if let Some(character) = pack.characters.values_mut().next() {
            character.secrets = vec![
                "3년 전 가족의 병간호 때문에 급히 서울을 떠났다".to_string(),
                "보내지 못한 편지를 아직 가지고 있다".to_string(),
            ];
            character.current_goal =
                "보내지 못한 편지를 보여줄지, 마감까지 남을지 결정한다".to_string();
        }
        pack.rules
            .allowed_flags
            .extend(["mentioned_returned_letters", "asked_about_departure"].map(String::from));

        let mut game = crate::engine::EngineState::initial(&pack).game;
        game.flags
            .insert("mentioned_returned_letters".to_string(), false);
        game.flags
            .insert("asked_about_departure".to_string(), false);

        let mut leads = Vec::new();
        for turn in 1..=6 {
            game.turn = turn;
            let lead = required_lead(&game, &pack);
            assert!(!lead.trim().is_empty(), "lead empty at turn {turn}");
            assert!(
                lead.contains("필수:") || lead.contains("REQUIRED:"),
                "lead missing bilingual directive at turn {turn}: {lead}"
            );
            leads.push(lead);
        }
        assert!(
            leads.windows(2).any(|pair| pair[0] != pair[1]),
            "required_lead must change across turns: {leads:?}"
        );

        advance_after_accept(&mut game, &pack, None);
        assert!(
            game.strings.contains_key(REQUIRED_LEAD),
            "advance_after_accept stores required_lead id"
        );
        let character_id = pack.characters.keys().next().unwrap().0.clone();
        let agenda = agenda(&game, &character_id).unwrap_or("");
        assert!(
            agenda.contains("required lead"),
            "agenda must pursue required lead theme, got: {agenda}"
        );
    }
}
