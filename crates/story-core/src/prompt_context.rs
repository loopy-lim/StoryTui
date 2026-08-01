//! Deterministic prompt assembly (ST-018).
//!
//! [`build_prompt`] turns a [`StoryPack`] + [`GameState`] + recent history into
//! a [`PromptBundle`] (system, user, SHA-256 hash). The output is a pure
//! function of its inputs: same pack/state/history ⇒ byte-identical system and
//! user strings ⇒ identical hash, regardless of the order in which the caller
//! built the underlying `BTreeMap`s. This is the contract the snapshot/golden
//! tests pin down.
//!
//! # Ordering rules
//!
//! - Characters, locations, flags, numbers, inventory, and strings are all
//!   stored as `BTreeMap`/`BTreeSet`; iteration is key order.
//! - Recent history is iterated in slice (turn) order.
//! - The state snapshot is serialized via `serde_json` whose default `Map` is
//!   `BTreeMap`-backed, so its keys are alphabetical — independent of the order
//!   in which the snapshot object was constructed.
//!
//! # ADR-003
//!
//! `sha2` is a pure-Rust crypto crate (no IO/UI/HTTP). Adding it to `story-core`
//! does not violate ADR-003, which forbids only `ratatui`/`crossterm`/`reqwest`
//! (and FS). `check-core-deps.sh` continues to pass.

use sha2::{Digest, Sha256};
use story_model::serde_json;
use story_model::{Choice, GameState, HistoryEntry, ParsedAction, StoryPack};

/// The assembled prompt: system + user strings plus the canonical hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptBundle {
    /// System prompt (role/safety/output-contract + allowed IDs + rules).
    pub system: String,
    /// User prompt (world/style/opening + state snapshot + recent turns).
    pub user: String,
    /// Lowercase-hex SHA-256 of `system + "\n\n---\n\n" + user`.
    pub hash: String,
}

/// Assemble a deterministic [`PromptBundle`] for one generation call.
///
/// Pure function of (`pack`, `state`, `recent`, `player_action`). When
/// `player_action` is `Some`, the system prompt gets an extra player-
/// sovereignty reinforcement and the user prompt carries the parsed action
/// (with `original_text` verbatim). See the module docs for the ordering rules
/// that make it deterministic and construction-order-independent.
pub fn build_prompt(
    pack: &StoryPack,
    state: &GameState,
    recent: &[HistoryEntry],
    player_action: Option<&ParsedAction>,
) -> PromptBundle {
    build_prompt_with_choice(pack, state, recent, player_action, None)
}

/// As [`build_prompt`], but includes the numbered choice the player just
/// selected. A normal choice is not represented by `ParsedAction`, so omitting
/// it makes successive turns look identical to the model.
pub fn build_prompt_with_choice(
    pack: &StoryPack,
    state: &GameState,
    recent: &[HistoryEntry],
    player_action: Option<&ParsedAction>,
    player_choice: Option<&Choice>,
) -> PromptBundle {
    let system = build_system(pack, player_action);
    let user = build_user(pack, state, recent, player_action, player_choice);

    let mut hasher = Sha256::new();
    hasher.update(system.as_bytes());
    hasher.update(b"\n\n---\n\n");
    hasher.update(user.as_bytes());
    let hash = hex_lower(hasher.finalize().as_slice());

    PromptBundle { system, user, hash }
}

// ---------------------------------------------------------------------------
// system prompt
// ---------------------------------------------------------------------------

fn build_system(pack: &StoryPack, player_action: Option<&ParsedAction>) -> String {
    let mut s = String::new();

    // Role / safety / output contract from the Pack.
    s.push_str(&pack.prompts.system);
    s.push_str("\n\n");

    // Allowed IDs — all iterated in BTreeMap/BTreeSet key order.
    s.push_str("# Allowed IDs\n");
    s.push_str("## Characters\n");
    for id in pack.characters.keys() {
        s.push_str("- ");
        s.push_str(&id.0);
        s.push('\n');
    }
    s.push_str("## Locations\n");
    for id in pack.rules.locations.keys() {
        s.push_str("- ");
        s.push_str(&id.0);
        s.push('\n');
    }
    s.push_str("## Flags\n");
    for id in &pack.rules.allowed_flags {
        s.push_str("- ");
        s.push_str(id);
        s.push('\n');
    }

    // Per-turn delta limits.
    let dl = &pack.rules.delta_limits;
    s.push_str(&format!(
        "\n# Delta limits (per turn, inclusive)\n\
         affection: {a_min}..={a_max}\n\
         trust: {t_min}..={t_max}\n\
         tension: {n_min}..={n_max}\n",
        a_min = dl.affection.min,
        a_max = dl.affection.max,
        t_min = dl.trust.min,
        t_max = dl.trust.max,
        n_min = dl.tension.min,
        n_max = dl.tension.max,
    ));

    // Choice count.
    s.push_str(&format!(
        "# Choices\noffer exactly {} choices\n",
        pack.default_choice_count()
    ));

    // Player-sovereignty rule (fixed text).
    s.push_str(
        "\n# Player sovereignty\n\
         Do not fix emotions, dialogue, physical contact, forgiveness, confessions, or \
         decisions the player did not specify. Do not invert or shrink the core intent of \
         the player's free-text input.\n",
    );
    s.push_str(
        "\n# Continuation and variation\n\
         Continue from the current state and the recent turns. Do not restart the opening. \
         Do not copy an exact sentence or repeat the same sensory description from a recent \
         narration. Advance only one small, observable beat in this turn.\n",
    );

    // When this turn originates from a free-text action, reinforce the
    // sovereignty clause in Korean right where the model reads its role.
    if player_action.is_some() {
        s.push_str(
            "\n# Free-text action sovereignty\n\
             이 입력은 플레이어의 의도다. 감정이나 행동을 임의로 확정/왜곡하지 말 것. \
             플레이어가 명시하지 않은 사랑·용서·접촉·고백을 첨가하지 말 것.\n",
        );
    }

    s
}

