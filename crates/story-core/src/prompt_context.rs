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

use std::collections::BTreeSet;

use sha2::{Digest, Sha256};
use story_model::serde_json;
use story_model::{Choice, GameState, HistoryEntry, MemoryEntry, ParsedAction, StoryPack};

use crate::memory_selector::{SelectionContext, select};
use crate::narration_quality::recurring_motifs;

/// Keep the immediately-played context crisp. Longer-lived information comes
/// back through the scored memory recall below instead of growing forever.
const RECENT_TURN_WINDOW: usize = 2;
/// A deliberately conservative slice of the provider context for recalled
/// memories. The provider still applies its final context bound afterwards.
const MEMORY_RECALL_BUDGET_TOKENS: u32 = 6_000;

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
    bundle_from(system, user)
}

/// Hash `system` + `user` into a [`PromptBundle`].
fn bundle_from(system: String, user: String) -> PromptBundle {
    let mut hasher = Sha256::new();
    hasher.update(system.as_bytes());
    hasher.update(b"\n\n---\n\n");
    hasher.update(user.as_bytes());
    let hash = hex_lower(hasher.finalize().as_slice());
    PromptBundle { system, user, hash }
}

// ---------------------------------------------------------------------------
// side-generation prompts (continue / suggest)
// ---------------------------------------------------------------------------

/// Fixed Korean instruction for continuation requests: keep writing the
/// previous narration, do not open a new turn. The engine accepts only the
/// `narration` field of the JSON draft.
const CONTINUE_SYSTEM_INSTRUCTION: &str = "\
# Continue narration (engine-owned)\n\
이전 내레이션을 그대로 이어서 같은 장면의 뒷부분을 계속 쓴다. 새로운 턴을 시작하지 않고 \
오프닝으로 되돌아가지 않는다. 이전 내레이션의 문장을 베끼지 않는다. 응답은 기존 JSON \
계약(narration, choices, state_patch, summary)을 유지하되, 엔진은 narration만 수용하고 \
나머지 필드는 버린다.\n";

/// Fixed Korean instruction for suggested-action requests: one Korean
/// sentence describing a suggested PLAYER action. The suggestion is an
/// example only — the player decides (sovereignty). The engine accepts only
/// the `narration` field.
const SUGGEST_SYSTEM_INSTRUCTION: &str = "\
# Suggest action (engine-owned)\n\
플레이어가 다음에 취할 행동 한 문장(한국어)만 제안한다. 이 제안은 예시일 뿐이며 \
플레이어가 따를 의무가 없다(플레이어 주권). 감정, 대사, 접촉, 용서, 고백, 결단을 \
플레이어 대신 확정하는 표현을 쓰지 않는다. 응답 JSON의 narration 필드에 그 한 문장만 \
넣고, choices/state_patch/summary는 형식 유지용 복사본이면 충분하다(엔진이 버린다).\n";

/// Assemble a deterministic prompt for
/// [`crate::engine_cmd::EngineCommand::ContinueNarration`]: the standard
/// turn context plus the fixed continuation instruction and the previous
/// narration (with the committed choices/patch echoed so the model can fill
/// the JSON contract it will not be judged on). Pure function of its inputs.
pub fn build_continue_prompt(
    pack: &StoryPack,
    state: &GameState,
    recent: &[HistoryEntry],
) -> PromptBundle {
    let base = build_prompt(pack, state, recent, None);
    let mut system = base.system;
    system.push('\n');
    system.push_str(CONTINUE_SYSTEM_INSTRUCTION);

    let mut user = base.user;
    user.push_str("\n\n# Continue target (engine-owned)\n");
    user.push_str(
        "이전 내레이션을 그대로 이어서 두 문단 이내로 계속한다. 아래 텍스트를 반복하지 않는다.\n",
    );
    if let Some(entry) = recent.last() {
        user.push_str(&format!(
            "\n## turn {} narration (이전 내레이션 — 이 바로 뒤에서 계속)\n",
            entry.turn
        ));
        user.push_str(&entry.narration);
        user.push('\n');
        user.push_str(
            "\n응답 형식 유지를 위해 아래 현재 값을 그대로 복사한다(엔진은 narration만 수용한다).\n",
        );
        user.push_str(&format!(
            "choices: {}\n",
            serde_json::to_string(&entry.choices).unwrap_or_else(|_| "[]".to_string())
        ));
        user.push_str(&format!(
            "state_patch: {}\n",
            serde_json::to_string(&entry.applied_patch).unwrap_or_else(|_| "{}".to_string())
        ));
    }
    bundle_from(system, user)
}

/// Assemble a deterministic prompt for
/// [`crate::engine_cmd::EngineCommand::SuggestAction`]: the standard turn
/// context plus the fixed one-sentence player-action suggestion instruction
/// with its player-sovereignty clause. Pure function of its inputs.
pub fn build_suggest_prompt(
    pack: &StoryPack,
    state: &GameState,
    recent: &[HistoryEntry],
) -> PromptBundle {
    let base = build_prompt(pack, state, recent, None);
    let mut system = base.system;
    system.push('\n');
    system.push_str(SUGGEST_SYSTEM_INSTRUCTION);

    let mut user = base.user;
    user.push_str("\n\n# Suggested player action (engine-owned)\n");
    user.push_str(
        "플레이어가 다음에 취할 행동 한 문장(한국어)을 narration 필드에 제안한다. \
         질문이 아닌 행동형 문장으로, 플레이어가 지금 실행할 동사로 끝낸다.\n",
    );
    bundle_from(system, user)
}

