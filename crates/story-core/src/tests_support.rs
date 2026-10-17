//! Test-support fixtures shared across the `story-core` integration tests.
//!
//! Pure data builders only — no IO, no mocking. Exposed as a `pub` module so
//! the W8 save/load tests can drive the same pack / draft shapes the W5/W7
//! reduce tests use, without duplicating them.

use std::path::PathBuf;

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use story_model::GameState;
use story_model::HistoryEntry;
use story_model::LocationDef;
use story_model::StatePatch;
use story_model::{
    CharacterDef, Choice, GeneratedTurnDraft, LocationId, PackContentPolicy, PackInitialState,
    PackMetadata, PackRating, PromptFragments, RelationshipStage, RiskLevel, StoryPack, StoryRules,
    TimeSlot, TurnSummary,
};
use story_model::{CharacterId, semver, uuid};
use story_model::{CharacterState, Emotion};

/// The "demo" pack used across reduce tests.
pub fn sample_pack() -> StoryPack {
    let metadata = PackMetadata {
        schema_version: semver::Version::parse("1.0.0").unwrap(),
        story_id: "demo".to_string(),
        story_version: semver::Version::new(0, 1, 0),
        min_engine_version: semver::Version::new(0, 1, 0),
        title: "데모 이야기".to_string(),
        language: "ko-KR".to_string(),
        genres: vec!["drama".to_string()],
        rating: PackRating::General,
        default_choice_count: 3,
        initial_state: PackInitialState {
            chapter: "chapter-1".to_string(),
            scene: "scene-1".to_string(),
            location: LocationId("loc-classroom".to_string()),
            time_slot: TimeSlot::Morning,
        },
        content: PackContentPolicy {
            violence: "none".to_string(),
        },
    };

    let character = CharacterDef {
        id: cid(),
        name: "나무".to_string(),
        age: Some(28),
        pronouns: None,
        description_md: None,
        dialogue_examples: Vec::new(),
        personality: vec![],
        appearance: vec![],
        values: vec![],
        likes: vec![],
        dislikes: vec![],
        secrets: vec![],
        initial_player_relationship: RelationshipStage::Acquaintance,
        speech_style: String::new(),
        honorific_rule: String::new(),
        frequent_phrases: vec![],
        avoid_phrases: vec![],
        known_facts: vec![],
        unknown_facts: vec![],
        current_goal: String::new(),
        long_term_desire: String::new(),
    };

    let mut characters = BTreeMap::new();
    characters.insert(cid(), character);

    let mut rules = StoryRules::minimal();
    rules.locations.insert(
        LocationId("loc-classroom".to_string()),
        LocationDef {
            id: LocationId("loc-classroom".to_string()),
            display_name: "교실".to_string(),
            adjacent: Default::default(),
        },
    );
    rules
        .allowed_flags
        .extend(["f-brave", "f-met"].map(String::from));

    StoryPack {
        lorebook: story_model::Lorebook::default(),
        metadata,
        characters,
        rules,
        world_md: String::new(),
        style_md: String::new(),
        opening_md: "오프닝: 비가 그쳤다.".to_string(),
        prompts: PromptFragments {
            system: "sys".to_string(),
            director: "d".to_string(),
            writer: "w".to_string(),
        },
        pack_root: PathBuf::from("/tmp/demo"),
    }
}

/// A draft whose patch applies cleanly against [`sample_pack`]'s rules
/// (`affection +N` for the known character).
pub fn valid_draft(generation_id: uuid::Uuid, affection_delta: i16) -> GeneratedTurnDraft {
    let mut patch = StatePatch::default();
    patch.affection_delta.insert(cid(), affection_delta);
    GeneratedTurnDraft {
        generation_id,
        narration: format!("유나가 고개를 끄덕인다 (affection {affection_delta:+})."),
        choices: vec![
            choice("choice-0", "다시 만난다", RiskLevel::Low),
            choice("choice-1", "돌아선다", RiskLevel::Medium),
            choice("choice-2", "기다린다", RiskLevel::Safe),
        ],
        proposed_patch: patch,
        summary: TurnSummary {
            facts: vec![],
            unresolved: vec![],
            relationship_changes: vec![],
            knowledge_changes: vec![],
            interpretation: None,
            address_form_changes: vec![],
            important_player_actions: vec![],
            source_history_ids: vec![],
        },
        usage: None,
    }
}

/// A minimal, all-defaults [`GameState`] for lorebook / prompt tests.
pub fn sample_game_state() -> GameState {
    GameState {
        save_schema_version: story_model::SAVE_SCHEMA_VERSION,
        story_id: "demo".to_string(),
        story_version: semver::Version::new(0, 1, 0),
        pack_schema_version: semver::Version::new(1, 0, 0),
        session_id: uuid::Uuid::nil(),
        current_chapter: "chapter-1".to_string(),
        current_scene_id: "scene-1".to_string(),
        turn: 1,
        location: LocationId("loc-classroom".to_string()),
        time_slot: TimeSlot::Morning,
        active_route: None,
        characters: BTreeMap::from([(
            cid(),
            CharacterState {
                affection: 0,
                trust: 0,
                tension: 0,
                relationship_stage: RelationshipStage::Acquaintance,
                current_emotion: Emotion::Neutral,
                known_facts: BTreeSet::new(),
                address_form_overrides: BTreeMap::new(),
            },
        )]),
        flags: BTreeMap::from([("f-brave".to_string(), false)]),
        numbers: BTreeMap::new(),
        strings: BTreeMap::new(),
        inventory: BTreeMap::new(),
        occurred_events: BTreeSet::new(),
        recent_summaries: VecDeque::new(),
        memories: Vec::new(),
        history: Vec::new(),
        last_choice: None,
        revision: 0,
        stable_generation_id: None,
        lorebook_activations: Vec::new(),
    }
}

/// One history entry carrying `narration` at `turn`.
pub fn sample_history_entry(turn: u32, narration: &str) -> HistoryEntry {
    HistoryEntry {
        id: uuid::Uuid::nil(),
        turn,
        scene_id: "scene-1".to_string(),
        created_at: story_model::chrono::Utc::now(),
        narration: narration.to_string(),
        player_action: None,
        choices: Vec::new(),
        applied_patch: StatePatch::default(),
        generation_id: uuid::Uuid::nil(),
    }
}

fn choice(id: &str, label: &str, risk: RiskLevel) -> Choice {
    Choice {
        id: id.to_string(),
        label: label.to_string(),
        intent: String::new(),
        tone: None,
        target: None,
        risk,
        hidden_requirements: vec![],
        preview: None,
    }
}

fn cid() -> CharacterId {
    CharacterId("c-namu".to_string())
}
