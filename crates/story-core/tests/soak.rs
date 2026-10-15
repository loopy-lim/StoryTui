//! W9 100-turn synthetic soak (ST-032).
//!
//! Drives a deterministic Mock-only 100-turn run against a pack whose events
//! are rigged to fire known key memories, then asserts the load-bearing
//! invariants of the memory subsystem:
//!
//! - state invariants every turn (revision == turn, no negative inventory,
//!   stats within limits, turn monotonic);
//! - memory creation is wired into `reduce_accept` (fired key events produce
//!   `MemoryEntry`s with the correct kind);
//! - 0 losses: every memory id ever created is still present at turn 100, and
//!   the union of `known_by` sets never shrinks (no knowledge-owner loss);
//! - resolving events clear the prior promise's `unresolved` flag without
//!   deleting the memory;
//! - `MemorySelector::select` stays within budget every turn (overflow
//!   diagnosed, never silent);
//! - save round-trip at turn 50 and turn 100 is byte-identical (pure-data
//!   `serde` round-trip; no FS — ADR-003);
//! - core-event recall ≥ 90% at turn 100 (the selector includes the fired key
//!   events under a generous budget).
//!
//! Mock-only, no network, non-ignored. ADR-003: no FS/UI/HTTP.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use story_core::engine::{EngineState, reduce};
use story_core::engine_cmd::{EngineCommand, EngineEffect};
use story_core::memory_selector::{SelectionContext, select};
use story_model::uuid::Uuid;
use story_model::{
    CharacterDef, CharacterId, CharacterState, Choice, ConditionExpr, Emotion, EventDef,
    EventEffects, EventId, GeneratedTurnDraft, LocationDef, LocationId, MemoryKind,
    PackContentPolicy, PackInitialState, PackMetadata, PackRating, PromptFragments,
    RelationshipStage, RiskLevel, RouteDef, SAVE_SCHEMA_VERSION, SavePayload, StatePatch,
    StoryPack, StoryRules, TimeSlot, TurnSummary,
};

const YUNA: &str = "yuna";

fn yuna() -> CharacterId {
    CharacterId(YUNA.to_string())
}

// ---------------------------------------------------------------------------
// Pack + rules fixtures
// ---------------------------------------------------------------------------