// ---------------------------------------------------------------------------
// user prompt
// ---------------------------------------------------------------------------

fn build_user(
    pack: &StoryPack,
    state: &GameState,
    recent: &[HistoryEntry],
    player_action: Option<&ParsedAction>,
    player_choice: Option<&Choice>,
) -> String {
    let mut s = String::new();

    s.push_str("# World\n");
    s.push_str(&pack.world_md);
    s.push_str("\n\n# Style\n");
    s.push_str(&pack.style_md);
    if recent.is_empty() {
        s.push_str("\n\n# Opening (first turn only)\n");
        s.push_str(&pack.opening_md);
    } else {
        s.push_str(
            "\n\n# Opening reference\n\
             The opening has already been shown. Do not reproduce its prose; continue from \
             the current state and recent turns below.\n",
        );
    }

    s.push_str("\n\n# Current state\n");
    s.push_str(&state_snapshot_json(state));

    s.push_str("\n\n# Recent turns\n");
    if recent.is_empty() {
        s.push_str("(none)\n");
    }
    for h in recent {
        s.push_str(&format!("## turn {} ({})\n", h.turn, h.scene_id));
        s.push_str("narration: ");
        s.push_str(&h.narration);
        s.push('\n');
        if let Some(act) = &h.player_action {
            s.push_str(&format!(
                "action: {} ({})\n",
                act.content,
                serde_json::to_string(&act.action_type).unwrap_or_else(|_| "\"?\"".to_string())
            ));
        }
    }

    if let Some(choice) = player_choice {
        s.push_str("\n# Player selected choice\n");
        s.push_str(&format!("id: {}\n", choice.id));
        s.push_str(&format!("label: {}\n", choice.label));
        s.push_str(&format!("intent: {}\n", choice.intent));
        if let Some(tone) = &choice.tone {
            s.push_str(&format!("tone: {tone}\n"));
        }
        if let Some(target) = &choice.target {
            s.push_str(&format!("target: {}\n", target.0));
        }
    }

    // When this turn originates from a free-text action, surface the parsed
    // intent (verbatim `original_text` + classification) to the model. The
    // sovereignty rule lives in the system prompt; this block only conveys the
    // player's input without interpreting it.
    if let Some(pa) = player_action {
        s.push_str("\n# Player free action\n");
        s.push_str(&format!(
            "action_type: {}\n",
            serde_json::to_string(&pa.action_type).unwrap_or_else(|_| "\"?\"".to_string())
        ));
        if let Some(t) = &pa.target {
            s.push_str(&format!("target: {}\n", t.0));
        }
        if let Some(c) = pa.confidence {
            s.push_str(&format!("confidence: {c:.2}\n"));
        }
        s.push_str(&format!("content: {}\n", pa.content));
        s.push_str(&format!("original_text: {}\n", pa.original_text));
    }

    s
}

