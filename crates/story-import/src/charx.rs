//! RisuAI CHARX (`.charx`) parsing: a zip container whose root `card.json`
//! holds a Character Card V2/V3 JSON object while the remaining entries are
//! assets referenced from the card via `embeded://<path>` URIs (Character
//! Card V3 spec, "CHARX" section).
//!
//! Policy: the card JSON goes through the same lenient [`crate::card`]
//! pipeline as every other format — the untouched JSON still ships in
//! `sources/card.json`. Only the avatar asset is unpacked from the
//! container (the pack format has no other asset sinks); everything else is
//! counted and reported.

use std::io::{Read, Seek};
use std::path::Path;

use crate::card::{AvatarImage, CardAsset, ParsedCard, parse_card_json};

/// Decompressed-size cap for entries we unpack. The spec lets applications
/// reject oversized files, and the pack can only carry a card JSON and an
/// avatar — anything larger is pathological.
const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;

/// Parse a `.charx` container: extract the root `card.json` and resolve the
/// avatar from `embeded://` asset URIs when present.
pub fn parse_charx(bytes: &[u8]) -> Result<ParsedCard, String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|error| {
        format!("CHARX zip을 열지 못했습니다 (손상되었거나 zip이 아닙니다): {error}")
    })?;

    let card_text = read_card_json(&mut archive)?;
    let mut parsed = parse_card_json(&card_text)?;
    resolve_embeded_avatar(&mut parsed, &mut archive);
    resolve_embeded_background(&mut parsed, &mut archive);
    // Count the assets the pack could not carry, now that avatar and
    // background have been consumed.
    let consumed = usize::from(parsed.canonical.avatar.is_some())
        + usize::from(parsed.canonical.background.is_some());
    let others = parsed.card_data.assets.len().saturating_sub(consumed);
    if others > 0 {
        parsed.notes.push((
            "assets".to_string(),
            format!("아바타·배경 외 CHARX 자산 {others}개는 미지원 (원본 card.json에 보존)"),
        ));
    }
    note_container_files(&mut parsed, &mut archive);
    Ok(parsed)
}

/// Read the root `card.json`, tolerating `./card.json`-style spellings and
/// nested copies. The spec names `card.json` at the zip root; the scan is a
/// leniency for exporters that prepend a path segment.
fn read_card_json<R: Read + Seek>(archive: &mut zip::ZipArchive<R>) -> Result<String, String> {
    if let Ok(mut file) = archive.by_name("card.json") {
        return read_zip_text(&mut file);
    }
    for index in 0..archive.len() {
        let mut file = match archive.by_index(index) {
            Ok(file) => file,
            Err(error) => return Err(entry_scan_error(&error)),
        };
        if let Ok(name) = file.name()
            && !name.ends_with('/')
            && name.ends_with("card.json")
        {
            return read_zip_text(&mut file);
        }
    }
    Err("CHARX 안에 card.json이 없습니다. 캐릭터 카드 CHARX 파일이 맞는지 확인하세요.".to_string())
}

/// Map a failed entry read to a Korean message; encrypted archives get
/// their own guidance instead of the raw zip error.
fn entry_scan_error(error: &zip::result::ZipError) -> String {
    if error.to_string().to_ascii_lowercase().contains("password") {
        "암호화된 CHARX는 지원하지 않습니다. 암호를 제거한 zip으로 다시 내보내 주세요.".to_string()
    } else {
        format!("CHARX zip을 훑지 못했습니다: {error}")
    }
}

fn read_zip_text<R: Read>(file: &mut zip::read::ZipFile<'_, R>) -> Result<String, String> {
    if file.size() > MAX_ENTRY_BYTES {
        return Err(format!(
            "CHARX의 card.json이 너무 큽니다 (압축 해제 상한 {MAX_ENTRY_BYTES}바이트 초과)"
        ));
    }
    let mut text = String::new();
    file.read_to_string(&mut text).map_err(|error| {
        format!(
            "CHARX의 card.json을 읽지 못했습니다 (암호화되었거나 손상되었을 수 있습니다): {error}"
        )
    })?;
    Ok(text)
}