/// Fixed Korean instruction for group-scene requests: two or more cast
/// members talk to each other, every spoken line uses the system prompt's
/// speaker-clarity line format, the player's own words and feelings are
/// never decided for them, and the JSON draft shape is kept only so the
/// provider's recovery parser works — the engine accepts narration alone and
/// discards the format-preserving copies.
const GROUP_SCENE_SYSTEM_INSTRUCTION: &str = "\
# Group scene (engine-owned)\n\
두 명 이상의 등장인물이 이어서 서로 대화하는 장면 한 비트를 쓴다. 각 발화 줄은 \
시스템 프롬프트의 화자 명확성 규칙 그대로 `이름: \"대사\"` 한 줄 형식으로 쓰고, \
이미 확립된 캐릭터 이름 밖의 새 화자를 만들지 않는다. 플레이어의 대사·감정·행동을 \
대신 정하지 않는다. 응답은 기존 JSON 계약(narration, choices, state_patch, \
summary)을 유지하되 choices/state_patch/summary는 형식 유지용 복사본이면 충분하고 \
엔진은 narration만 수용해 나머지는 버린다.\n";

/// Assemble a deterministic prompt for
/// [`crate::engine_cmd::EngineCommand::GroupScene`]: the standard turn
/// context plus the fixed group-conversation instruction, the cast list, and
/// the previous narration the scene bit continues from. Pure function of its
/// inputs.
pub fn build_group_scene_prompt(
    pack: &StoryPack,
    state: &GameState,
    recent: &[HistoryEntry],
) -> PromptBundle {
    let base = build_prompt(pack, state, recent, None);
    let mut system = base.system;
    system.push('\n');
    system.push_str(GROUP_SCENE_SYSTEM_INSTRUCTION);

    let mut user = base.user;
    user.push_str("\n\n# Group scene (engine-owned)\n");
    user.push_str(
        "아래 장면에서 두 명 이상의 등장인물이 이어서 대화한다. 각 발화 줄은 \
         `이름: \"대사\"` 한 줄 형식을 유지한다.\n",
    );
    // Deterministic cast list: state key order joined with pack display
    // names (state-only characters keep their id).
    let cast = state
        .characters
        .keys()
        .map(|id| {
            pack.characters
                .get(id)
                .map(|character| character.name.as_str())
                .unwrap_or(id.0.as_str())
        })
        .collect::<Vec<_>>()
        .join(", ");
    if cast.is_empty() {
        user.push_str("대화 가능한 인물: 없음\n");
    } else {
        user.push_str(&format!("대화 가능한 인물: {cast}\n"));
    }
    if let Some(entry) = recent.last() {
        user.push_str(&format!(
            "\n## turn {} narration (이전 내레이션 — 이 장면에 이어서 대화)\n",
            entry.turn
        ));
        user.push_str(&entry.narration);
        user.push('\n');
        user.push_str(
            "\n응답 형식 유지를 위해 아래 현재 값을 그대로 복사한다(엔진은 narration만 수용한다).\n",
        );
        user.push_str(&format!(
            "choices: {}\n",
            serde_json::to_string(&entry.choices).unwrap_or_else(|_| "[]".to_string())
        ));
        user.push_str(&format!(
            "state_patch: {}\n",
            serde_json::to_string(&entry.applied_patch).unwrap_or_else(|_| "{}".to_string())
        ));
    }
    bundle_from(system, user)
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

    // Per-turn delta limits and cumulative stat bounds.
    let dl = &pack.rules.delta_limits;
    let al = pack.rules.affection_stat_limits();
    let tl = pack.rules.trust_stat_limits();
    let nl = pack.rules.tension_stat_limits();
    s.push_str(&format!(
        "\n# Delta limits (per turn, inclusive)\n\
         affection: {a_min}..={a_max}\n\
         trust: {t_min}..={t_max}\n\
         tension: {n_min}..={n_max}\n\
         # Stat bounds (total after delta, inclusive)\n\
         affection: {as_min}..={as_max}\n\
         trust: {ts_min}..={ts_max}\n\
         tension: {ns_min}..={ns_max}\n\
         Never emit a delta that pushes `current + delta` below min or above max (e.g., if `tension=0` and min is `0`, `tension_delta` must be `>= 0`).\n",
        a_min = dl.affection.min,
        a_max = dl.affection.max,
        t_min = dl.trust.min,
        t_max = dl.trust.max,
        n_min = dl.tension.min,
        n_max = dl.tension.max,
        as_min = al.min,
        as_max = al.max,
        ts_min = tl.min,
        ts_max = tl.max,
        ns_min = nl.min,
        ns_max = nl.max,
    ));
    s.push_str(
        "\n# Relationship state update policy\n\
         After each scene, inspect the observable consequence for every involved character. \
         When the scene credibly changes warmth, reliability, or immediate strain, put a small \
         signed integer in `state_patch.affection_delta`, `trust_delta`, or `tension_delta` for \
         that character. Do not leave all three maps empty merely because the scene is subtle. \
         Use `{}` only when no observable relationship state changed. Never invent a change just \
         to make a meter move; narration and the patch must describe the same consequence.\n",
    );

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
        "\n# Speaker clarity\n\
         Narration is written from a second-person camera: `you` refers only to the player. \
         Never invent a direct quote, inner thought, or spoken reply for the player. The UI \
         already records the player's selected action separately. Put every NPC direct quote on \
         its own line in the exact visible form `Character name: \"dialogue\"` (or the \
         Korean equivalent punctuation). Use an established character name and never invent a \
         new speaker. This is a writing and display rule, not a JSON field.\n",
    );
    s.push_str(
        "\n# Working and display language\n\
         You may use English internally for analysis, retrieval keywords, and planning when it \
         improves understanding. Do not expose that working text or a chain of thought. Every \
         player-visible string in the JSON response, including narration and choice labels, must \
         be natural Korean unless the story content itself requires a foreign proper noun.\n",
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
    let full = recent;
    // Lorebook selection happens first: constant entries join the stable
    // prefix below, triggered entries form the turn-variable lore block.
    let lore = crate::lorebook::select_lore(pack, state, full, player_action, player_choice);
    let recent_start = full.len().saturating_sub(RECENT_TURN_WINDOW);
    // `earlier` is the narrative arc before the crisp recent window. It is
    // rendered below as a compact recap so the model keeps long-term continuity
    // even when event-driven memory recall is empty (the common case).
    let earlier = &full[..recent_start];
    let recent = &full[recent_start..];
    let recall = select(
        state,
        pack,
        // Recent turns are rendered below as a fixed, small window. Passing
        // them to the selector too would let their intentionally higher score
        // consume the entire recall budget before any long-term memory could
        // be selected.
        &[],
        &SelectionContext {
            current_participants: state.characters.keys().cloned().collect(),
            current_location: state.location.clone(),
            active_route: state.active_route.clone(),
            mentioned_proper_nouns: mentioned_proper_nouns(state, player_action, player_choice),
            budget_tokens: MEMORY_RECALL_BUDGET_TOKENS,
        },
    );

    s.push_str("# World\n");
    s.push_str(&pack.world_md);
    s.push_str("\n\n# Style\n");
    s.push_str(&pack.style_md);
    // Constant `before_char` lore: turn-invariant, so it stays in the stable
    // prefix at its canonical position.
    if !lore.stable_before.is_empty() {
        s.push('\n');
        s.push_str(&crate::lorebook::render_lore_block(&lore.stable_before));
    }

    // Keep pack-owned, turn-invariant material at the front of the user
    // message. Local runtimes can reuse this stable prefix across turns while
    // all state-, history-, and recall-dependent material stays below it.
    s.push_str("\n\n# Character canon and voice (pack-owned)\n");
    s.push_str(&character_canon_context(pack));
    // Constant `after_char` lore keeps its stable-prefix position; the
    // keyword-triggered entries follow in one turn-variable block (module
    // docs explain why they are not split back into the prefix).
    if !lore.stable_after.is_empty() {
        s.push('\n');
        s.push_str(&crate::lorebook::render_lore_block(&lore.stable_after));
    }
    if !lore.triggered.is_empty() {
        s.push('\n');
        s.push_str(&crate::lorebook::render_lore_block(&lore.triggered));
    }

    s.push_str("\n# Story progression rules (engine-authoritative)\n");
    s.push_str(&progression_context(pack));

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

    // Long-term continuity: a compact recap of every turn older than the recent
    // window. Without this the model only sees the last few turns plus state
    // numbers, and tends to repeat or regress earlier beats — especially weaker
    // local models. The recap is bounded; the provider applies its final context
    // bound afterwards.
    s.push_str(&earlier_recap(full, earlier.len(), &state.recent_summaries));

    s.push_str("\n\n# Current state\n");
    s.push_str(&state_snapshot_json(state));

    s.push_str("\n\n# Relationship ledger (engine-owned)\n");
    s.push_str(&relationship_ledger(state));

    s.push_str("\n# Live character knowledge (engine-owned)\n");
    s.push_str(&character_live_context(pack, state));

    s.push_str("\n# Current dramatic beat\n");
    s.push_str(&dramatic_brief(pack, state, &recall.included));
    s.push_str("\n# Autonomous world and NPC initiative\n");
    s.push_str(&world_initiative(pack, state));

    s.push_str("\n# Recent turns\n");
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

    // Compact semantic digest of the recent arc (model interpretation when
    // stored, else narration first clause) plus an explicit repetition flag.
    // Replaces the earlier surface-level anti-repeat phrase list — strictly
    // richer.
    s.push_str(&recent_arc_block(state, full));

    if !recall.included.is_empty() {
        s.push_str("\n# Recalled long-term memory\n");
        s.push_str(&memory_context(&recall.included));
    }

    s.push_str("\n# Engine-owned choice design\n");
    s.push_str(&crate::choice_quality::design_prompt(
        state,
        pack,
        player_choice,
    ));

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
            // Targeted free input (`@캐릭터 내용`) — pin the addressee so the
            // reply is mandatory and other cast members react around it.
            // Present only when a target exists, so plain free-action
            // prompts stay byte-identical.
            s.push_str(&format!(
                "{}은 플레이어가 직접 말 건 캐릭터 — 응답 필수, 타 인물 반응 자연 연결\n",
                t.0
            ));
        }
        if let Some(c) = pa.confidence {
            s.push_str(&format!("confidence: {c:.2}\n"));
        }
        s.push_str(&format!("content: {}\n", pa.content));
        s.push_str(&format!("original_text: {}\n", pa.original_text));
    }

    s
}

