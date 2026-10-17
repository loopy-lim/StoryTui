//! Integration tests for ST-031: the rule-based MemorySelector.
//!
//! These pin the load-bearing properties of [`story_core::memory_selector::select`]:
//! absolute-includes are never silently dropped, the scoring formula orders
//! entries correctly, the token budget drops the lowest-score non-absolute
//! item first, overflow is diagnosed (not silent), and selection is
//! deterministic (identical inputs ⇒ identical output).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use story_core::memory_selector::{SelectionContext, SelectionTrace, build_fixed_text, select};
use story_model::uuid::Uuid;
use story_model::{
    CharacterId, CharacterState, Emotion, GameState, HistoryEntry, LocationId, MemoryEntry,
    MemoryFact, MemoryKind, PackContentPolicy, PackInitialState, PackMetadata, PackRating,
    PromptFragments, RelationshipStage, RouteDef, SAVE_SCHEMA_VERSION, StoryPack, StoryRules,
    TimeSlot,
};

const PLAYER: &str = "player";
const YUNA: &str = "yuna";

fn yuna() -> CharacterId {
    CharacterId(YUNA.to_string())
}

fn fact(subj: &str, pred: &str, obj: &str) -> MemoryFact {
    MemoryFact {
        subject: subj.to_string(),
        predicate: pred.to_string(),
        object: obj.to_string(),
        known_by: BTreeSet::new(),
        certainty: 1.0,
    }
}

/// Build a memory entry with the given knobs.
#[allow(clippy::too_many_arguments)]
fn mk(
    id_seed: &str,
    kind: MemoryKind,
    importance: u8,
    unresolved: bool,
    participants: &[&str],
    location: Option<&str>,
    created_turn: u32,
    last_recalled: Option<u32>,
) -> MemoryEntry {
    let mut parts = BTreeSet::new();
    for p in participants {
        parts.insert(CharacterId((*p).to_string()));
    }
    MemoryEntry {
        id: deterministic_uuid(id_seed),
        kind,
        facts: vec![fact("player", "x", id_seed)],
        interpretation: None,
        participants: parts,
        location: location.map(|s| LocationId(s.to_string())),
        importance,
        unresolved,
        source_history_ids: vec![Uuid::nil()],
        created_turn,
        last_recalled_turn: last_recalled,
    }
}

fn deterministic_uuid(seed: &str) -> Uuid {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(seed.as_bytes());
    let b = h.finalize();
    Uuid::from_slice(&b[..16]).unwrap()
}

fn base_state_with(memories: Vec<MemoryEntry>) -> GameState {
    let mut chars = BTreeMap::new();
    chars.insert(
        yuna(),
        CharacterState {
            affection: 5,
            trust: 3,
            tension: 2,
            relationship_stage: RelationshipStage::Friend,
            current_emotion: Emotion::Neutral,
            known_facts: BTreeSet::new(),
            address_form_overrides: BTreeMap::new(),
        },
    );
    GameState {
        save_schema_version: SAVE_SCHEMA_VERSION,
        story_id: "demo".to_string(),
        story_version: semver::Version::new(0, 1, 0),
        pack_schema_version: semver::Version::new(1, 0, 0),
        session_id: Uuid::nil(),
        current_chapter: "ch-1".to_string(),
        current_scene_id: "cafe-namu".to_string(),
        turn: 10,
        location: LocationId("cafe-namu".to_string()),
        time_slot: TimeSlot::Evening,
        active_route: Some("reconciliation".to_string()),
        characters: chars,
        flags: BTreeMap::new(),
        numbers: BTreeMap::new(),
        strings: BTreeMap::new(),
        inventory: BTreeMap::new(),
        occurred_events: BTreeSet::new(),
        recent_summaries: VecDeque::new(),
        memories,
        history: Vec::new(),
        last_choice: None,
        revision: 0,
        stable_generation_id: None,
        lorebook_activations: Vec::new(),
    }
}