/// Resolve the avatar from `embeded://` asset URIs against the container's
/// entries. A missing or unreadable path keeps the import alive — the note
/// the card parser already recorded is sharpened instead.
fn resolve_embeded_avatar<R: Read + Seek>(
    parsed: &mut ParsedCard,
    archive: &mut zip::ZipArchive<R>,
) {
    // A data-URI avatar already extracted wins, matching the PNG-path
    // convention where the embedded data is primary.
    if parsed.canonical.avatar.is_some() {
        return;
    }
    let candidates: Vec<&CardAsset> = parsed
        .card_data
        .assets
        .iter()
        .filter(|asset| asset.uri.starts_with("embeded://") && asset.is_avatar())
        .collect();
    let Some(asset) = candidates
        .iter()
        .find(|asset| asset.name.eq_ignore_ascii_case("main"))
        .or_else(|| candidates.first())
        .copied()
    else {
        return;
    };
    let Some(path) = asset.uri.strip_prefix("embeded://").map(str::to_string) else {
        return;
    };

    // The "external URI" note parse_card_json left behind no longer applies.
    parsed
        .notes
        .retain(|(subject, _)| subject != "assets.avatar");

    let Some((bytes, entry_path)) = read_entry(archive, &path) else {
        parsed.notes.push((
            "assets.avatar".to_string(),
            format!("embeded:// 경로를 CHARX에서 찾지 못했습니다 ({path}) — 아바타 생략"),
        ));
        return;
    };
    parsed.canonical.avatar = Some(AvatarImage {
        bytes,
        extension: image_extension(&entry_path),
    });
    parsed.notes.push((
        "assets.avatar".to_string(),
        "CHARX embeded:// 자산에서 아바타를 복원했습니다".to_string(),
    ));
}

/// Resolve the background scene image from `embeded://` asset URIs: the V3
/// `background` type or Risu's `bg*`-named image assets. The scaffold writes
/// it to `images/locations/` (the host's scene-image convention).
fn resolve_embeded_background<R: Read + Seek>(
    parsed: &mut ParsedCard,
    archive: &mut zip::ZipArchive<R>,
) {
    if parsed.canonical.background.is_some() {
        return;
    }
    let candidates: Vec<&CardAsset> = parsed
        .card_data
        .assets
        .iter()
        .filter(|asset| {
            asset.uri.starts_with("embeded://")
                && (asset.asset_kind().contains("background")
                    || asset.name.to_ascii_lowercase().starts_with("bg"))
                && is_image_path(asset.uri.strip_prefix("embeded://").unwrap_or(&asset.uri))
        })
        .collect();
    let Some(asset) = candidates.first().copied() else {
        return;
    };
    let Some(path) = asset.uri.strip_prefix("embeded://").map(str::to_string) else {
        return;
    };

    // The "external URI" note parse_card_json left behind no longer applies.
    parsed
        .notes
        .retain(|(subject, _)| subject != "assets.background");

    let Some((bytes, entry_path)) = read_entry(archive, &path) else {
        parsed.notes.push((
            "assets.background".to_string(),
            format!("embeded:// 경로를 CHARX에서 찾지 못했습니다 ({path}) — 배경 생략"),
        ));
        return;
    };
    parsed.canonical.background = Some(AvatarImage {
        bytes,
        extension: image_extension(&entry_path),
    });
    parsed.notes.push((
        "assets.background".to_string(),
        "CHARX embeded:// 자산에서 배경 이미지를 복원했습니다".to_string(),
    ));
}

/// Report container files the pack cannot carry (Risu modules, RisuAI
/// x_meta, extra assets): the policy is recorded reduction, never silent
/// loss. The untouched `.charx` itself is the preservation medium.
fn note_container_files<R: Read + Seek>(parsed: &mut ParsedCard, archive: &mut zip::ZipArchive<R>) {
    let mut others: Vec<String> = Vec::new();
    for index in 0..archive.len() {
        let Ok(file) = archive.by_index(index) else {
            continue;
        };
        let Ok(name) = file.name() else {
            continue;
        };
        let name = name.trim_end_matches('/');
        if name != "card.json" {
            others.push(name.to_string());
        }
    }
    if others.is_empty() {
        return;
    }
    let count = others.len();
    let sample = match others.iter().find(|name| name.ends_with(".risum")) {
        Some(module) => format!(" (예: {module})"),
        None => {
            others.sort();
            format!(" (예: {})", others[0])
        }
    };
    parsed.notes.push((
        "container".to_string(),
        format!("CHARX 컨테이너의 기타 파일 {count}개{sample}는 미지원 — 원본 .charx에 보존"),
    ));
}