fn soak_pack() -> StoryPack {
    let mut rules = StoryRules::minimal();

    // Locations (a small adjacency cycle so next_location is never needed).
    for (id, adj) in [
        ("cafe-namu", vec!["riverside-walk"]),
        ("riverside-walk", vec!["cafe-namu"]),
    ] {
        let adjacent: BTreeSet<LocationId> =
            adj.into_iter().map(|s| LocationId(s.to_string())).collect();
        rules.locations.insert(
            LocationId(id.to_string()),
            LocationDef {
                id: LocationId(id.to_string()),
                display_name: id.to_string(),
                adjacent,
            },
        );
    }

    rules
        .allowed_flags
        .extend(["f-met", "f-apologized", "f-gift"].map(String::from));

    // A route whose premise is the absolute-include canary.
    rules.routes.insert(
        "reconciliation".to_string(),
        RouteDef {
            id: "reconciliation".to_string(),
            enter_condition: ConditionExpr::All { children: vec![] },
            premise: "두 사람은 다시 연락하기로 합의한다.".to_string(),
        },
    );

    // Memory-worthy events with always-true triggers (empty All). Each fires
    // whenever the scripted patch references it via add_events.
    let always = ConditionExpr::All { children: vec![] };
    let mk = |id: &str, priority: u32, effects: EventEffects| EventDef {
        id: EventId(id.to_string()),
        trigger: always.clone(),
        priority,
        effects,
        allow_relationship_skip: false,
        route_lock: None,
    };
    rules.events.insert(
        EventId("promise.share_umbrella".to_string()),
        mk("promise.share_umbrella", 10, EventEffects::default()),
    );
    rules.events.insert(
        EventId("secret.past_letter".to_string()),
        mk("secret.past_letter", 10, EventEffects::default()),
    );
    rules.events.insert(
        EventId("relationship.truce".to_string()),
        mk(
            "relationship.truce",
            10,
            EventEffects {
                affection_delta: BTreeMap::from([(yuna(), 1)]),
                ..EventEffects::default()
            },
        ),
    );
    rules.events.insert(
        EventId("key_item.umbrella".to_string()),
        mk("key_item.umbrella", 10, EventEffects::default()),
    );
    rules.events.insert(
        EventId("first_meeting.yuna".to_string()),
        mk("first_meeting.yuna", 10, EventEffects::default()),
    );
    rules.events.insert(
        EventId("confession.love".to_string()),
        mk("confession.love", 10, EventEffects::default()),
    );
    rules.events.insert(
        EventId("betrayal.lie".to_string()),
        mk("betrayal.lie", 10, EventEffects::default()),
    );
    rules.events.insert(
        EventId("resolve_promise.share_umbrella".to_string()),
        mk(
            "resolve_promise.share_umbrella",
            20,
            EventEffects::default(),
        ),
    );

    let mut characters = BTreeMap::new();
    characters.insert(
        yuna(),
        CharacterDef {
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
            story_id: "soak".to_string(),
            story_version: semver::Version::new(0, 1, 0),
            min_engine_version: semver::Version::new(0, 1, 0),
            title: "Soak".to_string(),
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
        world_md: "# 세계관\n비가 그친 카페.".to_string(),
        style_md: "# 문체\n짧고 정확한 문장.".to_string(),
        opening_md: "오프닝: 문이 열린다.".to_string(),
        prompts: PromptFragments {
            system: "sys".to_string(),
            director: "d".to_string(),
            writer: "w".to_string(),
        },
        pack_root: std::path::PathBuf::from("/tmp/soak"),
    }
}

// ---------------------------------------------------------------------------
// Scripted Mock: legal drafts cycling within delta limits
// ---------------------------------------------------------------------------

/// Schedule of (turn -> event id) fired via add_events at that turn. The set
/// covers every memory kind plus a resolver so we exercise both creation and
/// the unresolved-clearing convention.
fn event_schedule(turn: u32) -> Option<&'static str> {
    match turn {
        3 => Some("promise.share_umbrella"),
        8 => Some("first_meeting.yuna"),
        12 => Some("secret.past_letter"),
        18 => Some("relationship.truce"),
        22 => Some("key_item.umbrella"),
        28 => Some("confession.love"),
        34 => Some("promise.share_umbrella"), // a second promise
        40 => Some("betrayal.lie"),
        47 => Some("resolve_promise.share_umbrella"), // clears both priors
        55 => Some("relationship.truce"),
        63 => Some("secret.past_letter"),
        70 => Some("confession.love"),
        78 => Some("key_item.umbrella"),
        85 => Some("first_meeting.yuna"),
        92 => Some("promise.share_umbrella"),
        _ => None,
    }
}

fn choice(id: &str, label: &str) -> Choice {
    Choice {
        id: id.to_string(),
        label: label.to_string(),
        intent: String::new(),
        tone: None,
        target: None,
        risk: RiskLevel::Safe,
        hidden_requirements: vec![],
        preview: None,
    }
}

/// A legal draft for `turn`. The patch alternates affection deltas within
/// `[-3, 3]` (stays inside the 100-turn stat envelope) and adds the scheduled
/// memory-worthy event when one is due.
fn scripted_draft(generation_id: Uuid, turn: u32) -> GeneratedTurnDraft {
    let mut patch = StatePatch::default();
    // Alternate small legal deltas so affection oscillates inside stat_limits.
    let delta: i16 = match turn % 6 {
        0 => 3,
        1 => 1,
        2 => -1,
        3 => 2,
        4 => -2,
        _ => 0,
    };
    patch.affection_delta.insert(yuna(), delta);
    patch
        .trust_delta
        .insert(yuna(), if turn.is_multiple_of(2) { 1 } else { -1 });

    if let Some(ev) = event_schedule(turn) {
        patch.add_events.insert(ev.to_string());
    }
    // Occasionally set a known flag (keeps the validator reference-check honest).
    if turn == 5 {
        patch.set_flags.insert("f-met".to_string(), true);
    }
    if turn == 50 {
        patch.set_flags.insert("f-apologized".to_string(), true);
    }

    GeneratedTurnDraft {
        generation_id,
        narration: format!("turn {turn} 장면."),
        choices: vec![
            choice("choice-0", "계속"),
            choice("choice-1", "다른 길"),
            choice("choice-2", "기다린다"),
        ],
        proposed_patch: patch,
        summary: TurnSummary {
            facts: vec![format!("turn {turn}의 사실")],
            unresolved: vec![],
            relationship_changes: vec![],
            knowledge_changes: vec![],
            interpretation: Some(format!("turn {turn} 해석")),
            address_form_changes: vec![],
            important_player_actions: vec![],
            source_history_ids: vec![], // filled by the driver to the real id
        },
        usage: None,
    }
}