/// Build a deterministic compact-JSON snapshot of the current state. Iteration
/// over `BTreeMap`s guarantees key order; `serde_json`'s default `Map` is also
/// `BTreeMap`-backed so the serialized keys are alphabetical regardless of the
/// order in which the snapshot object was assembled.
fn state_snapshot_json(state: &GameState) -> String {
    let mut characters: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    for (id, c) in &state.characters {
        let mut m = serde_json::Map::new();
        m.insert("affection".to_string(), c.affection.into());
        m.insert("trust".to_string(), c.trust.into());
        m.insert("tension".to_string(), c.tension.into());
        m.insert(
            "emotion".to_string(),
            serde_json::to_value(c.current_emotion).expect("emotion serializes"),
        );
        m.insert(
            "stage".to_string(),
            serde_json::to_value(c.relationship_stage).expect("stage serializes"),
        );
        characters.insert(id.0.clone(), serde_json::Value::Object(m));
    }

    let root = serde_json::json!({
        "location": state.location,
        "time_slot": state.time_slot,
        "turn": state.turn,
        "characters": characters,
        "flags": state.flags,
        "numbers": state.numbers,
        "inventory": state.inventory,
    });
    serde_json::to_string(&root).expect("state snapshot serializes")
}

// ---------------------------------------------------------------------------
// hashing helpers
// ---------------------------------------------------------------------------

/// Lowercase-hex encode a byte slice without pulling in a `hex` crate.
fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    //! The hash-correctness check lives here (rather than in an integration
    //! test) because `sha2` is a regular dependency of this crate and is thus
    //! directly available in in-crate unit tests.

    use super::*;
    use std::collections::{BTreeMap, VecDeque};
    use story_model::{
        CharacterState, Emotion, GameState, LocationId, RelationshipStage, SAVE_SCHEMA_VERSION,
        StoryPack,
    };

    fn minimal_fixture() -> (StoryPack, GameState) {
        let pack = StoryPack {
            metadata: story_model::PackMetadata {
                schema_version: semver::Version::parse("1.0.0").unwrap(),
                story_id: "t".to_string(),
                story_version: semver::Version::new(0, 1, 0),
                min_engine_version: semver::Version::new(0, 1, 0),
                title: "t".to_string(),
                language: "ko-KR".to_string(),
                genres: vec![],
                rating: story_model::PackRating::General,
                all_major_characters_safe: true,
                default_choice_count: 3,
                initial_state: story_model::PackInitialState {
                    chapter: "c".to_string(),
                    scene: "s".to_string(),
                    location: LocationId("l".to_string()),
                    time_slot: story_model::TimeSlot::Morning,
                },
                content: story_model::PackContentPolicy {
                    safe_content: "none".to_string(),
                    violence: "none".to_string(),
                },
            },
            characters: BTreeMap::new(),
            rules: story_model::StoryRules::minimal(),
            world_md: "w".to_string(),
            style_md: "st".to_string(),
            opening_md: "o".to_string(),
            prompts: story_model::PromptFragments {
                system: "sys".to_string(),
                director: "d".to_string(),
                writer: "w".to_string(),
            },
            pack_root: std::path::PathBuf::from("/tmp"),
        };
        let state = GameState {
            save_schema_version: SAVE_SCHEMA_VERSION,
            story_id: "t".to_string(),
            story_version: semver::Version::new(0, 1, 0),
            pack_schema_version: semver::Version::new(1, 0, 0),
            session_id: story_model::uuid::Uuid::nil(),
            current_chapter: "c".to_string(),
            current_scene_id: "s".to_string(),
            turn: 0,
            location: LocationId("l".to_string()),
            time_slot: story_model::TimeSlot::Morning,
            active_route: None,
            characters: BTreeMap::from([(
                story_model::CharacterId("c-1".to_string()),
                CharacterState {
                    affection: 0,
                    trust: 0,
                    tension: 0,
                    relationship_stage: RelationshipStage::Stranger,
                    current_emotion: Emotion::Neutral,
                    known_facts: std::collections::BTreeSet::new(),
                    address_form_overrides: BTreeMap::new(),
                },
            )]),
            flags: BTreeMap::new(),
            numbers: BTreeMap::new(),
            strings: BTreeMap::new(),
            inventory: BTreeMap::new(),
            occurred_events: std::collections::BTreeSet::new(),
            recent_summaries: VecDeque::new(),
            memories: Vec::new(),
            history: Vec::new(),
            last_choice: None,
            revision: 0,
            stable_generation_id: None,
        };
        (pack, state)
    }

    #[test]
    fn hash_matches_manual_sha256_of_concatenation() {
        let (pack, state) = minimal_fixture();
        let bundle = build_prompt(&pack, &state, &[], None);
        // Recompute the expected hash independently.
        let expected_input = format!("{}\n\n---\n\n{}", bundle.system, bundle.user);
        let mut h = Sha256::new();
        h.update(expected_input.as_bytes());
        let expected: String = h
            .finalize()
            .as_slice()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(bundle.hash, expected);
    }

    #[test]
    fn build_prompt_unit_runs_minimally() {
        let (pack, state) = minimal_fixture();
        let b = build_prompt(&pack, &state, &[], None);
        assert!(!b.system.is_empty());
        assert!(!b.user.is_empty());
        assert_eq!(b.hash.len(), 64);
    }
}
