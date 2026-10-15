//! Character card parsing: SillyTavern/RisuAI V1/V2/V3 cards from `.png`
//! (embedded tEXt JSON) or `.json` files.
//!
//! Policy: the parsed JSON is kept verbatim (`sources/card.json` receives the
//! original content), so lenient parsing here never destroys card data —
//! anything this module does not map is reported by the caller instead.
//! Parsing itself owns no report: it returns the data plus the notes the
//! caller's report should carry.

use base64::Engine as _;
use serde::Deserialize;

use crate::worldinfo::{self, NormalizedBook};

/// A decoded card ready for scaffolding.
pub struct ParsedCard {
    /// Normalized card content (macro substitution happens later, in
    /// [`crate::scaffold`], against the configured player address).
    pub canonical: CanonicalCard,
    /// The raw card DTO, kept so the caller's report can record policy notes
    /// (ignored `system_prompt`, external avatars, ...).
    pub card_data: CardData,
    /// Parse-time report notes (book approximations, avatar issues) as
    /// `(subject, reason)` pairs, folded into the scaffold report.
    pub notes: Vec<(String, String)>,
    /// The original card JSON, preserved byte-for-byte in the pack.
    pub raw: serde_json::Value,
    /// Detected spec label for the report (`chara_card_v2`, `v1`, ...).
    pub spec_label: String,
}

/// Normalized card content across V1/V2/V3.
pub struct CanonicalCard {
    pub name: String,
    pub description: String,
    pub personality: String,
    pub scenario: String,
    pub first_mes: String,
    pub alternate_greetings: Vec<String>,
    pub mes_example: String,
    /// Embedded lorebook, normalized from the V2 `character_book`.
    pub book: Option<NormalizedBook>,
    /// Embedded avatar image (the PNG itself, or a V3 data-URI asset).
    pub avatar: Option<AvatarImage>,
}

/// A decoded avatar ready to be written under `images/characters/`.
pub struct AvatarImage {
    pub bytes: Vec<u8>,
    pub extension: &'static str,
}

/// Load and parse a card file (`.png` with tEXt JSON, or `.json`).
pub fn load_card(path: &std::path::Path) -> Result<ParsedCard, String> {
    let bytes = std::fs::read(path)
        .map_err(|e| format!("카드 파일을 읽지 못했습니다 ({}): {e}", path.display()))?;
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match extension.as_str() {
        "png" => parse_card_png(&bytes),
        "json" => {
            let text =
                String::from_utf8(bytes).map_err(|_| "카드 JSON이 UTF-8이 아닙니다".to_string())?;
            parse_card_json(&text)
        }
        other => Err(format!(
            "지원하지 않는 카드 확장자: .{other} — .png 또는 .json 카드를 사용하세요"
        )),
    }
}

/// Parse a card from PNG bytes by extracting the `chara`/`ccv3` tEXt chunk.
/// The PNG itself becomes the avatar unless a V3 data-URI asset replaces it.
pub fn parse_card_png(bytes: &[u8]) -> Result<ParsedCard, String> {
    let json = extract_card_json_from_png(bytes)?;
    let mut parsed = parse_card_json(&json)?;
    if parsed.canonical.avatar.is_none() {
        parsed.canonical.avatar = Some(AvatarImage {
            bytes: bytes.to_vec(),
            extension: "png",
        });
    }
    Ok(parsed)
}