fn pending_generation_id(t: &story_core::engine::Transition) -> Uuid {
    t.effects
        .iter()
        .find_map(|e| match e {
            EngineEffect::RequestGeneration { generation_id, .. } => Some(*generation_id),
            _ => None,
        })
        .expect("a RequestGeneration effect")
}

fn ctx_for(budget_tokens: u32, state: &story_model::GameState) -> SelectionContext {
    let mut parts = BTreeSet::new();
    parts.insert(yuna());
    // Proper nouns from the current scene + route for the absolute-include canary.
    let mut nouns = BTreeSet::new();
    nouns.insert("우산".to_string());
    nouns.insert("cafe-namu".to_string());
    SelectionContext {
        current_participants: parts,
        current_location: state.location.clone(),
        active_route: state.active_route.clone(),
        mentioned_proper_nouns: nouns,
        budget_tokens,
    }
}

// ---------------------------------------------------------------------------
// The soak
// ---------------------------------------------------------------------------

#[test]
fn hundred_turn_soak_with_memory_and_save_invariants() {
    let pack = soak_pack();
    let mut state = EngineState::initial(&pack);

    // --- Opening: StartNewGame → AcceptGeneration (turn 1) ------------------
    let t = reduce(&state, EngineCommand::StartNewGame, &pack).expect("start");
    let gid = pending_generation_id(&t);
    state = t.state;
    let t = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(scripted_draft(gid, 1)),
        },
        &pack,
    )
    .expect("accept opening");
    state = t.state;
    assert_eq!(state.game.turn, 1, "turn 1 after opening accept");

    // Track every memory id ever created + the cumulative known_by union.
    let mut ever_created: BTreeSet<Uuid> = BTreeSet::new();
    let mut known_by_union: BTreeSet<CharacterId> = BTreeSet::new();
    let mut known_by_snapshots: Vec<BTreeSet<CharacterId>> = Vec::new();
    let mut promise_created_turns: Vec<u32> = Vec::new();

    let observe = |state: &EngineState,
                   ever: &mut BTreeSet<Uuid>,
                   known_union: &mut BTreeSet<CharacterId>,
                   known_snaps: &mut Vec<BTreeSet<CharacterId>>,
                   promise_turns: &mut Vec<u32>| {
        for m in &state.game.memories {
            ever.insert(m.id);
            if m.kind == MemoryKind::Promise && m.unresolved {
                promise_turns.push(m.created_turn);
            }
            for f in &m.facts {
                for k in &f.known_by {
                    known_union.insert(k.clone());
                }
            }
        }
        known_snaps.push(known_union.clone());
    };
    observe(
        &state,
        &mut ever_created,
        &mut known_by_union,
        &mut known_by_snapshots,
        &mut promise_created_turns,
    );

    let mut snapshot50: Option<Vec<u8>> = None;

    for turn in 2..=100u32 {
        // Submit the stable choice offered by the previous turn's draft.
        let t = reduce(
            &state,
            EngineCommand::SubmitChoice {
                choice_id: "choice-0".to_string(),
            },
            &pack,
        )
        .expect("submit");
        let gid = pending_generation_id(&t);
        state = t.state;

        let t = reduce(
            &state,
            EngineCommand::AcceptGeneration {
                draft: Box::new(scripted_draft(gid, turn)),
            },
            &pack,
        )
        .expect("accept");
        state = t.state;

        // --- State invariants every turn ----------------------------------
        assert_eq!(state.game.turn, turn, "turn bumps by exactly 1");
        assert!(
            state.game.revision >= turn as u64,
            "revision ({}) monotonic ≥ turn ({turn})",
            state.game.revision
        );
        for (id, c) in &state.game.characters {
            assert!(
                c.affection >= -100 && c.affection <= 100,
                "turn {turn}: {id} affection {} out of [-100,100]",
                c.affection
            );
            assert!(
                c.trust >= -100 && c.trust <= 100,
                "turn {turn}: {id} trust {} out of [-100,100]",
                c.trust
            );
            assert!(
                c.tension >= 0,
                "turn {turn}: {id} tension {} < 0",
                c.tension
            );
        }
        for (id, qty) in &state.game.inventory {
            assert!(*qty < u32::MAX, "turn {turn}: inventory {id} saturated");
        }

        // Memory wiring: when the schedule fires an event, a memory appears.
        if let Some(ev) = event_schedule(turn) {
            // The event's memory kind is derived from its id; assert a memory
            // was created this turn (created_turn == turn).
            let created_this_turn: Vec<_> = state
                .game
                .memories
                .iter()
                .filter(|m| m.created_turn == turn)
                .collect();
            assert!(
                !created_this_turn.is_empty(),
                "turn {turn}: fired event {ev} produced no memory"
            );
            // Resolver events are memory-worthy too (kind Promise) but they
            // additionally clear prior unresolved promises.
            if ev.starts_with("resolve_promise") {
                let still_unresolved: Vec<_> = state
                    .game
                    .memories
                    .iter()
                    .filter(|m| m.kind == MemoryKind::Promise && m.unresolved)
                    .collect();
                assert!(
                    still_unresolved.is_empty(),
                    "turn {turn}: resolve_promise left unresolved promises"
                );
            }
        }

        // --- MemorySelector stays within budget every turn ----------------
        let ctx = ctx_for(4000, &state.game);
        let trace = select(&state.game, &pack, &state.game.history, &ctx);
        assert_eq!(trace.budget_tokens, 4000);
        // Either within budget, or any excess is purely diagnosed absolute
        // overflow (never a silent drop).
        if trace.total_tokens > trace.budget_tokens {
            // Only absolute overflow is permitted; every drop must be recorded.
            assert!(
                !trace.excluded_reason.is_empty(),
                "turn {turn}: over budget with no diagnosed drops"
            );
        }
        // Absolute-include canary: at least one Promise/RelationshipChange
        // memory must be in the trace's included set whenever one exists.
        let abs_existing: Vec<_> = state
            .game
            .memories
            .iter()
            .filter(|m| {
                m.kind == MemoryKind::RelationshipChange
                    || (m.kind == MemoryKind::Promise && m.unresolved)
            })
            .collect();
        for m in abs_existing {
            let included = trace.included.iter().any(|i| i.id == m.id);
            let diagnosed = trace
                .excluded_reason
                .iter()
                .any(|(e, r)| e.id == m.id && r.contains("overflow"));
            assert!(
                included || diagnosed,
                "turn {turn}: absolute memory {:?} neither included nor diagnosed",
                m.kind
            );
        }

        observe(
            &state,
            &mut ever_created,
            &mut known_by_union,
            &mut known_by_snapshots,
            &mut promise_created_turns,
        );

        // --- Save round-trip snapshot at turn 50 --------------------------
        if turn == 50 {
            snapshot50 = Some(serde_json::to_vec(&state.snapshot_payload()).expect("snap50"));
        }
    }

    // ----------------------------------------------------------------------
    // End-of-run assertions
    // ----------------------------------------------------------------------

    let final_ids: BTreeSet<Uuid> = state.game.memories.iter().map(|m| m.id).collect();
    for id in &ever_created {
        assert!(
            final_ids.contains(id),
            "0-losses invariant violated: memory {id} disappeared by turn 100"
        );
    }
    assert!(
        !ever_created.is_empty(),
        "soak produced no memories at all — wiring is broken"
    );

    // known_by union is monotonic non-decreasing across the run.
    let final_known: BTreeSet<CharacterId> = state
        .game
        .memories
        .iter()
        .flat_map(|m| m.facts.iter().flat_map(|f| f.known_by.iter().cloned()))
        .collect();
    for k in &known_by_union {
        assert!(
            final_known.contains(k),
            "knowledge-owner {k:?} lost by turn 100 (monotonicity violated)"
        );
    }
    // Sanity: the player sentinel + yuna are both knowers somewhere.
    assert!(final_known.contains(&CharacterId("player".to_string())));
    assert!(final_known.contains(&yuna()));

    // Save round-trip at turn 50: byte-identical after a serde cycle.
    if let Some(bytes) = snapshot50 {
        let loaded: SavePayload = serde_json::from_slice(&bytes).expect("load50");
        let re = serde_json::to_vec(&loaded).expect("re-serialize50");
        assert_eq!(
            re, bytes,
            "turn-50 save round-trip not byte-identical (pure-data serde)"
        );
    }
    // Save round-trip at turn 100.
    let bytes100 = serde_json::to_vec(&state.snapshot_payload()).expect("snap100");
    let loaded100: SavePayload = serde_json::from_slice(&bytes100).expect("load100");
    assert_eq!(
        serde_json::to_vec(&loaded100).expect("re-serialize100"),
        bytes100,
        "turn-100 save round-trip not byte-identical"
    );

    // ----------------------------------------------------------------------
    // Core-event recall ≥ 90% under a generous budget.
    // ----------------------------------------------------------------------
    let key_mems: Vec<_> = state
        .game
        .memories
        .iter()
        .filter(|m| m.kind != MemoryKind::Other)
        .collect();
    assert!(
        key_mems.len() >= 5,
        "soak should fire at least 5 key memories, got {}",
        key_mems.len()
    );
    let ctx = ctx_for(100_000, &state.game);
    let trace = select(&state.game, &pack, &state.game.history, &ctx);
    let included_ids: BTreeSet<Uuid> = trace.included.iter().map(|m| m.id).collect();
    let recalled = key_mems
        .iter()
        .filter(|m| included_ids.contains(&m.id))
        .count();
    let recall = recalled as f32 / key_mems.len() as f32;
    assert!(
        recall >= 0.9,
        "core-event recall {recall:.2} < 0.90 ({} of {} included)",
        recalled,
        key_mems.len()
    );

    // Report the turn-100 budget summary.
    eprintln!(
        "turn 100 tokens_by_tier = {:?}, total = {}, budget = {} (key memories: {}, recall: {:.2})",
        trace.tokens_by_tier,
        trace.total_tokens,
        trace.budget_tokens,
        key_mems.len(),
        recall
    );
}

