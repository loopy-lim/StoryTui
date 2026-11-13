//! Deterministic enrichment lands secrets / goals / second location in the
//! scaffolded pack, and `--inbox` aggregates per-file results.

use story_import::{ImportOptions, import_card, import_inbox};
use story_persistence::PackLoader;

fn secret_card_json() -> String {
    r#"{
        "spec": "chara_card_v2",
        "spec_version": "2.0",
        "data": {
            "name": "Mira",
            "description": "항구 마을의 사서. 사실은 밤에만 열리는 지하 서고 열쇠를 몰래 간직한다. 차 한 잔을 좋아한다.",
            "personality": "조용하고 관찰력이 좋다. 상대가 추측보다 직접 듣게 하고 싶다.",
            "scenario": "폭풍 뒤 닫힌 도서관에서 플레이어와 마주친다. 서고 문을 열지 말지 망설인다.",
            "first_mes": "오늘은… 일찍 오셨네요.",
            "mes_example": "<START>\n{{user}}: 여기 자주 와?\n{{char}}: 가끔요.\n{{char}}: 조용히 해주세요."
        }
    }"#
    .to_string()
}

#[test]
fn json_card_enrichment_feeds_lead_ladder() {
    let temp = tempfile::tempdir().expect("tempdir");
    let card_path = temp.path().join("mira.json");
    std::fs::write(&card_path, secret_card_json()).expect("card");
    let out_dir = temp.path().join("packs/mira");

    let output = import_card(&card_path, Some(&out_dir), &ImportOptions::default()).expect("import");
    assert!(output.report.loader_verified);

    let characters = std::fs::read_to_string(out_dir.join("characters.toml")).expect("characters");
    assert!(
        characters.contains("secrets"),
        "secrets key missing: {characters}"
    );
    assert!(
        characters.contains("비밀")
            || characters.contains("몰래")
            || characters.contains("열쇠")
            || characters.contains("지하"),
        "expected a secret line: {characters}"
    );
    assert!(
        characters.contains("current_goal")
            && !characters.contains("current_goal = \"\""),
        "current_goal must be non-empty: {characters}"
    );
    assert!(
        characters.contains("avoid_phrases"),
        "avoid_phrases missing: {characters}"
    );
    assert!(
        characters.contains("대답을 정해 두고 묻는"),
        "loop trap missing: {characters}"
    );

    let rules = std::fs::read_to_string(out_dir.join("rules.toml")).expect("rules");
    assert!(rules.contains("id = \"default-location\""), "{rules}");
    assert!(rules.contains("id = \"second-location\""), "{rules}");
    assert!(rules.contains("다른 장소"), "{rules}");

    PackLoader::load(&out_dir).expect("PackLoader::load");

    assert!(
        output
            .report
            .mapped
            .iter()
            .any(|line| line.contains("enrich secrets")),
        "{:?}",
        output.report.mapped
    );
    assert!(
        output
            .report
            .mapped
            .iter()
            .any(|line| line.contains("second-location")),
        "{:?}",
        output.report.mapped
    );
}

#[test]
fn import_inbox_imports_cards_and_skips_existing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let inbox = temp.path().join("inbox");
    std::fs::create_dir_all(&inbox).expect("inbox");
    std::fs::write(inbox.join("mira.json"), secret_card_json()).expect("mira");
    std::fs::write(
        inbox.join("other.json"),
        r#"{"name":"Other","description":"d","first_mes":"hi"}"#,
    )
    .expect("other");
    // Non-card files are ignored.
    std::fs::write(inbox.join("notes.txt"), "ignore").expect("notes");

    // Default out is stories/<slug> relative to cwd — run inside temp via
    // writing cards then importing with force into known paths is awkward for
    // inbox (always stories/<slug>). Change cwd for this test.
    let prev = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(temp.path()).expect("chdir");
    let result = import_inbox(&inbox, &ImportOptions::default());
    let _ = std::env::set_current_dir(&prev);
    let result = result.expect("inbox");

    assert_eq!(result.ok.len(), 2, "errors: {:?}", result.errors);
    assert!(result.errors.is_empty());

    // Second pass without --force should skip (existing dir → error per file).
    std::env::set_current_dir(temp.path()).expect("chdir");
    let again = import_inbox(&inbox, &ImportOptions::default()).expect("inbox again");
    let _ = std::env::set_current_dir(&prev);
    assert!(again.ok.is_empty(), "should skip existing packs");
    assert_eq!(again.errors.len(), 2);

    // --force re-imports.
    let mut force = ImportOptions::default();
    force.force = true;
    std::env::set_current_dir(temp.path()).expect("chdir");
    let forced = import_inbox(&inbox, &force).expect("force inbox");
    let _ = std::env::set_current_dir(&prev);
    assert_eq!(forced.ok.len(), 2, "{:?}", forced.errors);
}
