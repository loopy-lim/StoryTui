//! End-to-end: a synthetic RisuAI `.charx` compiles into a pack the real
//! PackLoader accepts, with the avatar restored from `embeded://`.

use story_import::{ImportOptions, import_card};

/// Build an in-memory zip from `(name, bytes)` entries.
fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    for (name, bytes) in entries {
        writer.start_file(*name, options).expect("start file");
        std::io::Write::write_all(&mut writer, bytes).expect("write entry");
    }
    writer.finish().expect("finish zip").into_inner()
}

#[test]
fn charx_imports_to_a_loader_verified_pack() {
    let temp = tempfile::tempdir().expect("tempdir");
    let card_json = r#"{
        "spec": "chara_card_v3",
        "data": {
            "name": "세라핀",
            "description": "숲의 약사",
            "first_mes": "어서 오세요.",
            "character_book": {
                "name": "숲의 기록",
                "entries": [
                    {"keys": ["약초"], "content": "약초는 숲 북쪽에서 난다.", "enabled": true}
                ]
            },
            "assets": [
                {"type": "icon", "uri": "embeded://assets/icon/images/avatar.png", "name": "avatar", "ext": "png"}
            ]
        }
    }"#;
    let zip_bytes = build_zip(&[
        ("card.json", card_json.as_bytes()),
        ("assets/icon/images/avatar.png", b"fakepng"),
    ]);
    let card_path = temp.path().join("seraphine.charx");
    std::fs::write(&card_path, &zip_bytes).expect("write charx");
    let out_dir = temp.path().join("pack");

    let output =
        import_card(&card_path, Some(&out_dir), &ImportOptions::default()).expect("import");
    assert!(output.report.loader_verified, "PackLoader self-check");
    // The Hangul name slugs to the `card` fallback.
    assert!(out_dir.join("images/characters/card.png").exists());
    assert_eq!(output.report.lorebook_entries, 1);
    // The raw card JSON (with its embeded:// URIs) ships untouched.
    let raw = std::fs::read_to_string(out_dir.join("sources/card.json")).expect("raw card");
    assert!(raw.contains("embeded://"), "{raw}");
    let report = std::fs::read_to_string(out_dir.join("import-report.md")).expect("report");
    assert!(report.contains("embeded://"), "{report}");
}
