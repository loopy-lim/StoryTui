//! End-to-end: a Character.AI community-dump JSON (the shape c.ai download
//! extensions emit) imports with its aliases mapped and the raw dump kept.

use story_import::{ImportOptions, import_card};

#[test]
fn cai_raw_dump_imports_with_alias_mapping() {
    let temp = tempfile::tempdir().expect("tempdir");
    let dump = r#"{"character":{"name":"세라","description":"AI 동료","greeting":"오셨군요.","definition":"<START>\n{{user}}: 안녕\n{{char}}: 반가워","title":"호기심 많은 AI"}}"#;
    let card_path = temp.path().join("sera.json");
    std::fs::write(&card_path, dump).expect("write dump");
    let out_dir = temp.path().join("pack");

    let output =
        import_card(&card_path, Some(&out_dir), &ImportOptions::default()).expect("import");
    assert!(output.report.loader_verified, "PackLoader self-check");

    let opening = std::fs::read_to_string(out_dir.join("opening.md")).expect("opening.md");
    assert!(opening.contains("오셨군요."), "{opening}");
    let characters =
        std::fs::read_to_string(out_dir.join("characters.toml")).expect("characters.toml");
    assert!(characters.contains("세라: 반가워"), "{characters}");
    // The aliases used are recorded, and the raw dump keeps `greeting`.
    let report = std::fs::read_to_string(out_dir.join("import-report.md")).expect("report");
    assert!(report.contains("c.ai"), "{report}");
    let raw = std::fs::read_to_string(out_dir.join("sources/card.json")).expect("raw dump");
    assert!(raw.contains("greeting"), "{raw}");
}
