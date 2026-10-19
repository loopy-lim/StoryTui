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
                {"type": "icon", "uri": "embeded://assets/icon/images/avatar.png", "name": "avatar", "ext": "png"},
                {"type": "background", "uri": "embeded://assets/background/images/bg.png", "name": "bg", "ext": "png"}
            ]
        }
    }"#;
    let zip_bytes = build_zip(&[
        ("card.json", card_json.as_bytes()),
        ("assets/icon/images/avatar.png", b"fakepng"),
        ("assets/background/images/bg.png", b"fakebg"),
    ]);
    let card_path = temp.path().join("seraphine.charx");
    std::fs::write(&card_path, &zip_bytes).expect("write charx");
    let out_dir = temp.path().join("pack");

    let output =
        import_card(&card_path, Some(&out_dir), &ImportOptions::default()).expect("import");
    assert!(output.report.loader_verified, "PackLoader self-check");
    // The Hangul name slugs to the `card` fallback.
    assert!(out_dir.join("images/characters/card.png").exists());
    // The background lands on the scene-image convention path.
    assert_eq!(
        std::fs::read(out_dir.join("images/locations/default-location.png")).expect("bg"),
        b"fakebg",
    );
    assert_eq!(output.report.lorebook_entries, 1);
    // The raw card JSON (with its embeded:// URIs) ships untouched.
    let raw = std::fs::read_to_string(out_dir.join("sources/card.json")).expect("raw card");
    assert!(raw.contains("embeded://"), "{raw}");
    let report = std::fs::read_to_string(out_dir.join("import-report.md")).expect("report");
    assert!(report.contains("embeded://"), "{report}");
    assert!(report.contains("배경 자산"), "{report}");
}

/// The real RisuRealm sample's background is a WEBP; prove the written file
/// decodes with the same `image` decoder the host's scene-image path uses.
#[test]
fn real_webp_background_imports_and_decodes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let webp: &[u8] = include_bytes!("fixtures/bg_ruins.webp");
    let card_json = r#"{
        "spec": "chara_card_v3",
        "data": {
            "name": "세라핀",
            "first_mes": "어서 오세요.",
            "assets": [
                {"type": "x-risu-asset", "uri": "embeded://assets/other/image/bg_ruins.webp", "name": "bg_ruins", "ext": "webp"}
            ]
        }
    }"#;
    let zip_bytes = build_zip(&[
        ("card.json", card_json.as_bytes()),
        ("assets/other/image/bg_ruins.webp", webp),
    ]);
    let card_path = temp.path().join("seraphine.charx");
    std::fs::write(&card_path, &zip_bytes).expect("write charx");
    let out_dir = temp.path().join("pack");

    import_card(&card_path, Some(&out_dir), &ImportOptions::default()).expect("import");
    let written =
        std::fs::read(out_dir.join("images/locations/default-location.webp")).expect("bg");
    assert_eq!(written, webp, "background bytes preserved verbatim");
    let decoded = image::ImageReader::new(std::io::Cursor::new(&written))
        .with_guessed_format()
        .expect("reader")
        .decode()
        .expect("webp decodes with the host's decoder");
    assert!(decoded.width() > 0 && decoded.height() > 0);
}