fn character_canon_context(pack: &StoryPack) -> String {
    let mut out = String::new();
    out.push_str(
        "Treat voice, canonical knowledge, goals, and boundaries below as canon. Unknown facts and secrets are private: do not reveal or let a character act on them unless the live state or an authorized event has established that knowledge.\n",
    );
    for (id, def) in &pack.characters {
        out.push_str(&format!("## {} ({})\n", def.name, id.0));
        if let Some(age) = def.age {
            out.push_str(&format!("age: {age}\n"));
        }
        if let Some(description) = def
            .description_md
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            out.push_str("description: |\n");
            for line in description.lines() {
                out.push_str(&format!("  {line}\n"));
            }
        }
        append_list(&mut out, "personality", &def.personality);
        append_list(&mut out, "appearance", &def.appearance);
        append_list(&mut out, "values", &def.values);
        append_list(&mut out, "likes", &def.likes);
        append_list(&mut out, "dislikes", &def.dislikes);
        append_value(&mut out, "speech_style", &def.speech_style);
        append_value(&mut out, "honorific_rule", &def.honorific_rule);
        append_list(&mut out, "frequent_phrases", &def.frequent_phrases);
        append_list(&mut out, "avoid_phrases", &def.avoid_phrases);
        if !def.dialogue_examples.is_empty() {
            out.push_str("dialogue_examples:\n");
            for example in &def.dialogue_examples {
                out.push_str("  - |\n");
                for line in example.lines() {
                    out.push_str(&format!("    {line}\n"));
                }
            }
        }
        append_value(&mut out, "current_goal", &def.current_goal);
        append_value(&mut out, "long_term_desire", &def.long_term_desire);
        append_list(&mut out, "canonical_known_facts", &def.known_facts);
        append_list(&mut out, "PRIVATE_unknown_facts", &def.unknown_facts);
        append_list(&mut out, "PRIVATE_secrets", &def.secrets);
    }
    out
}