/// Parse a card from its JSON text (V1 flat, V2/V3 nested).
pub fn parse_card_json(text: &str) -> Result<ParsedCard, String> {
    let raw: serde_json::Value = serde_json::from_str(text.trim())
        .map_err(|e| format!("카드 JSON을 해석하지 못했습니다: {e}"))?;
    let spec_label = raw
        .get("spec")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("v1")
        .to_string();
    if raw.get("entries").is_some() && raw.get("name").is_none() && raw.get("data").is_none() {
        return Err(
            "이 파일은 캐릭터 카드가 아니라 World Info(로어북) 내보내기로 보입니다. \
             --lorebook 옵션으로 붙이세요."
                .to_string(),
        );
    }
    let data = card_data_view(&raw);
    let mut card: CardData = serde_json::from_value(data.clone())
        .map_err(|e| format!("카드 필드를 해석하지 못했습니다: {e}"))?;
    if card.name.trim().is_empty() {
        return Err(
            "카드에 name이 없습니다. 손상된 카드이거나 지원하지 않는 형식입니다.".to_string(),
        );
    }

    let mut notes = Vec::new();
    let book = card
        .character_book
        .take()
        .map(|book| worldinfo::normalize_embedded_book(book, &mut notes));
    let (avatar, avatar_notes) = extract_asset_avatar(&card.assets);
    notes.extend(avatar_notes);

    let canonical = CanonicalCard {
        name: card.name.clone(),
        description: card.description.clone(),
        personality: card.personality.clone(),
        scenario: card.scenario.clone(),
        first_mes: card.first_mes.clone(),
        alternate_greetings: card.alternate_greetings.clone(),
        mes_example: card.mes_example.clone(),
        book,
        avatar,
    };

    Ok(ParsedCard {
        canonical,
        card_data: card,
        notes,
        raw,
        spec_label,
    })
}

/// Permissive card data: every field defaults so partially-broken cards still
/// import, while the raw JSON keeps everything.
#[derive(Default, Deserialize)]
#[serde(default)]
pub struct CardData {
    pub name: String,
    pub description: String,
    pub personality: String,
    pub scenario: String,
    pub first_mes: String,
    pub mes_example: String,
    pub creator_notes: String,
    pub system_prompt: String,
    pub post_history_instructions: String,
    pub alternate_greetings: Vec<String>,
    pub group_only_greetings: Vec<String>,
    pub character_book: Option<CardBook>,
    pub assets: Vec<CardAsset>,
}

/// V2/V3 embedded `character_book` DTO (spec shape + tolerated extras).
#[derive(Default, Deserialize)]
#[serde(default)]
pub struct CardBook {
    pub name: String,
    pub scan_depth: Option<u32>,
    pub token_budget: Option<u32>,
    pub recursive_scanning: bool,
    pub entries: Vec<CardBookEntry>,
}

/// One embedded-book entry. `position`/`logic`/`probability` may live at the
/// top level (V2 strings) or inside `extensions` (SillyTavern numeric export).
#[derive(Default, Deserialize)]
#[serde(default)]
pub struct CardBookEntry {
    pub keys: Vec<String>,
    pub secondary_keys: Vec<String>,
    pub content: String,
    pub enabled: Option<bool>,
    pub insertion_order: Option<i64>,
    pub case_sensitive: Option<bool>,
    pub selective: Option<bool>,
    pub constant: Option<bool>,
    pub position: Option<serde_json::Value>,
    pub priority: Option<i64>,
    pub comment: Option<String>,
    pub extensions: serde_json::Value,
}

/// V3 asset entry (`avatar` / `icon` / `other` / `cc3-*`).
#[derive(Default, Deserialize)]
#[serde(default)]
pub struct CardAsset {
    pub kind: String,
    pub uri: String,
    /// V3 spells the field `type`; `kind` kept for leniency.
    pub r#type: Option<String>,
}

/// `data` object for V2/V3 cards, top level for V1 (or spec-less V2 exports).
fn card_data_view(root: &serde_json::Value) -> &serde_json::Value {
    root.get("data")
        .filter(|data| data.is_object())
        .unwrap_or(root)
}