/// Tolerant entry lookup + read (`./` prefixes, path case; exact names match
/// the same scan). Returns the bytes and the stored path, which the
/// extension inference needs. Unreadable entries (damaged, encrypted, or an
/// unsupported codec) are skipped, not fatal — a later entry may still hold
/// the asset.
fn read_entry<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    path: &str,
) -> Option<(Vec<u8>, String)> {
    let wanted = path.trim_start_matches("./").trim_start_matches('/');
    let lowered = wanted.to_ascii_lowercase();
    for index in 0..archive.len() {
        let mut entry = match archive.by_index(index) {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let Ok(name) = entry.name() else { continue };
        let name = name.trim_start_matches("./").trim_start_matches('/');
        if name.eq_ignore_ascii_case(&lowered) {
            if entry.size() > MAX_ENTRY_BYTES {
                return None;
            }
            // Own the path first: the `name` borrow ends before the entry is
            // read mutably.
            let stored = name.to_string();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).ok()?;
            return Some((bytes, stored));
        }
    }
    None
}

/// Recognizable image path by extension — a Risu `bg*` filter must not
/// swallow non-images (e.g. a font named `bg_font.woff2`).
fn is_image_path(path: &str) -> bool {
    matches!(
        Path::new(path)
            .extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "webp" | "avif" | "gif")
    )
}