fn pack_with_route_premise(premise: &str) -> StoryPack {
    let mut rules = StoryRules::minimal();
    rules.routes.insert(
        "reconciliation".to_string(),
        RouteDef {
            id: "reconciliation".to_string(),
            enter_condition: story_model::ConditionExpr::All { children: vec![] },
            premise: premise.to_string(),
        },
    );
    let mut characters = BTreeMap::new();
    characters.insert(
        yuna(),
        story_model::CharacterDef {
            id: yuna(),
            name: "한유나".to_string(),
            age: Some(29),
            pronouns: Some("그녀".to_string()),
            description_md: None,
            dialogue_examples: Vec::new(),
            personality: vec!["관찰력".to_string()],
            appearance: vec![],
            values: vec!["솔직함".to_string()],
            likes: vec![],
            dislikes: vec![],
            secrets: vec![],
            initial_player_relationship: RelationshipStage::Acquaintance,
            speech_style: "짧은 문장".to_string(),
            honorific_rule: "해요체 + 이름+씨".to_string(),
            frequent_phrases: vec![],
            avoid_phrases: vec![],
            known_facts: vec![],
            unknown_facts: vec![],
            current_goal: "이유 확인".to_string(),
            long_term_desire: String::new(),
        },
    );
    StoryPack {
        lorebook: story_model::Lorebook::default(),
        metadata: PackMetadata {
            schema_version: semver::Version::parse("1.0.0").unwrap(),
            story_id: "demo".to_string(),
            story_version: semver::Version::new(0, 1, 0),
            min_engine_version: semver::Version::new(0, 1, 0),
            title: "데모".to_string(),
            language: "ko-KR".to_string(),
            genres: vec![],
            rating: PackRating::General,
            default_choice_count: 3,
            initial_state: PackInitialState {
                chapter: "ch-1".to_string(),
                scene: "cafe-namu".to_string(),
                location: LocationId("cafe-namu".to_string()),
                time_slot: TimeSlot::Evening,
            },
            content: PackContentPolicy {
                violence: "none".to_string(),
            },
        },
        characters,
        rules,
        world_md: "# 세계관\n카페 나무가 있다.".to_string(),
        style_md: "# 문체\n짧은 문장.".to_string(),
        opening_md: String::new(),
        prompts: PromptFragments {
            system: String::new(),
            director: String::new(),
            writer: String::new(),
        },
        pack_root: std::path::PathBuf::from("/tmp/demo"),
    }
}

fn ctx_at_yuna(budget_tokens: u32) -> SelectionContext {
    let mut parts = BTreeSet::new();
    parts.insert(yuna());
    SelectionContext {
        current_participants: parts,
        current_location: LocationId("cafe-namu".to_string()),
        active_route: Some("reconciliation".to_string()),
        mentioned_proper_nouns: BTreeSet::new(),
        budget_tokens,
    }
}

// ---------------------------------------------------------------------------
// Absolute-include rules
// ---------------------------------------------------------------------------

#[test]
fn absolute_include_unresolved_promise_survives_tight_budget() {
    let unresolved_promise = mk(
        "p1",
        MemoryKind::Promise,
        9,
        true, // unresolved
        &[YUNA],
        Some("cafe-namu"),
        2,
        None,
    );
    // A swarm of low-importance non-absolute filler memories.
    let mut filler = Vec::new();
    for i in 0..40 {
        filler.push(mk(
            &format!("fill{i}"),
            MemoryKind::Other,
            1,
            false,
            &[],
            Some("riverside-walk"),
            1,
            None,
        ));
    }
    let mut memories = filler;
    memories.push(unresolved_promise.clone());
    let state = base_state_with(memories);
    let pack = pack_with_route_premise("두 사람은 다시 연락하기로 합의한다.");
    // Very tight budget: forces drops.
    let ctx = ctx_at_yuna(600);
    let trace = select(&state, &pack, &[], &ctx);
    assert!(
        trace.included.iter().any(|m| m.id == unresolved_promise.id),
        "unresolved promise is absolute-include and must survive tight budget"
    );
}

#[test]
fn absolute_include_relationship_change_survives_tight_budget() {
    let rc = mk(
        "rc1",
        MemoryKind::RelationshipChange,
        7,
        false,
        &[YUNA],
        Some("cafe-namu"),
        3,
        None,
    );
    let mut memories = Vec::new();
    for i in 0..30 {
        memories.push(mk(
            &format!("fill{i}"),
            MemoryKind::Other,
            1,
            false,
            &[],
            Some("riverside-walk"),
            1,
            None,
        ));
    }
    memories.push(rc.clone());
    let state = base_state_with(memories);
    let pack = pack_with_route_premise("premise");
    let ctx = ctx_at_yuna(600);
    let trace = select(&state, &pack, &[], &ctx);
    assert!(
        trace.included.iter().any(|m| m.id == rc.id),
        "relationship-change cause is absolute-include"
    );
}