fn character_live_context(pack: &StoryPack, state: &GameState) -> String {
    let mut out = String::from(
        "These are facts and forms of address established during play. They override neither private canon nor the relationship ledger.\n",
    );
    let mut included = 0_usize;
    for (id, live) in &state.characters {
        if live.known_facts.is_empty() && live.address_form_overrides.is_empty() {
            continue;
        }
        included += 1;
        let name = pack
            .characters
            .get(id)
            .map(|character| character.name.as_str())
            .unwrap_or(id.0.as_str());
        out.push_str(&format!("## {name} ({})\n", id.0));
        let known: Vec<String> = live.known_facts.iter().cloned().collect();
        append_list(&mut out, "live_known_facts", &known);
        if !live.address_form_overrides.is_empty() {
            out.push_str(&format!(
                "live_address_forms: {}\n",
                serde_json::to_string(&live.address_form_overrides)
                    .unwrap_or_else(|_| "{}".to_string())
            ));
        }
    }
    if included == 0 {
        out.push_str("(no additional live character knowledge)\n");
    }
    out
}

fn append_list(out: &mut String, label: &str, values: &[String]) {
    if !values.is_empty() {
        out.push_str(label);
        out.push_str(": ");
        out.push_str(&values.join(" | "));
        out.push('\n');
    }
}

fn append_value(out: &mut String, label: &str, value: &str) {
    if !value.trim().is_empty() {
        out.push_str(label);
        out.push_str(": ");
        out.push_str(value);
        out.push('\n');
    }
}

fn progression_context(pack: &StoryPack) -> String {
    let mut out = String::new();
    out.push_str(
        "`state_patch.add_events` only proposes an event. Propose an id only when its trigger is true after this turn's patch and its route lock matches. Rust re-checks every condition and records only fired events.\n",
    );
    for (id, event) in &pack.rules.events {
        out.push_str(&format!(
            "event {}: trigger={}; route_lock={}; effects={}\n",
            id.0,
            serde_json::to_string(&event.trigger).unwrap_or_else(|_| "null".to_string()),
            event.route_lock.as_deref().unwrap_or("none"),
            serde_json::to_string(&event.effects).unwrap_or_else(|_| "{}".to_string()),
        ));
    }
    for (id, route) in &pack.rules.routes {
        out.push_str(&format!(
            "route {id}: enter_condition={}; premise={}\n",
            serde_json::to_string(&route.enter_condition).unwrap_or_else(|_| "null".to_string()),
            route.premise,
        ));
    }
    for (id, ending) in &pack.rules.endings {
        out.push_str(&format!(
            "ending {id}: condition={}; label={}\n",
            serde_json::to_string(&ending.condition).unwrap_or_else(|_| "null".to_string()),
            ending.label,
        ));
    }
    out
}

fn mentioned_proper_nouns(
    state: &GameState,
    player_action: Option<&ParsedAction>,
    player_choice: Option<&Choice>,
) -> BTreeSet<String> {
    let mut nouns = BTreeSet::new();
    if let Some(last) = &state.last_choice {
        nouns.insert(last.label.clone());
    }
    if let Some(action) = player_action {
        nouns.insert(action.content.clone());
        nouns.insert(action.original_text.clone());
    }
    if let Some(choice) = player_choice {
        nouns.insert(choice.label.clone());
        nouns.insert(choice.intent.clone());
        if let Some(target) = &choice.target {
            nouns.insert(target.0.clone());
        }
    }
    nouns.retain(|noun| !noun.trim().is_empty() && noun.chars().count() <= 80);
    nouns
}

fn relationship_ledger(state: &GameState) -> String {
    let mut ledger = String::new();
    for (id, character) in &state.characters {
        ledger.push_str(&format!(
            "- {}: stage={:?}; affection={}; trust={}; tension={}; emotion={:?}",
            id.0,
            character.relationship_stage,
            character.affection,
            character.trust,
            character.tension,
            character.current_emotion,
        ));
        if !character.address_form_overrides.is_empty() {
            let forms = character
                .address_form_overrides
                .iter()
                .map(|(target, form)| format!("{}={}", target.0, form))
                .collect::<Vec<_>>();
            ledger.push_str(&format!("; address_forms={}", forms.join(", ")));
        }
        ledger.push('\n');
    }
    if ledger.is_empty() {
        ledger.push_str("(no active character relationship state)\n");
    }
    ledger
}

