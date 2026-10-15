//! Import contract tests: a parsed card must scaffold a pack the real
//! `PackLoader` accepts, with every file where the engine expects it.

use base64::Engine as _;
use story_import::{ImportOptions, import_card};

/// Build a real PNG (1x1) with the card JSON in a `chara` tEXt chunk, using
/// the same encoder the ecosystem's cards are readable with.
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

fn v2_card_json() -> String {
    r#"{
        "spec": "chara_card_v2",
        "spec_version": "2.0",
        "data": {
            "name": "한유나",
            "description": "카페 나무의 단골. {{char}}는 관찰력이 좋고 {{user}}를 오래 기다렸다.",
            "personality": "차분하지만 비오는 날에는 말이 많아진다.",
            "scenario": "비가 그친 카페에서 3년 만에 재회했다.",
            "first_mes": "{{user}}… 정말 왔구나.",
            "mes_example": "<START>\n{{user}}: 오래 기다렸어?\n{{char}}: 세 년 정도?",
            "alternate_greetings": ["비 오는 날엔 네 생각이 났어."],
            "system_prompt": "카드의 시스템 프롬프트(무시되어야 함)",
            "character_book": {
                "name": "유나의 기억",
                "scan_depth": 3,
                "token_budget": 1500,
                "entries": [
                    {
                        "keys": ["전시회"],
                        "content": "3년 전 두 사람이 헤어진 계기가 된 전시회.",
                        "enabled": true,
                        "insertion_order": 10,
                        "position": "before_char"
                    },
                    {
                        "keys": ["우산"],
                        "secondary_keys": ["비"],
                        "selective": true,
                        "content": "두 사람이 함께 쓰던 파란 우산.",
                        "constant": false,
                        "insertion_order": 20,
                        "position": "after_char"
                    },
                    {
                        "keys": ["언제나"],
                        "content": "항상 주입되는 상시 설정.",
                        "constant": true,
                        "insertion_order": 5
                    }
                ]
            }
        }
    }"#
    .to_string()
}

fn options() -> ImportOptions {
    ImportOptions::default()
}

#[test]
fn v2_png_imports_into_a_loadable_pack() {
    let temp = tempfile::tempdir().expect("tempdir");
    let card_path = temp.path().join("yuna.png");
    std::fs::write(&card_path, v2_card_png(&v2_card_json())).expect("write card");
    let out_dir = temp.path().join("packs/yuna");

    let output = import_card(&card_path, Some(&out_dir), &options()).expect("import");
    assert!(
        output.report.loader_verified,
        "PackLoader self-check must pass"
    );
    assert_eq!(output.report.lorebook_entries, 3);
    assert!(
        output
            .report
            .ignored_fields
            .iter()
            .any(|(path, _)| path == "system_prompt")
    );

    // The ten required files all exist.
    for required in [
        "story.toml",
        "characters.toml",
        "rules.toml",
        "events.toml",
        "world.md",
        "style.md",
        "opening.md",
        "prompts/system.md",
        "prompts/director.md",
        "prompts/writer.md",
    ] {
        assert!(out_dir.join(required).is_file(), "missing {required}");
    }

    // Extras: avatar, greetings, sources, lorebook, report. The non-ASCII
    // card name falls back to the `card` slug — the same id characters.toml
    // carries, so the host portrait convention still matches.
    let character_id = "card";
    assert_eq!(
        std::fs::read(
            out_dir
                .join("images/characters")
                .join(format!("{character_id}.png"))
        )
        .expect("avatar"),
        v2_card_png(&v2_card_json()),
        "the card PNG itself becomes the portrait"
    );
    assert!(
        std::fs::read_to_string(out_dir.join("characters.toml"))
            .expect("characters")
            .contains(&format!("id = \"{character_id}\"")),
        "the avatar stem and the character id must agree"
    );
    assert!(
        out_dir.join("greetings/1.md").is_file(),
        "alternate greetings are kept"
    );
    assert!(out_dir.join("sources/card.json").is_file());
    assert!(out_dir.join("import-report.md").is_file());
    let lorebook = std::fs::read_to_string(out_dir.join("lorebook.toml")).expect("lorebook");
    assert!(lorebook.contains("전시회"));
    assert!(
        lorebook.contains("scan_depth = 3"),
        "book metadata maps: {lorebook}"
    );
    assert!(lorebook.contains("token_budget = 1500"));

    // Macro substitution + composition landed in characters.toml.
    let characters = std::fs::read_to_string(out_dir.join("characters.toml")).expect("characters");
    assert!(characters.contains("한유나는 관찰력이 좋고 플레이어를 오래 기다렸다."));
    assert!(characters.contains("# 성격"));
    assert!(characters.contains("dialogue_examples"));

    // Opening resolved {{user}}.
    let opening = std::fs::read_to_string(out_dir.join("opening.md")).expect("opening");
    assert!(opening.contains("플레이어… 정말 왔구나."));
}