/// Extract the base64 card JSON from `chara`/`ccv3` tEXt chunks. `ccv3` wins
/// over `chara` when both are present (V3 cards embed both for fallback).
fn extract_card_json_from_png(bytes: &[u8]) -> Result<String, String> {
    const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if !bytes.starts_with(SIGNATURE) {
        return Err("PNG 시그니처가 아닙니다. 손상된 파일이거나 PNG 카드가 아닙니다.".to_string());
    }
    let mut found_chara: Option<String> = None;
    let mut saw_compressed = false;
    let mut pos = SIGNATURE.len();
    while pos + 8 <= bytes.len() {
        let length =
            u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
                as usize;
        let chunk_type = &bytes[pos + 4..pos + 8];
        let data_start = pos + 8;
        if data_start + length + 4 > bytes.len() {
            break; // truncated chunk: stop scanning, report below
        }
        let data = &bytes[data_start..data_start + length];
        match chunk_type {
            b"tEXt" => {
                let keyword_len = data.iter().position(|&byte| byte == 0).unwrap_or(0);
                let (keyword, text) = data.split_at(keyword_len);
                // `text` still starts with the NUL separator; skip exactly one.
                let text = &text[usize::min(1, text.len())..];
                if keyword == b"ccv3" || keyword == b"chara" {
                    // tEXt payloads are Latin-1; card base64 is ASCII, so a
                    // lossy decode is always safe here.
                    let decoded = decode_card_base64(&String::from_utf8_lossy(text))?;
                    if keyword == b"ccv3" {
                        return Ok(decoded);
                    }
                    if found_chara.is_none() {
                        found_chara = Some(decoded);
                    }
                }
            }
            b"zTXt" | b"iTXt" => saw_compressed = true,
            b"IEND" => break,
            _ => {}
        }
        pos = data_start + length + 4; // skip payload + CRC
    }
    found_chara.ok_or_else(|| {
        if saw_compressed {
            "카드 JSON이 zTXt/iTXt 청크에 담겨 있습니다(미지원). tEXt 카드를 사용하거나 \
             JSON으로 내보낸 파일을 임포트하세요."
                .to_string()
        } else {
            "PNG에서 카드 데이터를 찾지 못했습니다 (tEXt 'chara'/'ccv3' 청크 없음). \
             이 PNG는 캐릭터 카드가 아닐 수 있습니다."
                .to_string()
        }
    })
}

/// Standard base64 with whitespace tolerated; URL-safe accepted as a fallback
/// (some third-party exporters emit it despite the spec).
fn decode_card_base64(text: &str) -> Result<String, String> {
    let cleaned: String = text.chars().filter(|ch| !ch.is_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(cleaned.as_bytes())
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(cleaned.as_bytes()))
        .map_err(|e| format!("카드 tEXt 청크의 base64를 디코드하지 못했습니다: {e}"))?;
    String::from_utf8(bytes).map_err(|_| "카드 JSON이 UTF-8이 아닙니다".to_string())
}