/// Pack-storable image extension from a stored path; unknown extensions fall
/// back to `png` (the scaffold writes `images/characters/<id>.<ext>`).
fn image_extension(path: &str) -> &'static str {
    let extension = Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match extension.as_str() {
        "jpg" | "jpeg" => "jpg",
        "webp" => "webp",
        "avif" => "avif",
        "gif" => "gif",
        _ => "png",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;

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

    const CARD_JSON: &str = r#"{
        "spec": "chara_card_v3",
        "data": {
            "name": "세라핀",
            "description": "숲의 약사",
            "first_mes": "어서 오세요.",
            "assets": [
                {"type": "icon", "uri": "embeded://assets/icon/images/avatar.png", "name": "avatar", "ext": "png"},
                {"type": "background", "uri": "embeded://assets/background/images/bg.png", "name": "bg", "ext": "png"},
                {"type": "x-risu-asset", "uri": "embeded://assets/other/fonts/font_hand.woff2", "name": "font_hand", "ext": "woff2"}
            ]
        }
    }"#;

    #[test]
    fn parses_card_json_and_restores_embeded_avatar() {
        let bytes = build_zip(&[
            ("card.json", CARD_JSON.as_bytes()),
            ("assets/icon/images/avatar.png", b"fakepng"),
            ("assets/background/images/bg.png", b"fakebg"),
            ("assets/other/fonts/font_hand.woff2", b"fakefont"),
        ]);
        let parsed = parse_charx(&bytes).expect("charx");
        assert_eq!(parsed.canonical.name, "세라핀");
        assert_eq!(parsed.canonical.first_mes, "어서 오세요.");
        let avatar = parsed.canonical.avatar.expect("avatar");
        assert_eq!(avatar.bytes, b"fakepng");
        assert_eq!(avatar.extension, "png");
        let background = parsed.canonical.background.expect("background");
        assert_eq!(background.bytes, b"fakebg");
        assert_eq!(background.extension, "png");
        assert!(
            parsed
                .notes
                .iter()
                .any(|(subject, reason)| subject == "assets.avatar" && reason.contains("복원")),
            "restored note missing: {:?}",
            parsed.notes
        );
        assert!(
            parsed
                .notes
                .iter()
                .any(|(subject, reason)| subject == "assets.background" && reason.contains("복원")),
            "background note missing: {:?}",
            parsed.notes
        );
        // Only the font remains unsupported: avatar + background consumed.
        assert!(
            parsed
                .notes
                .iter()
                .any(|(subject, reason)| subject == "assets"
                    && reason.contains("1개는 미지원")
                    && reason.contains("아바타·배경 외")),
            "other-assets note missing: {:?}",
            parsed.notes
        );
    }

    #[test]
    fn risu_bg_named_asset_resolves_as_background() {
        // Risu containers mark backgrounds as type "x-risu-asset" with a
        // `bg*` name; the font must NOT be mistaken for one.
        let card = r#"{"spec":"chara_card_v3","data":{"name":"아","assets":[
            {"type":"x-risu-asset","uri":"embeded://assets/other/image/bg_ruins.webp","name":"bg_ruins","ext":"webp"},
            {"type":"x-risu-asset","uri":"embeded://assets/other/fonts/bg_font.woff2","name":"bg_font","ext":"woff2"}
        ]}}"#;
        let bytes = build_zip(&[
            ("card.json", card.as_bytes()),
            ("assets/other/image/bg_ruins.webp", b"fakewebp"),
            ("assets/other/fonts/bg_font.woff2", b"fakefont"),
        ]);
        let parsed = parse_charx(&bytes).expect("charx");
        let background = parsed.canonical.background.expect("background");
        assert_eq!(background.bytes, b"fakewebp");
        assert_eq!(background.extension, "webp");
        assert!(parsed.canonical.avatar.is_none());
    }

    #[test]
    fn missing_embeded_path_is_reported_not_fatal() {
        let bytes = build_zip(&[("card.json", CARD_JSON.as_bytes())]);
        let parsed = parse_charx(&bytes).expect("charx");
        assert!(parsed.canonical.avatar.is_none());
        assert!(
            parsed
                .notes
                .iter()
                .any(|(_, reason)| reason.contains("찾지 못했습니다")),
            "missing-path note missing: {:?}",
            parsed.notes
        );
    }

    #[test]
    fn data_uri_avatar_wins_over_embeded() {
        let encoded = base64::engine::general_purpose::STANDARD.encode(b"datapng");
        let card = format!(
            r#"{{"spec":"chara_card_v3","data":{{"name":"아","assets":[
                {{"type":"icon","uri":"data:image/png;base64,{encoded}"}},
                {{"type":"icon","uri":"embeded://assets/icon/images/avatar.png"}}
            ]}}}}"#
        );
        let bytes = build_zip(&[
            ("card.json", card.as_bytes()),
            ("assets/icon/images/avatar.png", b"fakepng"),
        ]);
        let parsed = parse_charx(&bytes).expect("charx");
        let avatar = parsed.canonical.avatar.expect("avatar");
        assert_eq!(avatar.bytes, b"datapng");
    }

    #[test]
    fn main_named_embeded_asset_is_preferred() {
        let card = r#"{"spec":"chara_card_v3","data":{"name":"아","assets":[
            {"type":"icon","uri":"embeded://assets/icon/images/alt.png"},
            {"type":"icon","uri":"embeded://assets/icon/images/main.png","name":"main"}
        ]}}"#;
        let bytes = build_zip(&[
            ("card.json", card.as_bytes()),
            ("assets/icon/images/alt.png", b"altbytes"),
            ("assets/icon/images/main.png", b"mainbytes"),
        ]);
        let parsed = parse_charx(&bytes).expect("charx");
        let avatar = parsed.canonical.avatar.expect("avatar");
        assert_eq!(avatar.bytes, b"mainbytes");
    }

    #[test]
    fn container_files_beyond_card_json_are_reported() {
        let bytes = build_zip(&[
            ("card.json", CARD_JSON.as_bytes()),
            ("assets/icon/images/avatar.png", b"fakepng"),
            ("module.risum", b"risu-module"),
        ]);
        let parsed = parse_charx(&bytes).expect("charx");
        assert!(
            parsed
                .notes
                .iter()
                .any(|(subject, reason)| subject == "container"
                    && reason.contains("module.risum")
                    && reason.contains("2개")),
            "container note missing: {:?}",
            parsed.notes
        );
    }

    #[test]
    fn zip_without_card_json_is_rejected() {
        let bytes = build_zip(&[("assets/icon/images/avatar.png", b"x")]);
        let error = parse_charx(&bytes).unwrap_err();
        assert!(error.contains("card.json"), "{error}");
    }

    #[test]
    fn non_zip_bytes_get_a_clear_error() {
        let error = parse_charx(b"definitely not a zip").unwrap_err();
        assert!(error.contains("zip"), "{error}");
    }
}
