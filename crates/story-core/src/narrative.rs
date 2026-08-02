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
const MAX_RECENT_ROLES: usize = 3;

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
    }

    for (id, character) in &pack.characters {
        if !game.characters.contains_key(id) || character.current_goal.trim().is_empty() {
            continue;
        }
        let key = agenda_key(&id.0);
        let agenda = match phase {
            "arrival" => format!("approach: {}", character.current_goal),
            "pressure" => format!("press for movement: {}", character.current_goal),
            "complication" => format!(
                "pursue a cost or obstacle around: {}",
                character.current_goal
            ),
            _ => format!("reassess the consequence of: {}", character.current_goal),
        };
        game.strings.insert(key, agenda);
    }
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

pub fn classify_choice(choice: &Choice) -> &'static str {
    let text = format!("{} {}", choice.intent, choice.label).to_lowercase();
    if [
        "ask", "trust", "boundary", "offer", "사과", "묻", "약속", "거절", "마음",
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
        "찾",
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
