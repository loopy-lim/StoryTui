//! Rule-based free-text → [`ParsedAction`] parser (ST-023).
//!
//! [`parse_action`] structures the player's free-text intent WITHOUT
//! interpreting it. The output is a conservative, deterministic classification
//! (`Move` / `Dialogue` / `Action` / `GeneralAction`) plus a verbatim copy of
//! the input. Player sovereignty is load-bearing (see ADR-003 and the prompt
//! contract in [`crate::prompt_context`]):
//!
//! - `original_text` is ALWAYS the raw input, untouched.
//! - `content` is the trimmed input — we never paraphrase, expand, or fabricate.
//! - `intent` and `tone` are NEVER produced from thin air; they stay `None`.
//! - When the match is weak or ambiguous we fall back to
//!   [`ActionType::GeneralAction`] with low confidence rather than emit a
//!   confident false action. The model downstream is told "this is the player's
//!   intent; do not confirm or distort unstated emotions/actions."
//!
//! # Determinism
//!
//! The function is a pure function of (`raw`, `ctx`). `ctx` carries the
//! pack-known characters and locations as `BTreeMap`s; iteration order is fixed
//! by key order, and "longest match wins" breaks ties between overlapping names
//! deterministically.

use std::collections::BTreeMap;

use story_model::{ActionType, CharacterDef, CharacterId, LocationDef, LocationId, ParsedAction};

/// Read-only pack context the parser matches against: the characters and
/// locations the player could reasonably refer to by name this turn.
#[derive(Clone, Debug)]
pub struct ParseContext<'a> {
    /// Pack-known characters, keyed by id. The parser matches by `name`
    /// (longest match wins) and by id string.
    pub known_characters: &'a BTreeMap<CharacterId, CharacterDef>,
    /// Pack-known locations, keyed by id. The parser matches by `display_name`
    /// (longest match wins) and by id string.
    pub known_locations: &'a BTreeMap<LocationId, LocationDef>,
}

/// Parse free text into a conservative [`ParsedAction`].
///
/// See the module docs for the sovereignty contract. Classification precedence
/// (first match wins):
///
/// 1. Movement verb + a known location → [`ActionType::Move`].
/// 2. Dialogue verb or quoted span → [`ActionType::Dialogue`].
/// 3. A known character is mentioned AND there is content beyond the bare name
///    → [`ActionType::Action`].
/// 4. Otherwise → [`ActionType::GeneralAction`] (low confidence).
pub fn parse_action(raw: &str, ctx: &ParseContext) -> ParsedAction {
    let original_text = raw.to_string();
    let trimmed = raw.trim();

    if trimmed.is_empty() {
        return ParsedAction {
            action_type: ActionType::GeneralAction,
            target: None,
            intent: None,
            tone: None,
            content: String::new(),
            original_text,
            confidence: Some(0.0),
        };
    }

    let target = find_character(trimmed, ctx);
    let location = find_location(trimmed, ctx);
    let move_verb = has_move_verb(trimmed);
    let dialogue_verb = has_dialogue_verb(trimmed);
    let quoted = has_quoted(trimmed);

    let (action_type, confidence) = if move_verb && location.is_some() {
        (ActionType::Move, 0.85)
    } else if dialogue_verb || quoted {
        (ActionType::Dialogue, if dialogue_verb { 0.8 } else { 0.7 })
    } else if let Some((_, def)) = target.as_ref() {
        if has_content_beyond_name(trimmed, &def.name) {
            (ActionType::Action, 0.7)
        } else {
            // Bare character mention (vocative / ambiguous) — do NOT pretend it
            // is a confident directed action.
            (ActionType::GeneralAction, 0.35)
        }
    } else {
        (ActionType::GeneralAction, 0.3)
    };

    ParsedAction {
        action_type,
        target: target.map(|(id, _)| id.clone()),
        // Never fabricated: left None unless directly derivable (none are, for
        // a rule-based parser).
        intent: None,
        tone: None,
        content: trimmed.to_string(),
        original_text,
        confidence: Some(confidence),
    }
}

// ---------------------------------------------------------------------------
// entity matching
// ---------------------------------------------------------------------------

