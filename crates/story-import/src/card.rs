//! Character card parsing: SillyTavern/RisuAI V1/V2/V3 cards from `.png`
//! (embedded tEXt JSON), `.json`, or RisuAI `.charx` zip containers.
//! Character.AI community-dump field aliases (`greeting`, `definition`,
//! `{"character": ...}` wrapper, ...) collapse onto the standard vocabulary.
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
#[derive(Debug)]
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
#[derive(Debug)]
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
    /// Embedded background scene image (V3 `background` asset or a Risu
    /// `bg*` asset). The scaffold writes it to `images/locations/` so the
    /// host's scene-image convention picks it up.
    pub background: Option<AvatarImage>,
}

/// A decoded avatar ready to be written under `images/characters/`.
#[derive(Debug)]
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
        "charx" => crate::charx::parse_charx(&bytes),
        other => Err(format!(
            "지원하지 않는 카드 확장자: .{other} — .png, .json 또는 .charx 카드를 사용하세요"
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
    let mut working = raw.clone();
    let mut notes = Vec::new();
    apply_character_wrapper(&mut working, &mut notes);
    // Alias fills target the card-data vocabulary (the `data` object for
    // V2/V3, the root for V1/dumps) so stray root keys on spec cards never
    // produce false report notes.
    if let Some(view) = card_data_view_mut(&mut working) {
        apply_character_alias_fills(view, &mut notes);
    }
    let data = card_data_view(&working);
    let mut card: CardData = serde_json::from_value(data.clone())
        .map_err(|e| format!("카드 필드를 해석하지 못했습니다: {e}"))?;
    if card.name.trim().is_empty() {
        return Err(
            "카드에 name이 없습니다. 손상된 카드이거나 지원하지 않는 형식입니다.".to_string(),
        );
    }

    let book = card
        .character_book
        .take()
        .map(|book| worldinfo::normalize_embedded_book(book, &mut notes));
    let (avatar, avatar_notes) = extract_asset_avatar(&card.assets);
    notes.extend(avatar_notes);
    let (background, background_notes) = extract_asset_background(&card.assets);
    notes.extend(background_notes);

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
        background,
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
#[derive(Debug, Default, Deserialize)]
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
#[derive(Debug, Default, Deserialize)]
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
#[derive(Debug, Default, Deserialize)]
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
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct CardAsset {
    pub kind: String,
    pub uri: String,
    /// V3 spells the field `type`; `kind` kept for leniency.
    pub r#type: Option<String>,
    /// V3 asset display name; `main` marks the primary avatar.
    pub name: String,
}

impl CardAsset {
    /// Spec `type` (or lenient `kind`) spelling of the asset type.
    pub(crate) fn asset_kind(&self) -> &str {
        self.r#type.as_deref().unwrap_or(self.kind.as_str())
    }

    /// `true` for avatar/icon assets — the only assets the pack can carry.
    pub(crate) fn is_avatar(&self) -> bool {
        let kind = self.asset_kind();
        kind.contains("avatar") || kind == "icon"
    }
}

/// `data` object for V2/V3 cards, top level for V1 (or spec-less V2 exports).
fn card_data_view(root: &serde_json::Value) -> &serde_json::Value {
    root.get("data")
        .filter(|data| data.is_object())
        .unwrap_or(root)
}

/// Mutable [`card_data_view`] for the alias pass.
fn card_data_view_mut(
    root: &mut serde_json::Value,
) -> Option<&mut serde_json::Map<String, serde_json::Value>> {
    if root.get("data").is_some_and(|data| data.is_object()) {
        root.get_mut("data").and_then(|data| data.as_object_mut())
    } else {
        root.as_object_mut()
    }
}

/// Map Character.AI community-dump field aliases onto the standard card
/// vocabulary, on a working copy (the raw JSON keeps the original dump).
/// Character.AI has no official export; the shapes below are what its
/// download extensions emit (e.g. CAI Tools' raw `character` payload and
/// pygmalion-style hybrid dumps).
///
/// Filling rules: only when the standard field is absent, null, or empty —
/// a spec card that already carries `first_mes` is never overwritten — and
/// every application is recorded as a report note.
/// Unwrap the `{"character": {...}}` API-response wrapper: only when the
/// root carries no card vocabulary of its own, so a real card with an
/// unrelated `character` key is never disturbed.
fn apply_character_wrapper(root: &mut serde_json::Value, notes: &mut Vec<(String, String)>) {
    if let Some(character) = root.get("character").cloned() {
        let is_dump = root.get("data").is_none()
            && root.get("name").is_none()
            && root.get("spec").is_none()
            && character
                .get("name")
                .is_some_and(serde_json::Value::is_string);
        if is_dump {
            *root = character;
            notes.push((
                "character".to_string(),
                "(c.ai 덤프) character 래퍼를 벗겼습니다".to_string(),
            ));
        }
    }
}

/// Fill empty standard fields from c.ai dump aliases. Every application is
/// recorded as a report note; spec-card vocabulary is never overwritten.
fn apply_character_alias_fills(
    view: &mut serde_json::Map<String, serde_json::Value>,
    notes: &mut Vec<(String, String)>,
) {
    const ALIASES: &[(&str, &str)] = &[
        ("greeting", "first_mes"),
        ("char_greeting", "first_mes"),
        ("definition", "mes_example"),
        ("example_dialogue", "mes_example"),
        ("char_persona", "description"),
        ("world_scenario", "scenario"),
        ("title", "personality"),
    ];
    for (alias, target) in ALIASES {
        let Some(alias_text) = view
            .get(*alias)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
        else {
            continue;
        };
        let target_empty = match view.get(*target) {
            None | Some(serde_json::Value::Null) => true,
            Some(serde_json::Value::String(text)) => text.trim().is_empty(),
            Some(_) => false,
        };
        if target_empty {
            view.insert((*target).to_string(), alias_text.to_string().into());
            notes.push((
                format!("card.{alias}"),
                format!("(c.ai 덤프) {alias} → {target} 매핑"),
            ));
        }
    }
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

/// Pick an embedded background from V3 typed `background` assets (data URI
/// only). CHARX containers resolve `embeded://` backgrounds themselves (see
/// [`crate::charx`]), so those URIs stay silent here. Returns the image
/// plus report notes; reuses [`AvatarImage`] (bytes + extension).
fn extract_asset_background(assets: &[CardAsset]) -> (Option<AvatarImage>, Vec<(String, String)>) {
    let mut notes = Vec::new();
    for asset in assets {
        if !asset.asset_kind().contains("background") {
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
                "assets.background".to_string(),
                "data URI 디코딩 실패 — 배경 생략".to_string(),
            ));
            return (None, notes);
        }
        if !asset.uri.starts_with("embeded://") {
            notes.push((
                "assets.background".to_string(),
                "외부 URI 배경은 내려받지 않습니다 (data URI·CHARX embeded만 지원)".to_string(),
            ));
        }
        return (None, notes);
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
    fn cai_raw_dump_aliases_fill_empty_fields() {
        let dump = r#"{"name":"세라","description":"AI 동료","greeting":"오셨군요.","definition":"<START>\n{{user}}: 안녕\n{{char}}: 반가워","title":"호기심 많은 AI"}"#;
        let parsed = parse_card_json(dump).expect("c.ai dump");
        assert_eq!(parsed.canonical.first_mes, "오셨군요.");
        assert!(parsed.canonical.mes_example.contains("반가워"));
        assert_eq!(parsed.canonical.personality, "호기심 많은 AI");
        assert!(
            parsed
                .notes
                .iter()
                .any(|(_, reason)| reason.contains("greeting → first_mes")),
            "alias note missing: {:?}",
            parsed.notes
        );
        // The raw JSON keeps the original dump, aliases applied only on the
        // working copy.
        assert!(parsed.raw.get("greeting").is_some());
        assert!(parsed.raw.get("first_mes").is_none());
    }

    #[test]
    fn character_wrapper_is_unwrapped() {
        let dump = r#"{"character":{"name":"보","greeting":"반갑다","definition":""}}"#;
        let parsed = parse_card_json(dump).expect("wrapped dump");
        assert_eq!(parsed.canonical.name, "보");
        assert_eq!(parsed.canonical.first_mes, "반갑다");
        assert!(
            parsed
                .notes
                .iter()
                .any(|(_, reason)| reason.contains("래퍼")),
            "unwrap note missing: {:?}",
            parsed.notes
        );
    }

    #[test]
    fn aliases_never_override_standard_fields() {
        let dump = r#"{"name":"나","first_mes":"표준 인사","greeting":"구형 인사","personality":"표준 성격","title":"별명"}"#;
        let parsed = parse_card_json(dump).expect("hybrid dump");
        assert_eq!(parsed.canonical.first_mes, "표준 인사");
        assert_eq!(parsed.canonical.personality, "표준 성격");
    }

    #[test]
    fn v3_typed_background_data_uri_extracts() {
        let payload = base64::engine::general_purpose::STANDARD.encode(b"fakebg");
        let card = serde_json::json!({
            "spec": "chara_card_v3",
            "data": {
                "name": "아",
                "assets": [{
                    "type": "background",
                    "uri": format!("data:image/webp;base64,{payload}"),
                }],
            },
        });
        let parsed = parse_card_json(&card.to_string()).expect("v3 card");
        let background = parsed.canonical.background.expect("background");
        assert_eq!(background.bytes, b"fakebg");
        assert_eq!(background.extension, "webp");
    }

    #[test]
    fn root_level_stray_keys_do_not_trigger_aliases_on_spec_cards() {
        let card = r#"{"spec":"chara_card_v3","data":{"name":"아","description":"설명","personality":"brave"},"title":"메타 태그"}"#;
        let parsed = parse_card_json(card).expect("v3 card");
        assert_eq!(parsed.canonical.personality, "brave");
        assert!(
            parsed
                .notes
                .iter()
                .all(|(_, reason)| !reason.contains("c.ai")),
            "false alias notes: {:?}",
            parsed.notes
        );
        // The raw JSON keeps the stray key untouched.
        assert_eq!(
            parsed.raw.get("title").and_then(serde_json::Value::as_str),
            Some("메타 태그")
        );
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