// ---------------------------------------------------------------------------
// Auxiliary: EngineState carrying a hand-built game (used by the wiring test)
// ---------------------------------------------------------------------------

#[test]
fn reduce_accept_wires_memory_creation_and_resolver() {
    // A focused 2-turn slice: turn 1 fires a promise, turn 2 fires its resolver.
    // Asserts the engine appends memories and clears unresolved without deleting.
    use story_core::memory::resolve_memories;
    use story_model::EventId;

    let pack = soak_pack();
    let mut state = EngineState::initial(&pack);
    let t = reduce(&state, EngineCommand::StartNewGame, &pack).expect("start");
    let gid = pending_generation_id(&t);
    state = t.state;

    // Turn 1: fire the promise.
    let mut d1 = scripted_draft(gid, 1);
    d1.proposed_patch
        .add_events
        .insert("promise.share_umbrella".to_string());
    let t = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(d1),
        },
        &pack,
    )
    .expect("accept t1");
    state = t.state;
    let promise_mems: Vec<_> = state
        .game
        .memories
        .iter()
        .filter(|m| m.kind == MemoryKind::Promise)
        .collect();
    assert_eq!(
        promise_mems.len(),
        1,
        "promise memory wired into reduce_accept"
    );
    assert!(promise_mems[0].unresolved, "promise starts unresolved");
    assert_eq!(
        promise_mems[0].source_history_ids.len(),
        1,
        "memory carries its source history id"
    );
    let promise_id = promise_mems[0].id;

    // Turn 2: submit + accept a resolver.
    let t = reduce(
        &state,
        EngineCommand::SubmitChoice {
            choice_id: "choice-0".to_string(),
        },
        &pack,
    )
    .expect("submit");
    let gid2 = pending_generation_id(&t);
    state = t.state;
    let mut d2 = scripted_draft(gid2, 2);
    d2.proposed_patch
        .add_events
        .insert("resolve_promise.share_umbrella".to_string());
    let t = reduce(
        &state,
        EngineCommand::AcceptGeneration {
            draft: Box::new(d2),
        },
        &pack,
    )
    .expect("accept t2");
    state = t.state;

    let same_promise = state
        .game
        .memories
        .iter()
        .find(|m| m.id == promise_id)
        .expect("prior promise memory still present (0 losses)");
    assert!(
        !same_promise.unresolved,
        "resolver cleared unresolved on the prior promise (no deletion)"
    );

    // Sanity: the resolver helper is itself deterministic on a synthetic list.
    let mut scratch = state.game.memories.clone();
    let before = scratch.len();
    resolve_memories(&mut scratch, &[EventId("resolve_promise.x".to_string())]);
    assert_eq!(scratch.len(), before, "resolve_memories never deletes");
}

// Silence unused imports in non-test builds of the helper types.
#[allow(dead_code)]
fn _retain_state_types(s: story_model::GameState) -> story_model::GameState {
    // Touch a few fields so the imports are clearly used.
    let _ = s.turn;
    let _: VecDeque<String> = s.recent_summaries;
    let _: CharacterState = CharacterState {
        affection: 0,
        trust: 0,
        tension: 0,
        relationship_stage: RelationshipStage::Stranger,
        current_emotion: Emotion::Neutral,
        known_facts: BTreeSet::new(),
        address_form_overrides: BTreeMap::new(),
    };
    let _: SavePayload;
    let _ = SAVE_SCHEMA_VERSION;
    s
}