fn dramatic_brief(pack: &StoryPack, state: &GameState, recalled: &[MemoryEntry]) -> String {
    let unresolved = recalled.iter().find(|memory| memory.unresolved);
    let mode = if state.turn % 3 == 2 {
        "Sequel"
    } else {
        "Scene"
    };
    let mut brief = match unresolved.and_then(memory_summary) {
        Some(thread) => format!(
            "mode={mode}. Advance exactly one observable beat around this unresolved thread: {thread}. Do not resolve it completely unless a validated event makes that possible. End with a concrete new pressure, question, or consequence.\n"
        ),
        None => format!(
            "mode={mode}. Advance exactly one observable relationship or situation beat at {}. Do not restart the premise or repeat the last selected approach. End with a concrete new pressure, question, or consequence.\n",
            state.location.0,
        ),
    };
    brief.push_str(&format!(
        "\n# 필수 전진 (engine)\n{}\n금지: 최근과 같은 질문 유형(왜 불렀는지/왜 떠났는지/정해 두고 묻는지)을 다시 묻기.\n",
        crate::narrative::required_lead(state, pack)
    ));
    brief
}

fn world_initiative(pack: &StoryPack, state: &GameState) -> String {
    let phase = crate::narrative::phase(state);
    let pressure =
        crate::narrative::pressure(state).unwrap_or("move the current situation forward");
    let mut text = format!(
        "World clock: turn {} is in the {phase} phase; the next pressure is due by turn {}: {pressure}. Before offering choices, let the setting or an NPC take one observable initiative; do not wait for the player to invent all momentum.\n",
        state.turn,
        crate::narrative::due_turn(state)
    );
    for (id, character) in &pack.characters {
        if state.characters.contains_key(id) && !character.current_goal.trim().is_empty() {
            let agenda = crate::narrative::agenda(state, &id.0).unwrap_or(&character.current_goal);
            text.push_str(&format!("- {} pursues: {}\n", character.name, agenda));
        }
    }
    text.push_str("NPC initiative may create pressure, offer, refusal, departure, or new information, but must not decide the player's response, feelings, speech, or irreversible commitment.\n");
    text
}

fn memory_context(memories: &[MemoryEntry]) -> String {
    let mut text = String::new();
    for memory in memories {
        text.push_str("- ");
        text.push_str(&format!("{:?}", memory.kind));
        if memory.unresolved {
            text.push_str(" [unresolved]");
        }
        if let Some(summary) = memory_summary(memory) {
            text.push_str(": ");
            text.push_str(summary);
        }
        text.push('\n');
    }
    text
}

fn memory_summary(memory: &MemoryEntry) -> Option<&str> {
    memory
        .interpretation
        .as_deref()
        .filter(|text| !text.trim().is_empty())
        .or_else(|| memory.facts.first().map(|fact| fact.object.as_str()))
}

/// Cap the number of earlier turns rendered, so a long session cannot grow the
/// recap without bound. The most recent earlier turns matter most for immediate
/// continuity; the engine's state snapshot persists older consequences.
const EARLIER_RECAP_MAX_ENTRIES: usize = 12;
/// Per-entry narration cap. The recap is continuity, not replay: one beat per
/// earlier turn is enough to prevent regression/repetition.
const EARLIER_RECAP_PER_NARRATION: usize = 160;
/// Semantic digests can carry a little more detail than raw prose because they
/// are already condensed to the consequence of the turn.
const EARLIER_RECAP_PER_SUMMARY: usize = 240;

/// Render a compact, bounded recap of the turns before the recent window.
/// Returns an empty string when there is nothing earlier (keeps the prompt
/// byte-identical for short sessions, so the golden snapshot is unaffected).
fn earlier_recap(
    full: &[HistoryEntry],
    earlier_len: usize,
    summaries: &std::collections::VecDeque<String>,
) -> String {
    if earlier_len == 0 {
        return String::new();
    }
    let earlier = &full[..earlier_len];
    let start = earlier_len.saturating_sub(EARLIER_RECAP_MAX_ENTRIES);
    let mut s = String::from(
        "\n\n# Earlier story (recap — continuity only; do not reproduce these lines)\n",
    );
    if start > 0 {
        // Older turns are omitted; flag it so the model knows the arc is longer.
        s.push_str(&format!(
            "(turns before {} are omitted; rely on the current state for their consequences)\n",
            earlier[start].turn
        ));
    }
    for (index, h) in earlier.iter().enumerate().skip(start) {
        s.push_str(&format!("## turn {} ({})\n", h.turn, h.scene_id));
        let digest = stored_summary_for_history_index(summaries, full.len(), index)
            .filter(|summary| !summary.trim().is_empty())
            .map(|summary| truncate_prose(summary, EARLIER_RECAP_PER_SUMMARY))
            .unwrap_or_else(|| truncate_prose(&h.narration, EARLIER_RECAP_PER_NARRATION));
        s.push_str(&digest);
        s.push('\n');
        if let Some(act) = &h.player_action {
            s.push_str(&format!("action: {}\n", act.content));
        }
    }
    s
}

/// Trim prose to at most `max_chars` characters (char-based, UTF-8 safe),
/// appending an ellipsis when truncated. Whitespace is collapsed for a stable
/// single-line beat.
fn truncate_prose(text: &str, max_chars: usize) -> String {
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = collapsed.chars().count();
    if count <= max_chars {
        return collapsed;
    }
    let head: String = collapsed.chars().take(max_chars).collect();
    format!("{head}…")
}

