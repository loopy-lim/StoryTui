//! End-to-end: card → pack → prompt. An imported lorebook must reach the
//! model's user prompt with no manual wiring, and the pack must keep
//! verifying against the real loader after a full workspace build.

use std::fs;

use base64::Engine as _;
use story_core::prompt_context::build_prompt;
use story_import::{ImportOptions, import_card};
use story_persistence::PackLoader;

fn v2_card_png(card_json: &str) -> Vec<u8> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(card_json);
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .add_text_chunk("chara".to_string(), encoded)
            .expect("text chunk");
        let mut writer = encoder.write_header().expect("png header");
        writer.write_image_data(&[0, 0, 0, 0]).expect("png data");
    }
    out
}

#[test]
fn imported_lorebook_and_opening_reach_the_prompt() {
    let card = serde_json::json!({
        "spec": "chara_card_v2",
        "data": {
            "name": "Mira",
            "description": "{{char}} is a night-shift archivist. {{user}} is a conservator.",
            "first_mes": "{{user}}… you're late.",
            "character_book": {
                "entries": [
                    {
                        "keys": ["pocket watch"],
                        "content": "Her brass watch stopped at 3:12 AM the night her father died.",
                        "insertion_order": 20,
                        "position": "after_char"
                    },
                    {
                        "keys": [],
                        "content": "The insurance assessor arrives at dawn; anything unlogged counts as destroyed.",
                        "constant": true,
                        "insertion_order": 10
                    }
                ]
            }
        }
    });
    let temp = tempfile::tempdir().expect("tempdir");
    let card_path = temp.path().join("mira.png");
    fs::write(&card_path, v2_card_png(&card.to_string())).expect("write card");
    let out_dir = temp.path().join("pack");

    let output =
        import_card(&card_path, Some(&out_dir), &ImportOptions::default()).expect("import");
    assert!(output.report.loader_verified);

    // The pack verifies again through the public loader, independently of the
    // importer's own self-check.
    let pack = PackLoader::load(&out_dir).expect("pack loads");

    let mut state = story_core::tests_support::sample_game_state();
    state.history = vec![story_core::tests_support::sample_history_entry(
        1,
        "She checks the pocket watch again, silent.",
    )];

    let bundle = build_prompt(&pack, &state, &state.history, None);
    assert!(
        bundle.user.contains("stopped at 3:12 AM"),
        "keyed lore must reach the prompt: {}",
        bundle.user
    );
    assert!(
        bundle
            .user
            .contains("anything unlogged counts as destroyed"),
        "constant lore must reach the prompt: {}",
        bundle.user
    );
    // Placement: the constant (before_char) sits in the stable prefix before
    // the canon; the triggered keyed entry sits after the canon.
    let canon = bundle.user.find("# Character canon").expect("canon");
    let progression = bundle
        .user
        .find("# Story progression")
        .expect("progression");
    let constant_pos = bundle
        .user
        .find("anything unlogged counts as destroyed")
        .expect("constant lore");
    let watch_pos = bundle.user.find("stopped at 3:12 AM").expect("keyed lore");
    assert!(
        constant_pos < canon,
        "before_char constant stays in the prefix"
    );
    assert!(canon < watch_pos && watch_pos < progression);
    // The importer resolved {{user}} into opening.md (engine-side opening
    // rendering is covered by prompt_context tests).
    let opening = fs::read_to_string(out_dir.join("opening.md")).expect("opening");
    assert!(
        opening.contains("플레이어… you're late."),
        "{{user}} must be resolved on disk: {opening}"
    );
}