#[test]
fn standalone_world_info_merges_into_the_book() {
    let temp = tempfile::tempdir().expect("tempdir");
    let card_path = temp.path().join("card.json");
    std::fs::write(
        &card_path,
        r#"{"name":"유나","description":"d","first_mes":"안녕"}"#,
    )
    .expect("card");
    let wi_path = temp.path().join("world.json");
    std::fs::write(
        &wi_path,
        r#"{"entries": {"0": {"uid": 0, "key": ["카페"], "content": "단골 카페다.", "order": 100, "position": 0, "disable": false}}}"#,
    )
    .expect("wi");

    let mut options = options();
    options.lorebook_path = Some(wi_path);
    let out_dir = temp.path().join("pack");
    let output = import_card(&card_path, Some(&out_dir), &options).expect("import");
    assert_eq!(output.report.lorebook_entries, 1);

    let lorebook = std::fs::read_to_string(out_dir.join("lorebook.toml")).expect("lorebook");
    assert!(lorebook.contains("단골 카페다."), "{lorebook}");
    assert!(lorebook.contains("before_char"));
}

#[test]
fn existing_output_directory_requires_force() {
    let temp = tempfile::tempdir().expect("tempdir");
    let card_path = temp.path().join("card.json");
    std::fs::write(&card_path, r#"{"name":"유나","description":"d"}"#).expect("card");
    let out_dir = temp.path().join("pack");

    import_card(&card_path, Some(&out_dir), &options()).expect("first import");
    let second = import_card(&card_path, Some(&out_dir), &options());
    assert!(second.is_err(), "without --force a second import must fail");

    let mut force = options();
    force.force = true;
    import_card(&card_path, Some(&out_dir), &force).expect("forced re-import");
}

#[test]
fn oversized_description_is_truncated_and_reported() {
    let temp = tempfile::tempdir().expect("tempdir");
    let card_path = temp.path().join("card.json");
    let huge = "한".repeat(5000);
    std::fs::write(
        &card_path,
        format!(r#"{{"name":"유나","description":"{huge}"}}"#),
    )
    .expect("card");
    let out_dir = temp.path().join("pack");

    let output = import_card(&card_path, Some(&out_dir), &options()).expect("import");
    assert!(
        output
            .report
            .warnings
            .iter()
            .any(|warning| warning.contains("토큰 상한")),
        "token gate must warn: {:?}",
        output.report.warnings
    );
    let characters = std::fs::read_to_string(out_dir.join("characters.toml")).expect("characters");
    assert!(characters.contains("…(절단)"), "description must be cut");
}

#[test]
fn depth_and_timer_features_are_reported_as_approximations() {
    let temp = tempfile::tempdir().expect("tempdir");
    let card_path = temp.path().join("card.json");
    std::fs::write(
        &card_path,
        r#"{
            "name": "유나",
            "description": "d",
            "character_book": {
                "entries": [{
                    "keys": ["카페"],
                    "content": "단골 카페.",
                    "extensions": {"position": 4, "depth": 2, "vectorized": true}
                }]
            }
        }"#,
    )
    .expect("card");

    let output =
        import_card(&card_path, Some(&temp.path().join("pack")), &options()).expect("import");
    let approximations: Vec<&String> = output
        .report
        .approximations
        .iter()
        .filter(|note| note.contains("근사") || note.contains("미지원"))
        .collect();
    assert!(
        approximations.len() >= 2,
        "depth position and vectorized must be reported: {:?}",
        output.report.approximations
    );
}