/// Number of most-recent digests rendered in the `# Recent arc` block. Matches
/// the full-prose recent window so each of those turns gets a one-line semantic
/// companion.
const RECENT_ARC_RENDER_TURNS: usize = 4;

/// Render `# Recent arc` (last [`RECENT_ARC_RENDER_TURNS`] digests) and, only
/// when a motif is shared across at least two recent full narrations, a
/// `# 반복 감지` note. Returns an empty string when there is no history (keeps
/// byte-identical prompts for the very first turn).
fn recent_arc_block(state: &GameState, full: &[HistoryEntry]) -> String {
    if full.is_empty() {
        return String::new();
    }
    let stored = &state.recent_summaries;
    // `recent_summaries` is a bounded FIFO, so after its cap is reached it
    // aligns with the tail of full history rather than having the same length.
    // Fall back to narration for old saves without a digest at this index.
    let digest_for = |i: usize| -> String {
        let stored_entry = stored_summary_for_history_index(stored, full.len(), i);
        if let Some(entry) = stored_entry {
            let trimmed = entry.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
        first_clause(&full[i].narration, 120)
    };

    let mut s = String::from("\n# Recent arc\n");
    let render_start = full.len().saturating_sub(RECENT_ARC_RENDER_TURNS);
    for (i, entry) in full.iter().enumerate().skip(render_start) {
        s.push_str(&format!("## turn {} ({})\n", entry.turn, entry.scene_id));
        s.push_str(&digest_for(i));
        s.push('\n');
    }

    // Repetition check over complete recent narrations, not only the stored
    // digest or first sentence. Weak local models often move recurring props
    // (clock/window/gaze) into later paragraphs where a digest scan misses
    // them.
    let motifs = recurring_motifs(full);
    if !motifs.is_empty() {
        s.push_str(&format!(
            "\n# 반복 감지\n\
             최근 흐름에서 이 표현/소재가 반복되고 있다: {}.\n\
             같은 분위기·같은 묘사로 되풀이하지 말고, 한 가지 새로운 관찰 가능한 압력이나 방향으로 전진시킨다.\n",
            motifs.join(", ")
        ));
    }
    s
}

/// Map a full-history index onto the bounded summary FIFO. Summaries are
/// appended once per accepted turn and trimmed from the front, so a shorter
/// queue always represents the newest `queue.len()` history entries.
fn stored_summary_for_history_index(
    summaries: &std::collections::VecDeque<String>,
    history_len: usize,
    history_index: usize,
) -> Option<&str> {
    if summaries.is_empty() || history_index >= history_len {
        return None;
    }
    let stored_len = summaries.len().min(history_len);
    let stored_start = history_len - stored_len;
    history_index
        .checked_sub(stored_start)
        .and_then(|index| summaries.get(index))
        .map(String::as_str)
}

/// Extract the opening clause of a narration: up to the first sentence ender
/// (`.`, `。`, `!`, `?`, or a newline), capped at `max_chars` with an ellipsis.
pub fn first_clause(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    let end = trimmed
        .find(['.', '。', '!', '?', '\n'])
        .unwrap_or(trimmed.len());
    let clause = trimmed[..end].trim();
    let count = clause.chars().count();
    if count == 0 {
        return String::new();
    }
    if count <= max_chars {
        return clause.to_string();
    }
    let head: String = clause.chars().take(max_chars).collect();
    format!("{head}…")
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
        m.insert(
            "known_facts".to_string(),
            serde_json::to_value(&c.known_facts).expect("known facts serialize"),
        );
        m.insert(
            "address_form_overrides".to_string(),
            serde_json::to_value(&c.address_form_overrides).expect("address forms serialize"),
        );
        characters.insert(id.0.clone(), serde_json::Value::Object(m));
    }

    let root = serde_json::json!({
        "active_route": state.active_route,
        "current_chapter": state.current_chapter,
        "current_scene_id": state.current_scene_id,
        "location": state.location,
        "time_slot": state.time_slot,
        "turn": state.turn,
        "characters": characters,
        "flags": state.flags,
        "numbers": state.numbers,
        "strings": state.strings,
        "inventory": state.inventory,
        "occurred_events": state.occurred_events,
        "last_choice": state.last_choice,
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
    use std::collections::{BTreeMap, BTreeSet, VecDeque};
    use story_model::{
        CharacterState, Emotion, GameState, LocationId, MemoryEntry, MemoryKind, RelationshipStage,
        SAVE_SCHEMA_VERSION, StoryPack,
    };

    fn minimal_fixture() -> (StoryPack, GameState) {
        let pack = StoryPack {
            lorebook: story_model::Lorebook::default(),
            metadata: story_model::PackMetadata {
                schema_version: semver::Version::parse("1.0.0").unwrap(),
                story_id: "t".to_string(),
                story_version: semver::Version::new(0, 1, 0),
                min_engine_version: semver::Version::new(0, 1, 0),
                title: "t".to_string(),
                language: "ko-KR".to_string(),
                genres: vec![],
                rating: story_model::PackRating::General,
                default_choice_count: 3,
                initial_state: story_model::PackInitialState {
                    chapter: "c".to_string(),
                    scene: "s".to_string(),
                    location: LocationId("l".to_string()),
                    time_slot: story_model::TimeSlot::Morning,
                },
                content: story_model::PackContentPolicy {
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
            lorebook_activations: Vec::new(),
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

    #[test]
    fn system_prompt_makes_npc_speakers_visible_without_adding_a_json_field() {
        let (pack, state) = minimal_fixture();
        let bundle = build_prompt(&pack, &state, &[], None);

        assert!(bundle.system.contains("# Speaker clarity"));
        assert!(bundle.system.contains("Character name: \"dialogue\""));
        assert!(bundle.system.contains("not a JSON field"));
        assert!(bundle.system.contains("# Working and display language"));
        assert!(bundle.system.contains("must be natural Korean"));
    }

    #[test]
    fn prompt_includes_recalled_memory_relationship_ledger_and_distinct_choice_roles() {
        let (pack, mut state) = minimal_fixture();
        state.memories.push(MemoryEntry {
            id: story_model::uuid::Uuid::nil(),
            kind: MemoryKind::Promise,
            facts: vec![],
            interpretation: Some("유나는 반송된 편지의 이유를 다음에 말하기로 했다".to_string()),
            participants: BTreeSet::new(),
            location: None,
            importance: 10,
            unresolved: true,
            source_history_ids: vec![],
            created_turn: 0,
            last_recalled_turn: None,
        });
        state.memories.push(MemoryEntry {
            id: story_model::uuid::Uuid::from_u128(1),
            kind: MemoryKind::Other,
            facts: vec![],
            interpretation: Some("유나는 오래된 재즈를 좋아한다".to_string()),
            participants: BTreeSet::from([story_model::CharacterId("c-1".to_string())]),
            location: None,
            importance: 9,
            unresolved: false,
            source_history_ids: vec![],
            created_turn: 0,
            last_recalled_turn: None,
        });
        state.last_choice = Some(story_model::ChoiceRef {
            choice_id: "choice-1".to_string(),
            label: "편지를 직접 묻는다".to_string(),
            turn: 0,
        });

        let bundle = build_prompt(&pack, &state, &[], None);

        assert!(bundle.user.contains("# Relationship ledger (engine-owned)"));
        assert!(bundle.user.contains("# Recalled long-term memory"));
        assert!(bundle.user.contains("반송된 편지의 이유"));
        assert!(bundle.user.contains("오래된 재즈를 좋아한다"));
        assert!(bundle.user.contains("# Current dramatic beat"));
        assert!(bundle.user.contains("# Engine-owned choice design"));
        assert!(bundle.user.contains("role=relationship"));
        assert!(bundle.user.contains("cost="));
        assert!(bundle.user.contains("immediate_consequence="));
        assert!(bundle.user.contains("role=relationship"));
        assert!(bundle.user.contains("role=evidence"));
        assert!(bundle.user.contains("role=situation"));
        assert!(bundle.user.contains("cost="));
        assert!(bundle.user.contains("immediate_consequence="));
        assert!(bundle.user.contains("편지를 직접 묻는다"));
    }

    // --- side-generation prompts (continue / suggest) ------------------------

    fn committed_entry() -> HistoryEntry {
        HistoryEntry {
            id: story_model::uuid::Uuid::nil(),
            turn: 3,
            scene_id: "s".to_string(),
            created_at: story_model::chrono::Utc::now(),
            narration: "유나가 창밖의 빗소리를 들으며 조용히 말했다.".to_string(),
            player_action: None,
            choices: vec![Choice {
                id: "choice-0".to_string(),
                label: "자리를 파악한다".to_string(),
                intent: String::new(),
                tone: None,
                target: None,
                risk: story_model::RiskLevel::Low,
                hidden_requirements: vec![],
                preview: None,
            }],
            applied_patch: story_model::StatePatch::default(),
            generation_id: story_model::uuid::Uuid::nil(),
        }
    }

    #[test]
    fn continue_prompt_carries_the_korean_instruction_and_previous_narration() {
        let (pack, state) = minimal_fixture();
        let recent = [committed_entry()];

        let bundle = build_continue_prompt(&pack, &state, &recent);

        assert!(
            bundle
                .system
                .contains("# Continue narration (engine-owned)")
        );
        assert!(bundle.system.contains("이전 내레이션을 그대로 이어서"));
        assert!(bundle.user.contains("# Continue target (engine-owned)"));
        assert!(bundle.user.contains("이전 내레이션 — 이 바로 뒤에서 계속"));
        assert!(bundle.user.contains(&recent[0].narration));
        assert!(
            !bundle.user.contains("# Suggest action"),
            "the suggest instruction must not leak into the continue prompt"
        );
    }

    #[test]
    fn suggest_prompt_carries_the_suggestion_instruction_and_sovereignty() {
        let (pack, state) = minimal_fixture();

        let bundle = build_suggest_prompt(&pack, &state, &[]);

        assert!(bundle.system.contains("# Suggest action (engine-owned)"));
        assert!(
            bundle
                .system
                .contains("플레이어가 다음에 취할 행동 한 문장(한국어)만 제안")
        );
        assert!(
            bundle.system.contains("플레이어 주권"),
            "the suggestion must carry the player-sovereignty context"
        );
        assert!(
            bundle
                .user
                .contains("# Suggested player action (engine-owned)")
        );
        assert!(
            !bundle.system.contains("# Continue narration"),
            "the continue instruction must not leak into the suggest prompt"
        );
    }

    #[test]
    fn side_generation_prompts_are_deterministic_and_distinct() {
        let (pack, state) = minimal_fixture();
        let recent = [committed_entry()];

        let continue_a = build_continue_prompt(&pack, &state, &recent);
        let continue_b = build_continue_prompt(&pack, &state, &recent);
        assert_eq!(continue_a.hash, continue_b.hash, "same inputs, same hash");

        let suggest_a = build_suggest_prompt(&pack, &state, &recent);
        let suggest_b = build_suggest_prompt(&pack, &state, &recent);
        assert_eq!(suggest_a.hash, suggest_b.hash);

        let scene_a = build_group_scene_prompt(&pack, &state, &recent);
        let scene_b = build_group_scene_prompt(&pack, &state, &recent);
        assert_eq!(scene_a.hash, scene_b.hash);

        let plain = build_prompt(&pack, &state, &recent, None);
        assert_ne!(continue_a.hash, plain.hash);
        assert_ne!(suggest_a.hash, plain.hash);
        assert_ne!(scene_a.hash, plain.hash);
        assert_ne!(continue_a.hash, suggest_a.hash);
        assert_ne!(continue_a.hash, scene_a.hash);
        assert_ne!(suggest_a.hash, scene_a.hash);
    }

    // --- group-scene prompt (SideGenerationKind::Scene) ----------------------

    fn named_character(id: &str, name: &str) -> story_model::CharacterDef {
        story_model::CharacterDef {
            id: story_model::CharacterId(id.to_string()),
            name: name.to_string(),
            age: None,
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
        }
    }

    #[test]
    fn group_scene_prompt_carries_the_korean_instruction_and_line_format() {
        let (pack, state) = minimal_fixture();
        let recent = [committed_entry()];

        let bundle = build_group_scene_prompt(&pack, &state, &recent);

        assert!(bundle.system.contains("# Group scene (engine-owned)"));
        assert!(bundle.system.contains("두 명 이상의 등장인물"));
        assert!(bundle.system.contains("`이름: \"대사\"`"));
        assert!(
            bundle
                .system
                .contains("플레이어의 대사·감정·행동을 대신 정하지 않는다"),
            "the scene must carry the player-sovereignty clause"
        );
        assert!(
            bundle
                .system
                .contains("choices/state_patch/summary는 형식 유지용 복사본"),
            "the format-preserving copy contract is stated"
        );
        assert!(bundle.user.contains("# Group scene (engine-owned)"));
        assert!(bundle.user.contains("`이름: \"대사\"`"));
        assert!(bundle.user.contains(&recent[0].narration));
        assert!(
            !bundle.system.contains("# Continue narration"),
            "the continue instruction must not leak into the scene prompt"
        );
        assert!(
            !bundle.system.contains("# Suggest action"),
            "the suggest instruction must not leak into the scene prompt"
        );
    }

    #[test]
    fn group_scene_user_block_lists_the_pack_cast_by_name() {
        let (mut pack, mut state) = minimal_fixture();
        pack.characters.insert(
            story_model::CharacterId("c-1".to_string()),
            named_character("c-1", "한유나"),
        );
        let extra_id = story_model::CharacterId("c-2".to_string());
        pack.characters
            .insert(extra_id.clone(), named_character("c-2", "나무"));
        state.characters.insert(
            extra_id,
            CharacterState {
                affection: 0,
                trust: 0,
                tension: 0,
                relationship_stage: RelationshipStage::Stranger,
                current_emotion: Emotion::Neutral,
                known_facts: BTreeSet::new(),
                address_form_overrides: BTreeMap::new(),
            },
        );

        let bundle = build_group_scene_prompt(&pack, &state, &[]);

        assert!(
            bundle.user.contains("대화 가능한 인물: 한유나, 나무"),
            "the cast line joins pack display names in state key order: {:?}",
            bundle.user
        );
    }

    #[test]
    fn free_action_with_target_gains_the_response_directive_line() {
        let (pack, state) = minimal_fixture();
        let targeted = ParsedAction {
            action_type: story_model::ActionType::Dialogue,
            target: Some(story_model::CharacterId("c-1".to_string())),
            intent: None,
            tone: None,
            content: "안녕".to_string(),
            original_text: "@c-1 안녕".to_string(),
            confidence: Some(1.0),
        };

        let with = build_prompt(&pack, &state, &[], Some(&targeted));
        let without = build_prompt(&pack, &state, &[], None);
        let again = build_prompt(&pack, &state, &[], Some(&targeted));

        assert!(
            with.user
                .contains("c-1은 플레이어가 직접 말 건 캐릭터 — 응답 필수, 타 인물 반응 자연 연결"),
            "the targeted directive line rides the free-action block: {:?}",
            with.user
        );
        assert!(
            !without.user.contains("플레이어가 직접 말 건"),
            "plain free actions keep their prompt byte-shape (no directive line)"
        );
        assert_eq!(with.hash, again.hash, "the directive line is deterministic");
    }

    #[test]
    fn system_prompt_includes_cumulative_stat_bounds() {
        let (mut pack, state) = minimal_fixture();
        let default_bundle = build_prompt(&pack, &state, &[], None);
        assert!(
            default_bundle
                .system
                .contains("# Stat bounds (total after delta, inclusive)"),
            "{}",
            default_bundle.system
        );
        assert!(
            default_bundle.system.contains("tension: -100..=100"),
            "{}",
            default_bundle.system
        );

        pack.rules.stat_limits_by_axis = Some(story_model::DeltaLimits {
            affection: story_model::Int16Range {
                min: -100,
                max: 100,
            },
            trust: story_model::Int16Range {
                min: -100,
                max: 100,
            },
            tension: story_model::Int16Range { min: 0, max: 30 },
        });
        let custom_bundle = build_prompt(&pack, &state, &[], None);
        assert!(
            custom_bundle.system.contains("tension: 0..=30"),
            "{}",
            custom_bundle.system
        );
    }
}