/// Find the best-matching known character in `text`. "Best" = the match whose
/// hit (name or id) has the most scalar values, so `한유나` wins over `유나`.
/// Ties break by `BTreeMap` key order (deterministic).
fn find_character<'a>(
    text: &str,
    ctx: &'a ParseContext<'a>,
) -> Option<(&'a CharacterId, &'a CharacterDef)> {
    ctx.known_characters
        .iter()
        .filter_map(|(id, def)| {
            let hit_len = match_hits(text, [&def.name.as_str(), &id.0.as_str()]);
            hit_len.map(|l| (id, def, l))
        })
        .max_by_key(|(_, _, l)| *l)
        .map(|(id, def, _)| (id, def))
}

/// Find the best-matching known location in `text` by `display_name` or id.
fn find_location<'a>(
    text: &str,
    ctx: &'a ParseContext<'a>,
) -> Option<(&'a LocationId, &'a LocationDef)> {
    ctx.known_locations
        .iter()
        .filter_map(|(id, def)| {
            let hit_len = match_hits(text, [&def.display_name.as_str(), &id.0.as_str()]);
            hit_len.map(|l| (id, def, l))
        })
        .max_by_key(|(_, _, l)| *l)
        .map(|(id, def, _)| (id, def))
}

/// Return the longest matched candidate's scalar count, or `None` if no
/// candidate appears in `text`.
fn match_hits<'s, I>(text: &str, candidates: I) -> Option<usize>
where
    I: IntoIterator<Item = &'s &'s str>,
{
    candidates
        .into_iter()
        .filter(|c| !c.is_empty())
        .filter(|c| text.contains(*c))
        .map(|c| c.chars().count())
        .max()
}

// ---------------------------------------------------------------------------
// verb / quote detection
// ---------------------------------------------------------------------------

/// Movement verbs. Conservative: each token is an unambiguous Korean movement
/// stem. Bare `가` is deliberately excluded (it appears inside many words).
const MOVE_VERBS: &[&str] = &[
    "간다",
    "이동",
    "갈까",
    "가자",
    "출발",
    "돌아가",
    "들어가",
    "나간다",
    "올라간다",
];

fn has_move_verb(text: &str) -> bool {
    MOVE_VERBS.iter().any(|v| text.contains(v))
}

/// Dialogue verbs. Each is a clear speech act stem.
const DIALOGUE_VERBS: &[&str] = &[
    "말한다",
    "묻는다",
    "대답한다",
    "외친다",
    "속삭인다",
    "말했",
    "대답했",
    "물었",
    "말하고",
];

fn has_dialogue_verb(text: &str) -> bool {
    DIALOGUE_VERBS.iter().any(|v| text.contains(v))
}

/// True when `text` carries a quoted span: a pair of ASCII `"`, or any CJK
/// quotation bracket (`「` `」` `『` `』`). A lone apostrophe-like `"` does not
/// count.
fn has_quoted(text: &str) -> bool {
    let double_quotes = text.chars().filter(|&c| c == '"').count();
    double_quotes >= 2 || text.contains('「') || text.contains('『')
}

// ---------------------------------------------------------------------------
// "content beyond a bare name" test
// ---------------------------------------------------------------------------

/// Korean case particles / delimiters that may attach to a name without adding
/// semantic action content (`유나에게`, `유나를`, `유나는`, ...). Stripping them
/// lets us tell `유나` (bare mention) apart from `유나를 본다` (real action).
const PARTICLES: &str = "을를에게한테가이는은도의와과야아야여조차부터까지도";

fn is_particle(c: char) -> bool {
    PARTICLES.contains(c)
}

/// True when `text` contains non-trivial content beyond `name` + particles +
/// whitespace. `name = "유나"`: `"유나"` → false; `"유나를 본다"` → true.
fn has_content_beyond_name(text: &str, name: &str) -> bool {
    // Remove the name FIRST, then strip particles/whitespace from the
    // remainder. Removing particles first would wrongly eat syllables that
    // overlap a particle character (e.g. `한` in `한유나` overlaps `한테`).
    let remainder = text.replace(name, "");
    let core: String = remainder
        .chars()
        .filter(|c| !c.is_whitespace() && !is_particle(*c))
        .collect();
    !core.is_empty()
}