/// Pick an embedded avatar from V3 `assets` (data URI only; external URIs are
/// reported, never fetched). Returns the image plus report notes.
fn extract_asset_avatar(assets: &[CardAsset]) -> (Option<AvatarImage>, Vec<(String, String)>) {
    let mut notes = Vec::new();
    for asset in assets {
        let kind = asset
            .r#type
            .as_deref()
            .or(Some(asset.kind.as_str()))
            .unwrap_or("");
        let is_avatar = kind.contains("avatar") || kind == "icon";
        if !is_avatar {
            continue;
        }
        if let Some(rest) = asset.uri.strip_prefix("data:") {
            let (mime, payload) = rest.split_once(',').unwrap_or(("", rest));
            let extension = if mime.contains("jpeg") || mime.contains("jpg") {
                "jpg"
            } else if mime.contains("webp") {
                "webp"
            } else {
                "png"
            };
            if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(payload.trim()) {
                return (Some(AvatarImage { bytes, extension }), notes);
            }
            notes.push((
                "assets.avatar".to_string(),
                "data URI 디코딩 실패 — 아바타 생략".to_string(),
            ));
            return (None, notes);
        }
        notes.push((
            "assets.avatar".to_string(),
            "외부 URI 아바타는 내려받지 않습니다 (data URI만 지원)".to_string(),
        ));
    }
    (None, notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_chara_text_chunk_from_synthetic_png() {
        let json = r#"{"name":"유나","description":"d"}"#;
        let encoded = base64::engine::general_purpose::STANDARD.encode(json);
        let mut png = Vec::from(&SIGNATURE[..]);
        push_chunk(&mut png, b"IHDR", &[0; 4]);
        let mut text = Vec::from(&b"chara"[..]);
        text.push(0);
        text.extend_from_slice(encoded.as_bytes());
        push_chunk(&mut png, b"tEXt", &text);
        push_chunk(&mut png, b"IEND", &[]);
        assert_eq!(extract_card_json_from_png(&png).unwrap(), json);
    }

    #[test]
    fn ccv3_chunk_wins_over_chara() {
        let v2 = base64::engine::general_purpose::STANDARD.encode(r#"{"name":"v2"}"#);
        let v3 = base64::engine::general_purpose::STANDARD.encode(r#"{"name":"v3"}"#);
        let mut png = Vec::from(&SIGNATURE[..]);
        let mut text2 = Vec::from(&b"chara"[..]);
        text2.push(0);
        text2.extend_from_slice(v2.as_bytes());
        push_chunk(&mut png, b"tEXt", &text2);
        let mut text3 = Vec::from(&b"ccv3"[..]);
        text3.push(0);
        text3.extend_from_slice(v3.as_bytes());
        push_chunk(&mut png, b"tEXt", &text3);
        push_chunk(&mut png, b"IEND", &[]);
        let json = extract_card_json_from_png(&png).unwrap();
        assert!(json.contains("v3"));
    }

    #[test]
    fn compressed_chunks_get_a_clear_error() {
        let mut png = Vec::from(&SIGNATURE[..]);
        push_chunk(&mut png, b"zTXt", &[0, 1, 2]);
        push_chunk(&mut png, b"IEND", &[]);
        let error = extract_card_json_from_png(&png).unwrap_err();
        assert!(error.contains("zTXt"), "{error}");
    }

    #[test]
    fn v1_flat_card_parses_without_data_object() {
        let parsed = parse_card_json(r#"{"name":"나","description":"설명","first_mes":"안녕"}"#)
            .expect("v1 card");
        assert_eq!(parsed.spec_label, "v1");
        assert_eq!(parsed.canonical.name, "나");
        assert_eq!(parsed.canonical.first_mes, "안녕");
    }

    #[test]
    fn world_info_json_gets_a_guidance_error() {
        let error = parse_card_json(r#"{"entries": {"0": {"content": "x"}}}"#)
            .err()
            .expect("world info must be rejected");
        assert!(error.contains("--lorebook"), "{error}");
    }

    #[test]
    fn v3_data_uri_avatar_decodes() {
        let payload = base64::engine::general_purpose::STANDARD.encode(b"fakepng");
        let card = serde_json::json!({
            "spec": "chara_card_v3",
            "data": {
                "name": "아",
                "assets": [{
                    "type": "avatar",
                    "uri": format!("data:image/png;base64,{payload}"),
                }],
            },
        });
        let parsed = parse_card_json(&card.to_string()).expect("v3 card");
        let avatar = parsed.canonical.avatar.expect("data uri avatar");
        assert_eq!(avatar.extension, "png");
        assert_eq!(avatar.bytes, b"fakepng");
    }

    // --- synthetic PNG helpers (no image decoding; chunk framing only) ------

    const SIGNATURE: [u8; 8] = *b"\x89PNG\r\n\x1a\n";

    fn push_chunk(out: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(chunk_type);
        out.extend_from_slice(data);
        out.extend_from_slice(&[0, 0, 0, 0]); // CRC placeholder: the scanner skips it
    }
}