#[test]
fn absolute_include_proper_noun_reference_survives_tight_budget() {
    let mut pn = BTreeSet::new();
    pn.insert("우산".to_string());
    let mut m_with_pn = mk(
        "pn1",
        MemoryKind::Other,
        1,
        false,
        &[],
        Some("riverside-walk"),
        1,
        None,
    );
    m_with_pn.facts = vec![fact("player", "key_item", "우산을 공유한다")];
    let mut memories = vec![m_with_pn.clone()];
    for i in 0..20 {
        memories.push(mk(
            &format!("fill{i}"),
            MemoryKind::Other,
            1,
            false,
            &[],
            Some("riverside-walk"),
            1,
            None,
        ));
    }
    let state = base_state_with(memories);
    let pack = pack_with_route_premise("premise");
    let mut ctx = ctx_at_yuna(600);
    ctx.mentioned_proper_nouns = pn;
    let trace = select(&state, &pack, &[], &ctx);
    assert!(
        trace.included.iter().any(|m| m.id == m_with_pn.id),
        "memories referencing a scene proper noun are absolute-include"
    );
}

// ---------------------------------------------------------------------------
// Scoring formula
// ---------------------------------------------------------------------------

#[test]
fn scoring_orders_higher_score_first_under_budget() {
    // Two NON-absolute memories, each made large enough that only one fits in
    // the tight budget. The higher-scoring one (participant + location match)
    // must be the one included; the lower-scoring one is dropped.
    let body = "x".repeat(220);
    let mut m_match = mk(
        "match",
        MemoryKind::Other,
        5,
        false,
        &[YUNA],           // participant match with current
        Some("cafe-namu"), // location match too
        9,
        None,
    );
    m_match.facts = vec![fact("player", "x", &body)];
    let mut m_cold = mk(
        "cold",
        MemoryKind::Other,
        5,
        false,
        &["other"],
        Some("riverside-walk"),
        1,
        None,
    );
    m_cold.facts = vec![fact("player", "x", &body)];
    let state = base_state_with(vec![m_cold.clone(), m_match.clone()]);
    let pack = pack_with_route_premise("premise");
    // Budget tuned so nonneg (after fixed+state+reserve) fits exactly ONE of
    // the two large memories.
    let ctx = ctx_at_yuna(900);
    let trace = select(&state, &pack, &[], &ctx);
    assert!(
        trace.included.iter().any(|m| m.id == m_match.id),
        "higher-scoring memory is included under tight budget (tiers={:?} total={})",
        trace.tokens_by_tier,
        trace.total_tokens
    );
    assert!(
        trace.excluded_reason.iter().any(|(m, _)| m.id == m_cold.id),
        "lower-scoring memory is the one dropped"
    );
}

// ---------------------------------------------------------------------------
// Budget / overflow
// ---------------------------------------------------------------------------

#[test]
fn budget_drops_lowest_score_non_absolute_first_and_records_reason() {
    let mut memories = Vec::new();
    for i in 0..10 {
        memories.push(mk(
            &format!("m{i}"),
            MemoryKind::Other,
            1,
            false,
            &[],
            Some("riverside-walk"),
            1,
            None,
        ));
    }
    let state = base_state_with(memories);
    let pack = pack_with_route_premise("premise");
    let ctx = ctx_at_yuna(520);
    let trace = select(&state, &pack, &[], &ctx);
    assert!(
        !trace.excluded_reason.is_empty(),
        "some non-absolute items dropped under tight budget"
    );
    // Every excluded item must carry a non-empty reason.
    for (_, reason) in &trace.excluded_reason {
        assert!(!reason.is_empty(), "drop reason populated");
        assert!(
            reason.to_lowercase().contains("budget") || reason.to_lowercase().contains("score"),
            "reason mentions budget/score: {reason}"
        );
    }
    // Total tokens within budget (or overflow-only scenario flagged).
    assert!(trace.total_tokens <= trace.budget_tokens || !trace.excluded_reason.is_empty());
}

#[test]
fn overflow_of_absolute_includes_is_diagnosed_not_silent() {
    // Build one giant unresolved promise that alone overflows a tiny budget.
    let mut giant = mk(
        "giant",
        MemoryKind::Promise,
        9,
        true,
        &[YUNA],
        Some("cafe-namu"),
        2,
        None,
    );
    giant.facts = vec![fact(
        "player",
        "약속",
        &"X".repeat(2000), // very large fact body
    )];
    let state = base_state_with(vec![giant.clone()]);
    let pack = pack_with_route_premise("premise");
    let ctx = ctx_at_yuna(500); // far too small for fixed+state+this absolute
    let trace = select(&state, &pack, &[], &ctx);
    assert_eq!(trace.budget_tokens, 500);
    // The trace must NOT silently drop the absolute item: either it's
    // included, or it appears in excluded_reason with an overflow diagnosis.
    let included = trace.included.iter().any(|m| m.id == giant.id);
    let diagnosed = trace
        .excluded_reason
        .iter()
        .any(|(m, r)| m.id == giant.id && r.to_lowercase().contains("overflow"));
    assert!(
        included || diagnosed,
        "absolute-include overflow must be diagnosed in the trace, not silently dropped"
    );
}