#[cfg(test)]
mod tests {
    //! ST-023: 20 fixed free inputs. `original_text` is asserted verbatim for
    //! every case; low-confidence / ambiguous inputs fall back to
    //! `GeneralAction` rather than a confident false classification.

    use super::*;
    use std::collections::BTreeMap;
    use story_model::{CharacterDef, CharacterId, LocationDef, LocationId, RelationshipStage};

    fn yuna() -> CharacterId {
        CharacterId("yuna".to_string())
    }
    fn namu() -> CharacterId {
        CharacterId("c-namu".to_string())
    }

    fn classroom() -> LocationId {
        LocationId("loc-classroom".to_string())
    }
    fn hall() -> LocationId {
        LocationId("loc-hall".to_string())
    }
    fn cafe() -> LocationId {
        LocationId("loc-cafe".to_string())
    }

    fn minimal_character(id: CharacterId, name: &str) -> CharacterDef {
        CharacterDef {
            id,
            name: name.to_string(),
            age: None,
            pronouns: None,
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

    fn fixture() -> (
        BTreeMap<CharacterId, CharacterDef>,
        BTreeMap<LocationId, LocationDef>,
    ) {
        let mut chars = BTreeMap::new();
        chars.insert(yuna(), minimal_character(yuna(), "한유나"));
        chars.insert(namu(), minimal_character(namu(), "나무"));

        let mut locs = BTreeMap::new();
        locs.insert(
            classroom(),
            LocationDef {
                id: classroom(),
                display_name: "교실".to_string(),
                adjacent: Default::default(),
            },
        );
        locs.insert(
            hall(),
            LocationDef {
                id: hall(),
                display_name: "복도".to_string(),
                adjacent: Default::default(),
            },
        );
        locs.insert(
            cafe(),
            LocationDef {
                id: cafe(),
                display_name: "카페".to_string(),
                adjacent: Default::default(),
            },
        );
        (chars, locs)
    }

    fn parse<'a>(
        raw: &str,
        chars: &'a BTreeMap<CharacterId, CharacterDef>,
        locs: &'a BTreeMap<LocationId, LocationDef>,
    ) -> ParsedAction {
        let ctx = ParseContext {
            known_characters: chars,
            known_locations: locs,
        };
        parse_action(raw, &ctx)
    }

    // ---- helper for confidence-band checks --------------------------------

    fn assert_low(p: &ParsedAction) {
        assert!(
            p.confidence.unwrap_or(1.0) < 0.5,
            "expected low confidence, got {:?} for {:?}",
            p.confidence,
            p.original_text
        );
    }
    fn assert_high(p: &ParsedAction) {
        assert!(
            p.confidence.unwrap_or(0.0) >= 0.6,
            "expected high confidence, got {:?} for {:?}",
            p.confidence,
            p.original_text
        );
    }

    // =====================================================================
    // 20-input suite
    // =====================================================================

    #[test]
    fn t01_dialogue_to_known_character_with_particle_에게() {
        let (c, l) = fixture();
        let p = parse("한유나에게 안녕이라고 말한다", &c, &l);
        assert_eq!(p.original_text, "한유나에게 안녕이라고 말한다");
        assert_eq!(p.action_type, ActionType::Dialogue);
        assert_eq!(p.target.as_ref().map(|c| c.0.as_str()), Some("yuna"));
        assert_eq!(p.intent, None);
        assert_eq!(p.tone, None);
        assert_high(&p);
    }

    #[test]
    fn t02_dialogue_question_verb_묻는다() {
        let (c, l) = fixture();
        let p = parse("한유나에게 묻는다", &c, &l);
        assert_eq!(p.original_text, "한유나에게 묻는다");
        assert_eq!(p.action_type, ActionType::Dialogue);
        assert_eq!(p.target.as_ref().map(|c| c.0.as_str()), Some("yuna"));
    }

    #[test]
    fn t03_move_to_known_location_hall() {
        let (c, l) = fixture();
        let raw = "복도로 간다";
        let p = parse(raw, &c, &l);
        assert_eq!(p.original_text, raw);
        assert_eq!(p.action_type, ActionType::Move);
        // Move keeps `target` for characters only; location is conveyed via content.
        assert!(p.target.is_none());
        assert!(p.content.contains("복도"), "location recorded in content");
        assert_high(&p);
    }

    #[test]
    fn t04_move_to_known_location_classroom_이동() {
        let (c, l) = fixture();
        let p = parse("교실로 이동한다", &c, &l);
        assert_eq!(p.action_type, ActionType::Move);
        assert!(p.target.is_none());
        assert!(p.content.contains("교실"));
    }

    #[test]
    fn t05_move_to_known_location_cafe() {
        let (c, l) = fixture();
        let p = parse("카페로 간다", &c, &l);
        assert_eq!(p.action_type, ActionType::Move);
        assert!(p.content.contains("카페"));
    }

    #[test]
    fn t06_quoted_dialogue_with_target() {
        let (c, l) = fixture();
        let raw = "\"정말 떠났어?\"라고 한유나에게 묻는다";
        let p = parse(raw, &c, &l);
        assert_eq!(p.original_text, raw);
        assert_eq!(p.action_type, ActionType::Dialogue);
        assert_eq!(p.target.as_ref().map(|c| c.0.as_str()), Some("yuna"));
    }

    #[test]
    fn t07_quoted_dialogue_no_target() {
        let (c, l) = fixture();
        let p = parse("\"다시 만나자.\"", &c, &l);
        assert_eq!(p.original_text, "\"다시 만나자.\"");
        assert_eq!(p.action_type, ActionType::Dialogue);
        assert!(p.target.is_none());
    }

    #[test]
    fn t08_action_with_object_particle_을() {
        let (c, l) = fixture();
        let p = parse("나무를 안는다", &c, &l);
        assert_eq!(p.original_text, "나무를 안는다");
        assert_eq!(p.action_type, ActionType::Action);
        assert_eq!(p.target.as_ref().map(|c| c.0.as_str()), Some("c-namu"));
        assert_eq!(p.intent, None);
        assert_high(&p);
    }

    #[test]
    fn t09_action_with_particle_를() {
        let (c, l) = fixture();
        let p = parse("한유나를 본다", &c, &l);
        assert_eq!(p.action_type, ActionType::Action);
        assert_eq!(p.target.as_ref().map(|c| c.0.as_str()), Some("yuna"));
    }

    #[test]
    fn t10_action_with_particle_한테() {
        let (c, l) = fixture();
        let p = parse("한유나한테 편지를 건넨다", &c, &l);
        assert_eq!(p.action_type, ActionType::Action);
        assert_eq!(p.target.as_ref().map(|c| c.0.as_str()), Some("yuna"));
    }

    #[test]
    fn t11_bare_character_name_is_general_action() {
        let (c, l) = fixture();
        let p = parse("한유나", &c, &l);
        assert_eq!(p.original_text, "한유나");
        assert_eq!(p.action_type, ActionType::GeneralAction);
        // Target is recorded as a mention, but the action is NOT a confident false action.
        assert_eq!(p.target.as_ref().map(|c| c.0.as_str()), Some("yuna"));
        assert_low(&p);
    }

    #[test]
    fn t12_bare_character_with_particle_is_general_action() {
        let (c, l) = fixture();
        let p = parse("한유나에게", &c, &l);
        assert_eq!(p.action_type, ActionType::GeneralAction);
        assert_low(&p);
    }

    #[test]
    fn t13_unknown_noun_is_general_action_with_original_preserved() {
        let (c, l) = fixture();
        let raw = "하늘을 바라본다";
        let p = parse(raw, &c, &l);
        assert_eq!(p.original_text, raw);
        assert_eq!(p.action_type, ActionType::GeneralAction);
        assert_eq!(p.content, raw);
        assert!(p.target.is_none());
        assert_low(&p);
    }

    #[test]
    fn t14_bare_verb_no_entity_is_general_action() {
        let (c, l) = fixture();
        let p = parse("기다린다", &c, &l);
        assert_eq!(p.action_type, ActionType::GeneralAction);
        assert!(p.target.is_none());
        assert_low(&p);
    }

    #[test]
    fn t15_unsure_phrase_is_general_action() {
        let (c, l) = fixture();
        let p = parse("모르겠다", &c, &l);
        assert_eq!(p.action_type, ActionType::GeneralAction);
        assert_low(&p);
    }

    #[test]
    fn t16_move_verb_without_known_location_is_general_action() {
        // "밖" is not a pack-known location → cannot confidently classify as Move.
        let (c, l) = fixture();
        let raw = "밖으로 나간다";
        let p = parse(raw, &c, &l);
        assert_eq!(p.original_text, raw);
        assert_eq!(p.action_type, ActionType::GeneralAction);
        assert_low(&p);
    }

    #[test]
    fn t17_go_to_character_is_action_not_move() {
        // "한유나한테 간다": movement toward a character (no known location) →
        // not Move; target present with content beyond name → Action.
        let (c, l) = fixture();
        let p = parse("한유나한테 간다", &c, &l);
        assert_eq!(p.action_type, ActionType::Action);
        assert_eq!(p.target.as_ref().map(|c| c.0.as_str()), Some("yuna"));
    }

    #[test]
    fn t18_cjk_bracket_quote_is_dialogue() {
        let (c, l) = fixture();
        let raw = "「돌아왔어」라고 말했어";
        let p = parse(raw, &c, &l);
        assert_eq!(p.original_text, raw);
        assert_eq!(p.action_type, ActionType::Dialogue);
    }

    #[test]
    fn t19_empty_string_is_general_action_empty_content() {
        let (c, l) = fixture();
        let p = parse("", &c, &l);
        assert_eq!(p.original_text, "");
        assert_eq!(p.action_type, ActionType::GeneralAction);
        assert_eq!(p.content, "");
        assert_eq!(p.confidence, Some(0.0));
    }

    #[test]
    fn t20_whitespace_only_is_general_action_and_original_preserved() {
        let (c, l) = fixture();
        let raw = "   ";
        let p = parse(raw, &c, &l);
        // original_text is verbatim (including the spaces).
        assert_eq!(p.original_text, "   ");
        assert_eq!(p.action_type, ActionType::GeneralAction);
        assert_eq!(p.content, "", "content is trimmed");
    }

    // =====================================================================
    // sovereignty invariants across the whole suite
    // =====================================================================

    #[test]
    fn original_text_is_always_preserved_across_representative_inputs() {
        let (c, l) = fixture();
        let inputs = [
            "한유나에게 말한다",
            "복도로 간다",
            "\"대사\"",
            "한유나",
            "  spaced  ",
            "모르겠다",
            "한유나를 본다",
            "",
        ];
        for raw in inputs {
            let p = parse(raw, &c, &l);
            assert_eq!(p.original_text, raw, "original_text must equal raw input");
            assert!(p.intent.is_none(), "intent never fabricated");
            assert!(p.tone.is_none(), "tone never fabricated");
        }
    }

    #[test]
    fn longest_character_name_wins_over_substring() {
        // "한유나" (3 syllables) should beat a hypothetical "유나" (2). We add
        // a "유나" alias-character to force the overlap.
        let mut c: BTreeMap<CharacterId, CharacterDef> = BTreeMap::new();
        c.insert(
            CharacterId("short".into()),
            minimal_character(CharacterId("short".into()), "유나"),
        );
        c.insert(
            CharacterId("long".into()),
            minimal_character(CharacterId("long".into()), "한유나"),
        );
        let l = BTreeMap::new();
        let p = parse("한유나에게 묻는다", &c, &l);
        assert_eq!(
            p.target.as_ref().map(|c| c.0.as_str()),
            Some("long"),
            "longest match wins"
        );
        assert_eq!(p.action_type, ActionType::Dialogue);
    }

    #[test]
    fn low_confidence_never_produces_confident_action_type() {
        let (c, l) = fixture();
        // Every low-confidence input must be GeneralAction.
        for raw in [
            "한유나",
            "한유나에게",
            "기다린다",
            "밖으로 나간다",
            "모르겠다",
        ] {
            let p = parse(raw, &c, &l);
            if p.confidence.unwrap_or(1.0) < 0.5 {
                assert_eq!(
                    p.action_type,
                    ActionType::GeneralAction,
                    "low-confidence must be GeneralAction (raw={raw})"
                );
            }
        }
    }
}
