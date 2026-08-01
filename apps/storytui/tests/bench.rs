//! Determinism test for the benchmark runner (ST-036).
//!
//! Drives the engine through a fixed prompt bundle against a deterministic
//! Mock provider, then asserts that running the same bundle twice produces
//! IDENTICAL prompt hashes and result patch digests. This is the
//! code-level reproducibility check; a real-provider `#[ignore]` run is the
//! operator's job (see docs/RELEASE-RUNTIME-GATE.md).

use std::path::PathBuf;

use sha2::{Digest, Sha256};
use story_core::engine::{EngineState, reduce};
use story_core::engine_cmd::EngineCommand;
use story_model::StoryPack;
use story_model::{CharacterId, Choice, GeneratedTurnDraft, RiskLevel, StatePatch, TurnSummary};
use story_persistence::PackLoader;
use storytui::bench::{BenchConfig, TurnRecord, run_deterministic_bench};

fn rainy_reunion_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("stories")
        .join("rainy-reunion")
}

fn load_pack() -> StoryPack {
    let root = rainy_reunion_root();
    assert!(root.is_dir(), "stories/rainy-reunion not found at {root:?}");
    PackLoader::load(&root).unwrap_or_else(|e| panic!("rainy-reunion failed to load: {e}"))
}

/// THE determinism test: same prompt bundle ⇒ identical prompt hashes AND
/// identical result patch digests across runs.
#[test]
fn benchmark_reproduces_identical_hashes_and_digests() {
    let pack = load_pack();
    let cfg = BenchConfig {
        turns: 5,
        temperature: 0.85,
        top_p: 0.92,
        top_k: 40,
        max_tokens: 1800,
    };
    let report_a = run_deterministic_bench(&pack, &cfg);
    let report_b = run_deterministic_bench(&pack, &cfg);

    assert_eq!(
        report_a.turns.len(),
        report_b.turns.len(),
        "turn count diverged"
    );
    for (a, b) in report_a.turns.iter().zip(report_b.turns.iter()) {
        assert_eq!(
            a.prompt_hash, b.prompt_hash,
            "prompt hash diverged at turn {}: {} vs {}",
            a.turn, a.prompt_hash, b.prompt_hash
        );
        assert_eq!(
            a.patch_digest, b.patch_digest,
            "patch digest diverged at turn {}",
            a.turn
        );
        assert_eq!(a.temperature, b.temperature);
        assert_eq!(a.top_p, b.top_p);
        assert_eq!(a.top_k, b.top_k);
        assert_eq!(a.max_tokens, b.max_tokens);
    }
    assert_eq!(
        report_a.final_turn, report_b.final_turn,
        "final turn diverged"
    );
}

/// Prompt hashes MUST change turn-to-turn as history grows — otherwise the
/// prompt is not reflecting accumulated state.
#[test]
fn benchmark_prompt_hashes_are_distinct_per_turn() {
    let pack = load_pack();
    let cfg = BenchConfig {
        turns: 4,
        temperature: 0.85,
        top_p: 0.92,
        top_k: 40,
        max_tokens: 1800,
    };
    let report = run_deterministic_bench(&pack, &cfg);
    let mut seen = std::collections::HashSet::new();
    for t in &report.turns {
        assert!(
            seen.insert(&t.prompt_hash),
            "duplicate prompt hash at turn {}: {}",
            t.turn,
            t.prompt_hash
        );
    }
}

/// Sampling parameters recorded per turn must mirror the config.
#[test]
fn benchmark_records_sampling_params() {
    let pack = load_pack();
    let cfg = BenchConfig {
        turns: 2,
        temperature: 0.7,
        top_p: 0.9,
        top_k: 30,
        max_tokens: 1024,
    };
    let report = run_deterministic_bench(&pack, &cfg);
    let t0 = &report.turns[0];
    assert_eq!(t0.temperature, 0.7);
    assert_eq!(t0.top_p, 0.9);
    assert_eq!(t0.top_k, 30);
    assert_eq!(t0.max_tokens, 1024);
}

/// The report carries a model id placeholder and a seed-feasibility note so a
/// future operator knows what a real run replaces.
#[test]
fn benchmark_report_carries_metadata() {
    let pack = load_pack();
    let cfg = BenchConfig {
        turns: 1,
        temperature: 0.85,
        top_p: 0.92,
        top_k: 40,
        max_tokens: 1800,
    };
    let report = run_deterministic_bench(&pack, &cfg);
    assert!(!report.note.is_empty(), "note must explain the seed");
    assert!(
        report.note.contains("Mock")
            || report.note.contains("mock")
            || report.note.contains("deterministic"),
        "note should call out the Mock/deterministic nature: {}",
        report.note
    );
    assert_eq!(report.config.turns, 1);
}

/// A reference hash pinned against `run_deterministic_bench`'s documented
/// algorithm — guards against accidental prompt-construction drift. Hash the
/// system+user of the opening turn manually and confirm the runner matches.
#[test]
fn benchmark_prompt_hash_matches_reference_algorithm() {
    let pack = load_pack();
    let cfg = BenchConfig {
        turns: 1,
        temperature: 0.85,
        top_p: 0.92,
        top_k: 40,
        max_tokens: 1800,
    };
    let report = run_deterministic_bench(&pack, &cfg);
    let t0 = &report.turns[0];

    // Rebuild the same prompt the engine built for the opening turn and hash
    // it the same way the runner does (system || 0 || user || 0 over role+content).
    let init = EngineState::initial(&pack);
    let start = reduce(&init, EngineCommand::StartNewGame, &pack).expect("start");
    let req = start
        .effects
        .iter()
        .find_map(|e| match e {
            story_core::engine_cmd::EngineEffect::RequestGeneration { request, .. } => {
                Some(request)
            }
            _ => None,
        })
        .expect("opening RequestGeneration");

    let mut h = Sha256::new();
    for m in &req.messages {
        h.update(m.role.as_bytes());
        h.update(b"\x00");
        h.update(m.content.as_bytes());
        h.update(b"\x00");
    }
    let expected = hex_lower(h.finalize().as_slice());
    assert_eq!(t0.prompt_hash, expected, "prompt hash algorithm diverged");
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

// Bring the demo turn-record shape into scope for documentation assertions.
const _: fn(&TurnRecord) = |_| {};

#[allow(dead_code)]
fn _types_used(
    _draft: GeneratedTurnDraft,
    _patch: StatePatch,
    _sum: TurnSummary,
    _id: CharacterId,
    _ch: Choice,
    _r: RiskLevel,
) {
}