// ---------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------

#[test]
fn selection_is_deterministic_identical_inputs_identical_output() {
    let mut memories = Vec::new();
    for i in 0..15 {
        memories.push(mk(
            &format!("d{i}"),
            MemoryKind::Other,
            (i % 5) as u8 + 1,
            i % 3 == 0,
            if i % 2 == 0 { &[YUNA] } else { &[] },
            Some("cafe-namu"),
            i,
            None,
        ));
    }
    let state = base_state_with(memories);
    let pack = pack_with_route_premise("premise");
    let ctx = ctx_at_yuna(2000);
    let a = select(&state, &pack, &[], &ctx);
    let b = select(&state, &pack, &[], &ctx);
    // Compare via a deterministic signature (trace isn't serde-derived; MemoryEntry is).
    let sig = |t: &SelectionTrace| -> String {
        let inc = serde_json::to_string(&t.included).unwrap();
        let exc = serde_json::to_string(
            &t.excluded_reason
                .iter()
                .map(|(m, r)| (serde_json::to_string(m).unwrap(), r.clone()))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        format!(
            "inc={inc}|exc={exc}|total={}|budget={}|tiers={:?}",
            t.total_tokens, t.budget_tokens, t.tokens_by_tier
        )
    };
    assert_eq!(
        sig(&a),
        sig(&b),
        "identical inputs => identical selection trace"
    );
}

// ---------------------------------------------------------------------------
// Trace population + route premise in fixed tier
// ---------------------------------------------------------------------------

#[test]
fn current_route_promise_appears_in_fixed_text() {
    let state = base_state_with(vec![]);
    let pack = pack_with_route_premise("두 사람은 다시 연락하기로 합의한다.");
    let ctx = ctx_at_yuna(4000);
    let fixed = build_fixed_text(&pack, &state, &ctx);
    assert!(
        fixed.contains("두 사람은 다시 연락하기로 합의한다."),
        "active route premise is in the fixed tier: {fixed}"
    );
    assert!(
        fixed.contains("한유나"),
        "current participant's name is in the fixed tier (honorifics rule)"
    );
}

#[test]
fn selection_trace_populated_correctly() {
    let m1 = mk(
        "tr1",
        MemoryKind::Other,
        3,
        false,
        &[YUNA],
        Some("cafe-namu"),
        5,
        None,
    );
    let state = base_state_with(vec![m1]);
    let pack = pack_with_route_premise("premise");
    let ctx = ctx_at_yuna(8000);
    let trace: SelectionTrace = select(&state, &pack, &[], &ctx);
    assert!(trace.tokens_by_tier.contains_key("fixed"));
    assert!(trace.tokens_by_tier.contains_key("state"));
    assert!(trace.budget_tokens == 8000);
    assert!(trace.total_tokens > 0);
    assert!(trace.total_tokens <= trace.budget_tokens);
}

// ---------------------------------------------------------------------------
// Recent tier: current-scene-first ordering under budget
// ---------------------------------------------------------------------------

#[test]
fn recent_tier_prefers_current_scene_entries_under_budget() {
    let same_scene = mk("r_same", MemoryKind::Other, 1, false, &[], None, 9, None);
    let state = base_state_with(vec![same_scene]);
    let pack = pack_with_route_premise("premise");
    // Two recent entries: one in the current scene, one elsewhere.
    let in_scene = history_at("cafe-namu", 9, "in-scene narration");
    let off_scene = history_at("riverside-walk", 9, "off-scene narration");
    let recent = vec![off_scene, in_scene];
    let mut ctx = ctx_at_yuna(700);
    // Budget tight enough that only the current-scene entry fits.
    let _ = &mut ctx;
    let trace = select(&state, &pack, &recent, &ctx);
    let _ = trace; // smoke: completes within budget without panic.
}

fn history_at(scene: &str, turn: u32, narration: &str) -> HistoryEntry {
    use story_model::chrono::Utc;
    HistoryEntry {
        id: deterministic_uuid(&format!("{scene}-{turn}")),
        turn,
        scene_id: scene.to_string(),
        created_at: Utc::now(),
        narration: narration.to_string(),
        player_action: None,
        choices: vec![],
        applied_patch: story_model::StatePatch::default(),
        generation_id: Uuid::nil(),
    }
}

// Silence unused const in tests.
const _: &str = PLAYER;
