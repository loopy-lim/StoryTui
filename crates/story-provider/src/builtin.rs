//! Built-in offline story provider (`ST-builtin`).
//!
//! Lets StoryTUI launch and play complete stories without any external LLM
//! server running or configured. Implements the full [`LlmProvider`] contract —
//! health, model listing, single-pass turn generation, streaming narration,
//! 2-stage Director/Writer generation, side commands (`continue`, `group_scene`,
//! `suggest`), and choice refreshes — while satisfying `story-core`'s
//! deterministic narration and choice quality gates.

use std::collections::BTreeMap;

use async_trait::async_trait;
use story_model::uuid::Uuid;
use story_model::{
    CharacterId, ChatMessage, Choice, DirectorPlan, DirectorRequest, Emotion, EmotionArc,
    GeneratedTurnDraft, ProviderCapabilities, ProviderError, ProviderHealth, RiskLevel, StatePatch,
    TokenUsage, TurnRequest, TurnSummary, WriterEvent, WriterFinish, WriterRequest, serde_json,
};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;

use crate::config::{MaxTokensField, ProviderConfig, StructuredOutputMode};

/// Reserved provider id for the built-in offline engine.
pub const BUILTIN_PROVIDER_ID: &str = "builtin-offline";
/// Sentinel base URL indicating no network traffic leaves the process.
pub const BUILTIN_BASE_URL: &str = "builtin://offline";
/// Default model id advertised by [`BuiltinStoryProvider`].
pub const BUILTIN_DEFAULT_MODEL: &str = "builtin-story-v1";
/// All models exposed by [`BuiltinStoryProvider`].
pub const BUILTIN_MODELS: &[&str] = &["builtin-story-v1", "builtin-story-fast"];

/// Construct a default [`ProviderConfig`] targeting [`BuiltinStoryProvider`].
pub fn builtin_provider_config() -> ProviderConfig {
    ProviderConfig {
        kind: "builtin".to_string(),
        name: BUILTIN_PROVIDER_ID.to_string(),
        base_url: BUILTIN_BASE_URL.to_string(),
        chat_path: String::new(),
        health_path: String::new(),
        models_path: String::new(),
        model: BUILTIN_DEFAULT_MODEL.to_string(),
        api_key_env: None,
        api_key_file: None,
        api_key: None,
        temperature: 0.7,
        top_p: 0.92,
        top_k: 40,
        max_tokens: 2048,
        max_tokens_field: MaxTokensField::MaxTokens,
        min_narration_chars: 0,
        request_timeout_ms: 5_000,
        connect_timeout_ms: 100,
        max_retries: 0,
        context_budget_tokens: 24_576,
        structured_output: StructuredOutputMode::On,
        stream_turns: true,
    }
}

/// Returns `true` when `config` selects the built-in offline provider.
pub fn is_builtin_config(config: &ProviderConfig) -> bool {
    config.name == BUILTIN_PROVIDER_ID
        || config.base_url.trim().starts_with("builtin://")
        || config.kind == "builtin"
}

/// Deterministic local story generator with zero external dependencies.
#[derive(Clone, Debug)]
pub struct BuiltinStoryProvider {
    id: String,
    config: ProviderConfig,
}

impl BuiltinStoryProvider {
    /// Construct from a [`ProviderConfig`]. Any missing model falls back to
    /// [`BUILTIN_DEFAULT_MODEL`].
    pub fn new(mut config: ProviderConfig) -> Self {
        if config.name.trim().is_empty() {
            config.name = BUILTIN_PROVIDER_ID.to_string();
        }
        if config.model.trim().is_empty() {
            config.model = BUILTIN_DEFAULT_MODEL.to_string();
        }
        Self {
            id: config.name.clone(),
            config,
        }
    }

    fn effective_model(&self) -> &str {
        if self.config.model.trim().is_empty() {
            BUILTIN_DEFAULT_MODEL
        } else {
            &self.config.model
        }
    }

    fn is_fast_mode(&self) -> bool {
        self.effective_model() == "builtin-story-fast"
    }

    async fn emit_chunks(
        &self,
        text: &str,
        sink: &Sender<WriterEvent>,
        cancel: &CancellationToken,
    ) -> Result<(), ProviderError> {
        let chunk_chars = if self.is_fast_mode() { 96 } else { 28 };
        let mut buf = String::new();
        for ch in text.chars() {
            if cancel.is_cancelled() {
                return Err(ProviderError::Cancelled);
            }
            buf.push(ch);
            if buf.chars().count() >= chunk_chars {
                let _ = sink
                    .send(WriterEvent::Chunk {
                        text: std::mem::take(&mut buf),
                    })
                    .await;
                tokio::task::yield_now().await;
            }
        }
        if !buf.is_empty() {
            if cancel.is_cancelled() {
                return Err(ProviderError::Cancelled);
            }
            let _ = sink.send(WriterEvent::Chunk { text: buf }).await;
        }
        Ok(())
    }
}

#[async_trait]
impl crate::provider::LlmProvider for BuiltinStoryProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn model_name(&self) -> &str {
        self.effective_model()
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_structured_output: true,
            supports_director: true,
            supports_streaming: true,
        }
    }

    async fn health(&self) -> Result<ProviderHealth, ProviderError> {
        Ok(ProviderHealth::Ready)
    }

    async fn list_models(&self) -> Result<Vec<String>, ProviderError> {
        Ok(BUILTIN_MODELS.iter().map(|s| (*s).to_string()).collect())
    }

    async fn generate_turn(
        &self,
        request: TurnRequest,
        cancel: CancellationToken,
    ) -> Result<GeneratedTurnDraft, ProviderError> {
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        let ctx = PromptContextInfo::parse(&request.messages, request.json_schema.as_ref());
        let draft = build_turn_draft(&ctx, request.generation_id, self.is_fast_mode());
        Ok(draft)
    }

    async fn generate_turn_stream(
        &self,
        request: TurnRequest,
        sink: Sender<WriterEvent>,
        cancel: CancellationToken,
    ) -> Result<GeneratedTurnDraft, ProviderError> {
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        let ctx = PromptContextInfo::parse(&request.messages, request.json_schema.as_ref());
        let draft = build_turn_draft(&ctx, request.generation_id, self.is_fast_mode());
        self.emit_chunks(&draft.narration, &sink, &cancel).await?;
        let _ = sink
            .send(WriterEvent::Done {
                usage: draft.usage.clone(),
                finish_reason: Some("stop".to_string()),
            })
            .await;
        Ok(draft)
    }

    async fn direct(
        &self,
        request: DirectorRequest,
        cancel: CancellationToken,
    ) -> Result<DirectorPlan, ProviderError> {
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        let ctx = PromptContextInfo::parse(&request.messages, request.json_schema.as_ref());
        Ok(build_director_plan(&ctx, request.generation_id))
    }

    async fn write_stream(
        &self,
        request: WriterRequest,
        sink: Sender<WriterEvent>,
        cancel: CancellationToken,
    ) -> Result<WriterFinish, ProviderError> {
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        let ctx = PromptContextInfo::parse(&request.messages, None);
        let full_text = build_prose(&ctx, request.generation_id, self.is_fast_mode());
        self.emit_chunks(&full_text, &sink, &cancel).await?;
        let usage = Some(estimate_usage(&ctx.raw_prompt, &full_text));
        let _ = sink
            .send(WriterEvent::Done {
                usage: usage.clone(),
                finish_reason: Some("stop".to_string()),
            })
            .await;
        Ok(WriterFinish {
            generation_id: request.generation_id,
            full_text,
            usage,
            finish_reason: Some("stop".to_string()),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequestMode {
    NormalTurn,
    ChoiceRefresh,
    ContinueSide,
    GroupSceneSide,
    SuggestSide,
    NarrationRepair,
}

#[derive(Clone, Debug)]
struct PromptContextInfo {
    mode: RequestMode,
    turn: u32,
    expected_choices: usize,
    characters: Vec<(String, String)>, // (id, display_name)
    locations: Vec<String>,
    current_location: Option<String>,
    affection_limits: (i16, i16),
    trust_limits: (i16, i16),
    tension_limits: (i16, i16),
    selected_choice_label: Option<String>,
    free_action_text: Option<String>,
    free_action_target: Option<String>,
    last_choice_label: Option<String>,
    group_cast: Vec<String>,
    recent_narrations: Vec<String>,
    director_scene_goal: Option<String>,
    raw_prompt: String,
}

impl PromptContextInfo {
    fn parse(messages: &[ChatMessage], json_schema: Option<&serde_json::Value>) -> Self {
        let mut system_text = String::new();
        let mut user_text = String::new();
        let mut assistant_texts = Vec::new();
        for msg in messages {
            match msg.role.as_str() {
                "system" => {
                    if !system_text.is_empty() {
                        system_text.push('\n');
                    }
                    system_text.push_str(&msg.content);
                }
                "assistant" => assistant_texts.push(msg.content.clone()),
                _ => {
                    if !user_text.is_empty() {
                        user_text.push('\n');
                    }
                    user_text.push_str(&msg.content);
                }
            }
        }
        let combined = format!("{system_text}\n{user_text}");

        let mode = if is_choice_only_schema(json_schema)
            || user_text.contains("# 세 슬롯")
            || system_text.contains("선택지") && system_text.contains("JSON으로 쓴다")
        {
            RequestMode::ChoiceRefresh
        } else if user_text.contains("# Continue target (engine-owned)") {
            RequestMode::ContinueSide
        } else if user_text.contains("# Group scene (engine-owned)") {
            RequestMode::GroupSceneSide
        } else if user_text.contains("# Suggested player action (engine-owned)") {
            RequestMode::SuggestSide
        } else if user_text.contains("# Narration-only quality repair")
            || user_text.contains("# Writer narration quality repair")
        {
            RequestMode::NarrationRepair
        } else {
            RequestMode::NormalTurn
        };

        let expected_choices = parse_expected_choices(&system_text)
            .or_else(|| schema_choice_count(json_schema))
            .unwrap_or(3)
            .clamp(1, 6);

        let allowed_char_ids = parse_id_list(&system_text, "## Characters");
        let locations = parse_id_list(&system_text, "## Locations");
        let char_names = parse_character_names(&user_text);

        let mut characters = Vec::new();
        for id in allowed_char_ids {
            let display = char_names
                .get(&id)
                .cloned()
                .unwrap_or_else(|| fallback_display_name(&id));
            characters.push((id, display));
        }
        if characters.is_empty() {
            for (id, display) in char_names {
                characters.push((id, display));
            }
        }

        let (turn, current_location, last_choice_label, active_chars) =
            parse_current_state(&user_text);
        if characters.is_empty() {
            for id in active_chars {
                let display = fallback_display_name(&id);
                characters.push((id, display));
            }
        }

        let affection_limits = parse_delta_limit(&system_text, "affection:").unwrap_or((-3, 3));
        let trust_limits = parse_delta_limit(&system_text, "trust:").unwrap_or((-3, 3));
        let tension_limits = parse_delta_limit(&system_text, "tension:").unwrap_or((-3, 3));

        let selected_choice_label =
            extract_section_field(&user_text, "# Player selected choice", "label:");
        let free_action_text =
            extract_section_field(&user_text, "# Player free action", "original_text:")
                .or_else(|| extract_section_field(&user_text, "# Player free action", "content:"));
        let free_action_target =
            extract_section_field(&user_text, "# Player free action", "target:");

        let group_cast = extract_line_after(&user_text, "대화 가능한 인물:")
            .map(|line| {
                line.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty() && *s != "없음")
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let mut recent_narrations = parse_recent_narrations(&user_text);
        recent_narrations.extend(assistant_texts);

        let director_scene_goal = parse_director_scene_goal(&user_text);

        Self {
            mode,
            turn,
            expected_choices,
            characters,
            locations,
            current_location,
            affection_limits,
            trust_limits,
            tension_limits,
            selected_choice_label,
            free_action_text,
            free_action_target,
            last_choice_label,
            group_cast,
            recent_narrations,
            director_scene_goal,
            raw_prompt: combined,
        }
    }

    fn primary_character(&self) -> Option<(&str, &str)> {
        self.characters
            .first()
            .map(|(id, name)| (id.as_str(), name.as_str()))
    }
}

fn is_choice_only_schema(json_schema: Option<&serde_json::Value>) -> bool {
    json_schema
        .and_then(|schema| schema.get("properties"))
        .and_then(serde_json::Value::as_object)
        .is_some_and(|props| props.len() == 1 && props.contains_key("choices"))
}

fn schema_choice_count(json_schema: Option<&serde_json::Value>) -> Option<usize> {
    json_schema?
        .get("properties")?
        .get("choices")?
        .get("minItems")?
        .as_u64()
        .map(|n| n as usize)
}

fn parse_expected_choices(system: &str) -> Option<usize> {
    for line in system.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("offer exactly ")
            && let Some(num) = rest.split_whitespace().next()
            && let Ok(parsed) = num.parse::<usize>()
        {
            return Some(parsed);
        }
    }
    None
}

fn parse_id_list(system: &str, heading: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Some(start) = system.find(heading) else {
        return out;
    };
    for line in system[start + heading.len()..].lines().skip(1) {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            break;
        }
        if let Some(id) = trimmed.strip_prefix("- ") {
            let id = id.trim();
            if !id.is_empty() {
                out.push(id.to_string());
            }
        }
    }
    out
}

fn parse_character_names(user: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let Some(start) = user.find("# Character canon and voice") else {
        return map;
    };
    for line in user[start..].lines().skip(1) {
        let trimmed = line.trim();
        if trimmed.starts_with("# ") {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("## ")
            && let Some(open) = rest.rfind('(')
            && rest.ends_with(')')
        {
            let name = rest[..open].trim();
            let id = rest[open + 1..rest.len() - 1].trim();
            if !id.is_empty() && !name.is_empty() {
                map.insert(id.to_string(), name.to_string());
            }
        }
    }
    map
}

fn parse_current_state(user: &str) -> (u32, Option<String>, Option<String>, Vec<String>) {
    let Some(start) = user.find("# Current state") else {
        return (0, None, None, Vec::new());
    };
    for line in user[start..].lines().skip(1) {
        let trimmed = line.trim();
        if trimmed.starts_with('{')
            && let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed)
        {
            let turn = val.get("turn").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let location = val
                .get("location")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let last_choice = val
                .get("last_choice")
                .and_then(|v| v.get("label"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let chars = val
                .get("characters")
                .and_then(|v| v.as_object())
                .map(|obj| obj.keys().cloned().collect())
                .unwrap_or_default();
            return (turn, location, last_choice, chars);
        }
        if trimmed.starts_with("# ") {
            break;
        }
    }
    (0, None, None, Vec::new())
}

fn parse_delta_limit(system: &str, prefix: &str) -> Option<(i16, i16)> {
    for line in system.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(prefix)
            && let Some((lo, hi)) = rest.trim().split_once("..=")
            && let (Ok(min), Ok(max)) = (lo.trim().parse::<i16>(), hi.trim().parse::<i16>())
        {
            return Some((min, max));
        }
    }
    None
}

fn extract_section_field(text: &str, section: &str, field: &str) -> Option<String> {
    let start = text.find(section)?;
    for line in text[start + section.len()..].lines().skip(1) {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            break;
        }
        if let Some(val) = trimmed.strip_prefix(field) {
            let val = val.trim();
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

fn extract_line_after(text: &str, prefix: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return Some(rest.trim().to_string());
        }
    }
    None
}

fn parse_recent_narrations(user: &str) -> Vec<String> {
    let mut narrations = Vec::new();
    let mut current: Option<String> = None;
    let in_recent_or_all = user;
    for line in in_recent_or_all.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("narration:") {
            if let Some(prev) = current.take()
                && !prev.trim().is_empty()
            {
                narrations.push(prev);
            }
            current = Some(rest.trim().to_string());
        } else if let Some(buf) = current.as_mut() {
            if trimmed.starts_with("action:")
                || trimmed.starts_with("choices:")
                || trimmed.starts_with("state_patch:")
                || trimmed.starts_with("## ")
                || trimmed.starts_with("# ")
            {
                if !buf.trim().is_empty() {
                    narrations.push(buf.trim().to_string());
                }
                current = None;
            } else if !trimmed.is_empty() {
                if !buf.is_empty() {
                    buf.push('\n');
                }
                buf.push_str(trimmed);
            }
        }
    }
    if let Some(last) = current
        && !last.trim().is_empty()
    {
        narrations.push(last);
    }
    narrations
}

fn parse_director_scene_goal(user: &str) -> Option<String> {
    let marker = "# Validated Director plan";
    let start = user.find(marker)?;
    for line in user[start..].lines().skip(1) {
        let trimmed = line.trim();
        if trimmed.starts_with('{')
            && let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed)
            && let Some(goal) = val.get("scene_goal").and_then(|v| v.as_str())
        {
            return Some(goal.to_string());
        }
    }
    None
}

fn fallback_display_name(id: &str) -> String {
    match id {
        "yuna" => "유나".to_string(),
        "sera" => "세라".to_string(),
        "mira" => "미라".to_string(),
        "c-namu" | "namu" => "나무".to_string(),
        _ => id.to_string(),
    }
}

fn build_turn_draft(
    ctx: &PromptContextInfo,
    generation_id: Uuid,
    fast: bool,
) -> GeneratedTurnDraft {
    let choices = if matches!(
        ctx.mode,
        RequestMode::ContinueSide | RequestMode::GroupSceneSide | RequestMode::SuggestSide
    ) {
        Vec::new()
    } else {
        select_choices(ctx, generation_id)
    };
    let narration = build_prose(ctx, generation_id, fast);
    let proposed_patch = if matches!(
        ctx.mode,
        RequestMode::ChoiceRefresh
            | RequestMode::ContinueSide
            | RequestMode::GroupSceneSide
            | RequestMode::SuggestSide
    ) {
        StatePatch::default()
    } else {
        build_state_patch(ctx)
    };
    let summary = build_summary(ctx);
    let usage = Some(estimate_usage(&ctx.raw_prompt, &narration));

    GeneratedTurnDraft {
        generation_id,
        narration,
        choices,
        proposed_patch,
        summary,
        usage,
    }
}

fn build_director_plan(ctx: &PromptContextInfo, generation_id: Uuid) -> DirectorPlan {
    let (_, primary_name) = ctx.primary_character().unwrap_or(("partner", "상대"));
    let choices = select_choices(ctx, generation_id);
    let proposed_patch = build_state_patch(ctx);

    let mut emotion_arcs = BTreeMap::new();
    if let Some((char_id, _)) = ctx.primary_character() {
        let end = match ctx.turn % 4 {
            0 => Emotion::Anxious,
            1 => Emotion::Cold,
            2 => Emotion::Happy,
            _ => Emotion::Affectionate,
        };
        emotion_arcs.insert(
            CharacterId(char_id.to_string()),
            EmotionArc {
                character: CharacterId(char_id.to_string()),
                start: Emotion::Neutral,
                end,
                reason: Some(format!(
                    "턴 {}에서 플레이어의 선택에 반응한다",
                    ctx.turn + 1
                )),
            },
        );
    }

    let scene_goal = match (&ctx.free_action_text, &ctx.selected_choice_label) {
        (Some(free), _) => format!("'{free}' 행동에 대한 {primary_name}의 직접 반응과 다음 단서"),
        (None, Some(label)) => {
            format!("'{label}' 선택 이후 {primary_name}와의 긴장 조율")
        }
        (None, None) => format!("{primary_name}와의 첫 대면을 다음 단서로 연결"),
    };

    DirectorPlan {
        scene_goal,
        required_beats: vec![
            format!("{primary_name}의 관찰 가능한 반응과 대사 한 줄"),
            "현재 장면의 구체적 소품 또는 장소 변화 하나".to_string(),
        ],
        emotion_arcs,
        character_outfits: BTreeMap::new(),
        ambient_situation: Some("오프라인 내장 엔진이 장면 흐름을 조율 중".to_string()),
        reveal_fact_ids: Vec::new(),
        must_not_reveal_fact_ids: Vec::new(),
        proposed_patch,
        choices,
        conflict_checks: Vec::new(),
        end_condition: "플레이어가 다음 행동이나 선택지를 고를 시점에 도달한다".to_string(),
    }
}

fn build_state_patch(ctx: &PromptContextInfo) -> StatePatch {
    let mut patch = StatePatch::default();
    let Some((char_id, _)) = ctx.primary_character() else {
        return patch;
    };
    let cid = CharacterId(char_id.to_string());
    let clamp = |v: i16, (lo, hi): (i16, i16)| v.clamp(lo, hi);

    let action_hint = ctx
        .selected_choice_label
        .as_deref()
        .or(ctx.free_action_text.as_deref())
        .unwrap_or("");
    if action_hint.contains("사과")
        || action_hint.contains("양보")
        || action_hint.contains("부탁")
        || action_hint.contains("건넨")
    {
        let a = clamp(1, ctx.affection_limits);
        let t = clamp(1, ctx.trust_limits);
        let s = clamp(-1, ctx.tension_limits);
        if a != 0 {
            patch.affection_delta.insert(cid.clone(), a);
        }
        if t != 0 {
            patch.trust_delta.insert(cid.clone(), t);
        }
        if s != 0 {
            patch.tension_delta.insert(cid, s);
        }
    } else if action_hint.contains("경계")
        || action_hint.contains("거절")
        || action_hint.contains("묻")
    {
        let t = clamp(1, ctx.trust_limits);
        let s = clamp(1, ctx.tension_limits);
        if t != 0 {
            patch.trust_delta.insert(cid.clone(), t);
        }
        if s != 0 {
            patch.tension_delta.insert(cid, s);
        }
    } else {
        let step = if ctx.turn.is_multiple_of(2) { 1 } else { 0 };
        let a = clamp(step, ctx.affection_limits);
        let t = clamp(1, ctx.trust_limits);
        if a != 0 {
            patch.affection_delta.insert(cid.clone(), a);
        }
        if t != 0 {
            patch.trust_delta.insert(cid, t);
        }
    }

    if !ctx.locations.is_empty()
        && let Some(curr) = &ctx.current_location
        && action_hint.contains("이동")
        && let Some(next_loc) = ctx.locations.iter().find(|loc| *loc != curr)
    {
        patch.next_location = Some(story_model::LocationId(next_loc.clone()));
    }
    patch
}

fn build_summary(ctx: &PromptContextInfo) -> TurnSummary {
    let (_, primary_name) = ctx.primary_character().unwrap_or(("partner", "상대"));
    let turn_num = ctx.turn + 1;
    let player_move = ctx
        .free_action_text
        .clone()
        .or_else(|| ctx.selected_choice_label.clone())
        .unwrap_or_else(|| "대면 장면을 시작했다".to_string());
    TurnSummary {
        facts: vec![format!(
            "턴 {turn_num}: {primary_name}가 장면 단서를 짚고 대화를 이어 갔다."
        )],
        unresolved: vec![format!("{primary_name}와의 핵심 약속 확인")],
        relationship_changes: vec![format!(
            "{primary_name}와의 대화 긴장이 턴 {turn_num}에서 조금씩 움직였다."
        )],
        knowledge_changes: Vec::new(),
        interpretation: Some(format!("턴 {turn_num} 진행: {player_move}")),
        address_form_changes: Vec::new(),
        important_player_actions: vec![player_move],
        source_history_ids: Vec::new(),
    }
}

fn build_prose(ctx: &PromptContextInfo, generation_id: Uuid, fast: bool) -> String {
    let (_, primary_name) = ctx.primary_character().unwrap_or(("partner", "상대"));
    match ctx.mode {
        RequestMode::ChoiceRefresh => return "선택지를 새로 고쳤다.".to_string(),
        RequestMode::SuggestSide => return build_suggest_action(ctx),
        RequestMode::GroupSceneSide => return build_group_scene(ctx, generation_id),
        RequestMode::ContinueSide => return build_continue_prose(ctx, generation_id),
        RequestMode::NormalTurn | RequestMode::NarrationRepair => {}
    }

    let scenes: [(&str, &str, &str); 12] = [
        (
            "젖은 유리창 너머 가로등 불빛이 번지는 사이, 도자기 컵에서 옅은 김이 피어오른다.",
            "\"조금 전 언급한 날짜가 옛 수첩 숫자와 맞물리지 않아서 다시 대조해 보려 했어.\"",
            "말끝을 맺은 뒤 구석 스탠드 아래 놓인 엽서 모서리를 손가락으로 톡 두드린다.",
        ),
        (
            "서늘한 밤공기가 문틈을 스치자 바닥의 가죽 끈이 흔들리며 작은 금속 버클 소리를 낸다.",
            "\"결론을 서두르기보다 빠진 순서부터 한 항목씩 차근차근 짚어 보자.\"",
            "팔짱을 풀고 나무 의자 등받이에 기대며 당신의 다음 반응을 기다린다.",
        ),
        (
            "선반 위 탁상용 초침이 규칙적으로 울리는 가운데 빗방울 하나가 창틀을 타고 굴러떨어진다.",
            "\"솔직히 그때 남긴 쪽지는 절반만 적은 셈이야. 나머지는 얼굴을 보고 전해야 했거든.\"",
            "머뭇거리던 태도를 거두고 탁자 모퉁이의 흰 봉투를 한 뼘 앞으로 밀어 놓는다.",
        ),
        (
            "복도 천장 등이 낮아진 자리에서 발걸음 울림이 잦아들자 두 사람 사이의 정적이 가라앉는다.",
            "\"네가 그 대목을 먼저 짚을 줄은 몰랐네. 다행히 아직 지우지 않은 사본이 남아 있어.\"",
            "주머니에서 낡은 수첩을 꺼내 책갈피가 끼워진 페이지를 조심스럽게 펼쳐 보인다.",
        ),
        (
            "마른 잎사귀와 갈색 쟁반 위로 오후 늦은 볕이 비스듬히 길게 내려앉는다.",
            "\"변명처럼 비칠까 봐 망설였는데, 사실 나도 그날 역 광장 시계탑 아래 한참 서 있었어.\"",
            "목소리를 낮추고 찻잔 받침을 쥔 손끝에 잠깐 힘을 주었다가 천천히 풀어낸다.",
        ),
        (
            "반쯤 열린 블라인드 틈으로 지나가는 차량 전조등 빛이 벽지를 가로질렀다가 순식간에 사라진다.",
            "\"서로 어긋난 시각이 언제였는지만 확인하면, 오해도 의외로 간단하게 풀릴 거야.\"",
            "벽시계 바늘을 한 차례 올려다본 뒤 지퍼 파우치에서 작은 황동 열쇠를 꺼낸다.",
        ),
        (
            "에스프레소 추출기 진동이 멎자 카운터 안쪽 유리병 두 개가 나란히 그림자를 드리운다.",
            "\"네가 챙겨 온 단서 덕분에 이제야 앞뒤 고리가 하나씩 맞물리기 시작했어.\"",
            "짧게 고개를 끄덕이며 이면지 여백에 둥근 표시 두 개를 나란히 그려 넣는다.",
        ),
        (
            "찬 바람이 스치는 난간 건너편에서 먼 건물 옥상의 항공 장애등이 붉은빛을 깜빡인다.",
            "\"지금 뒤로 물러서면 또 몇 년을 에둘러야 해. 이번엔 나도 끝까지 마주해 볼게.\"",
            "귀 뒤로 흐트러진 머리카락을 넘기며 반 보 가까이 간격을 좁힌다.",
        ),
        (
            "철제 다리가 바닥 타일을 긁는 짧은 소리 뒤로 먼 지하철 환승 안내 방송이 희미하게 번진다.",
            "\"어젯밤 받은 발신 번호를 맞춰 보니 낯선 주소가 아니라 우리 예전 모임 건물 쪽이었어.\"",
            "손목 밴드 화면을 켜서 저장된 통화 시각 숫자를 당신 쪽으로 기울여 보여 준다.",
        ),
        (
            "습기를 머금은 종이 표면의 잉크 번짐이 주황빛 전구 아래에서 유독 선명하게 드러난다.",
            "\"잠깐, 이 우체국 소인 날짜 좀 봐. 우리가 헤어진 다음 주에 다시 접수된 내역이야.\"",
            "놀란 기색을 감추지 못한 채 뒷면 우편 번호 다섯 자리를 손끝으로 짚는다.",
        ),
        (
            "인쇄기 옆 묵직한 서류 더미 사이에서 떨어진 얇은 소책자 한 권이 문턱 앞에서 멈춘다.",
            "\"감추려고 입을 다문 게 아니야. 네 안전이 걸린 일이라 때를 기다렸을 뿐이야.\"",
            "입술을 굳게 다물었다가 긴 한숨을 내쉬며 두꺼운 파일 겉장을 도로 덮는다.",
        ),
        (
            "비 개인 테라스 난간으로 선선한 바람이 불어와 놓여 있던 얼음 컵 뚜껑을 가볍게 흔든다.",
            "\"좋아, 그럼 차례를 정하자. 네가 오늘 꼭 확인하고 싶은 첫 질문부터 꺼내 줘.\"",
            "어깨 힘을 빼고 똑바로 고쳐 앉으며 검은 볼펜을 공책 옆에 가지런히 내려놓는다.",
        ),
    ];

    let base_idx = (ctx.turn as usize).wrapping_add(generation_id.as_bytes()[0] as usize % 3);
    let compose = |idx: usize, lead_variant: usize| -> String {
        let action_lead = action_lead_sentence(ctx, primary_name, lead_variant);
        let (env, dialogue, reaction) = scenes[idx];
        if fast {
            format!("{action_lead} {env}\n{primary_name}: {dialogue}")
        } else {
            let goal_line = ctx
                .director_scene_goal
                .as_deref()
                .map(|goal| format!(" 장면의 초점은 '{goal}'에 닿아 있다."))
                .unwrap_or_default();
            format!(
                "{action_lead} {env}\n\n{primary_name}: {dialogue}\n\n{primary_name}는 {reaction}{goal_line}"
            )
        }
    };

    let mut best_text = compose(base_idx % scenes.len(), ctx.turn as usize);
    let mut best_score = usize::MAX;
    for offset in 0..scenes.len() {
        let candidate_idx = (base_idx + offset) % scenes.len();
        let candidate = compose(candidate_idx, (ctx.turn as usize).wrapping_add(offset));
        let score = quality_conflict_score(&candidate, &ctx.recent_narrations);
        if score == 0 {
            return candidate;
        }
        if score < best_score {
            best_score = score;
            best_text = candidate;
        }
    }
    best_text
}

fn action_lead_sentence(ctx: &PromptContextInfo, primary_name: &str, variant: usize) -> String {
    if let Some(free) = &ctx.free_action_text {
        let trimmed = free.trim();
        return if let Some(target) = &ctx.free_action_target {
            format!("당신이 {target}에게 '{trimmed}' 뜻으로 다가가자 주변 공기가 즉각 달라진다.")
        } else {
            format!("당신이 '{trimmed}' 움직임을 보이자 그쪽을 향한 분위기 결이 달라진다.")
        };
    }
    if let Some(label) = &ctx.selected_choice_label {
        return match variant % 6 {
            0 => format!("당신이 '{label}' 방향을 고르자 {primary_name}가 잠시 말을 멈춘다."),
            1 => format!("'{label}' 선택이 이어지자 {primary_name}는 고개를 들어 상대를 살핀다."),
            2 => format!("당신이 '{label}' 행동을 택하자 실내 정적의 무게가 미묘하게 바뀐다."),
            3 => format!("'{label}' 결정이 떨어지자 {primary_name}의 표정에 새로운 기색이 스친다."),
            4 => format!("당신이 '{label}' 쪽으로 마음을 정하자 대화의 온도가 한 겹 변한다."),
            _ => format!("'{label}' 태도가 드러나자 {primary_name}가 조용히 다음 문장을 고른다."),
        };
    }
    format!("이야기 막이 열리자 {primary_name}가 한 걸음 내디디며 정면을 바라본다.")
}

fn build_continue_prose(ctx: &PromptContextInfo, generation_id: Uuid) -> String {
    let (_, primary_name) = ctx.primary_character().unwrap_or(("partner", "상대"));
    let idx = seed_index(ctx, generation_id) % 4;
    match idx {
        0 => format!(
            "잠시 정적이 흐른 뒤, {primary_name}는 말없이 손목시계를 한 번 확인하고 테이블 위의 메모를 한 장 넘긴다.\n\n{primary_name}: \"아직 시간이 조금 남았으니, 네가 궁금했던 대목부터 천천히 짚어 보자.\""
        ),
        1 => format!(
            "창밖 빗소리가 한 톤 낮아지는 사이 {primary_name}는 가방 옆 주머니에서 작은 영수증 조각을 꺼내 조용히 펼쳐 놓는다.\n\n{primary_name}: \"이 날짜가 맞다면 우리가 서로 다른 곳을 찾고 있었던 셈이네.\""
        ),
        2 => format!(
            "{primary_name}는 컵 가장자리를 손끝으로 따라 그리다가 시선을 들어 당신의 반응이 어느 쪽으로 기우는지 살핀다.\n\n{primary_name}: \"서두르지 않아도 괜찮아. 다만 오늘 안에는 매듭을 짓고 싶어.\""
        ),
        _ => format!(
            "문 바깥쪽 발자국 소리가 멀어지고 나자 {primary_name}는 몸을 조금 앞으로 기울이며 목소리 톤을 낮춘다.\n\n{primary_name}: \"지금 이 자리에서 바로 확인해 두지 않으면 다음 기회는 없을지도 몰라.\""
        ),
    }
}

fn build_group_scene(ctx: &PromptContextInfo, generation_id: Uuid) -> String {
    let mut speakers = ctx.group_cast.clone();
    if speakers.is_empty() {
        speakers = ctx
            .characters
            .iter()
            .map(|(_, name)| name.clone())
            .collect();
    }
    if speakers.is_empty() {
        speakers.push("동행".to_string());
    }
    let first = &speakers[0];
    let second = speakers.get(1).unwrap_or(first);
    let idx = seed_index(ctx, generation_id) % 3;
    if first != second {
        match idx {
            0 => format!(
                "{first}: \"방금 전 시간 기록, 원래 일정표하고 앞뒤가 맞지 않아요.\"\n{second}: \"맞아, 누군가 출발 전에 마지막 메모만 바꿔 놓은 것 같아.\""
            ),
            1 => format!(
                "{first}: \"일단 문 앞쪽 동선부터 확인하고 나서 다음 자리를 정하죠.\"\n{second}: \"그래, 서류 철에 찍힌 도장 날짜도 같이 대조해 보자.\""
            ),
            _ => format!(
                "{first}: \"지금 분위기라면 오래 끌수록 서로 오해만 깊어질 거예요.\"\n{second}: \"그럼 우리가 아는 사실만 먼저 테이블 위에 꺼내 놓자.\""
            ),
        }
    } else {
        format!(
            "{first}: \"일정표 시간을 다시 맞출 필요가 있겠네.\"\n{first}: \"우선 눈앞에 남아 있는 단서부터 차례대로 확인해 보자.\""
        )
    }
}

fn build_suggest_action(ctx: &PromptContextInfo) -> String {
    let (_, primary_name) = ctx.primary_character().unwrap_or(("partner", "상대"));
    match ctx.turn % 4 {
        0 => format!("{primary_name} 앞에 놓인 봉투의 날짜를 가리키며 경위를 묻는다."),
        1 => "주머니에서 메모지를 꺼내 최근 일정과 대조해 본다.".to_string(),
        2 => format!("{primary_name}에게 자리를 옮겨 차분히 이야기하자고 제안한다."),
        _ => "휴대전화에 남은 마지막 메시지 시각을 직접 확인한다.".to_string(),
    }
}

/// Deterministic choice trio pools engineered to satisfy every check in
/// `story_core::choice_quality::assess_choices`:
/// - slot role `"relationship"` -> `RiskLevel::Medium` or `RiskLevel::High`,
///   concrete hook + player action verb, non-generic, Korean `intent`/`preview`.
/// - slot role `"evidence"` -> `RiskLevel::Safe` or `RiskLevel::Low`,
///   no relationship keywords, concrete evidence action verb.
/// - slot role `"situation"` -> `RiskLevel::Medium` or `RiskLevel::High`,
///   NO relationship or evidence keywords, concrete situation verb.
fn select_choices(ctx: &PromptContextInfo, generation_id: Uuid) -> Vec<Choice> {
    struct TrioCandidate {
        rel: (&'static str, &'static str, &'static str, RiskLevel),
        evi: (&'static str, &'static str, &'static str, RiskLevel),
        sit: (&'static str, &'static str, &'static str, RiskLevel),
    }

    const CANDIDATES: &[TrioCandidate] = &[
        TrioCandidate {
            rel: (
                "먼저 사과한다",
                "과거의 침묵을 인정하고 신뢰 회복을 시도한다",
                "상대의 방어심이 누그러질 수 있다.",
                RiskLevel::Medium,
            ),
            evi: (
                "편지 날짜를 확인한다",
                "봉투에 찍힌 소인과 발송 시점을 대조한다",
                "시간순의 빈틈 하나가 드러난다.",
                RiskLevel::Low,
            ),
            sit: (
                "창가 자리를 부탁한다",
                "조용한 구석 테이블로 이동해 대화를 이어간다",
                "주변 소음과 시선에서 벗어난다.",
                RiskLevel::Medium,
            ),
        },
        TrioCandidate {
            rel: (
                "약속을 지키겠다고 말한다",
                "이번 만남만큼은 끝까지 책임지겠다고 밝힌다",
                "관계의 무게가 한층 무거워진다.",
                RiskLevel::High,
            ),
            evi: (
                "메모지 뒷면을 읽어본다",
                "접힌 종이 안쪽에 남은 추가 문구를 살핀다",
                "숨겨진 단서 문장 하나를 읽는다.",
                RiskLevel::Safe,
            ),
            sit: (
                "산책로로 이동한다",
                "실내를 벗어나 바깥 공기를 쐬며 걸음을 옮긴다",
                "장소 전환으로 대화의 결이 달라진다.",
                RiskLevel::Medium,
            ),
        },
        TrioCandidate {
            rel: (
                "우산 약속을 묻는다",
                "빗속에서 남겼던 마지막 언약의 의미를 묻는다",
                "예민한 과거 기억을 건드릴 수 있다.",
                RiskLevel::High,
            ),
            evi: (
                "사진 모서리를 꺼낸다",
                "지갑에 접어 둔 옛 사진을 꺼내 보여준다",
                "당시의 현장 단서를 공유한다.",
                RiskLevel::Low,
            ),
            sit: (
                "따뜻한 차를 주문한다",
                "카운터에서 음료를 청해 호흡을 가다듬는다",
                "잠깐의 틈이 생기며 분위기가 환기된다.",
                RiskLevel::Medium,
            ),
        },
        TrioCandidate {
            rel: (
                "무리한 부탁은 거절한다",
                "지킬 수 없는 제안에는 분명히 선을 긋는다",
                "당장의 긴장감이 높아질 수 있다.",
                RiskLevel::High,
            ),
            evi: (
                "영수증 기록을 대조한다",
                "결제 시각과 이동 동선을 맞춰 본다",
                "사실 관계의 모순 여부를 가린다.",
                RiskLevel::Low,
            ),
            sit: (
                "역 입구로 나간다",
                "문을 열고 바깥 통로 쪽으로 함께 발걸음을 뗀다",
                "현재 공간을 떠나 다음 구간으로 간다.",
                RiskLevel::High,
            ),
        },
        TrioCandidate {
            rel: (
                "이전 오해를 해명한다",
                "전달되지 못한 연락의 전후 사정을 밝힌다",
                "상대가 사실 여부를 되물을 수 있다.",
                RiskLevel::Medium,
            ),
            evi: (
                "메시지 보관함을 확인한다",
                "휴대전화에 남은 옛 대화 목록을 찾아본다",
                "정확한 전송 이력을 확보한다.",
                RiskLevel::Safe,
            ),
            sit: (
                "자리에서 일어선다",
                "대화를 다음 단계로 옮기기 위해 먼저 몸을 일으킨다",
                "마감 시점이 다가오며 결정을 재촉한다.",
                RiskLevel::Medium,
            ),
        },
        TrioCandidate {
            rel: (
                "솔직히 경계를 밝힌다",
                "감정보다 사실 정리가 우선임을 또렷이 말한다",
                "친밀감 대신 냉정한 거리가 놓인다.",
                RiskLevel::High,
            ),
            evi: (
                "서류 철을 열어본다",
                "테이블 옆 파일 안의 최근 목록을 읽는다",
                "누락되었던 항목 하나를 발견한다.",
                RiskLevel::Low,
            ),
            sit: (
                "내일 다시 만나자고 제안한다",
                "오늘은 여기서 정리하고 다음 일정을 잡는다",
                "장면이 흩어지며 여운과 과제가 남는다.",
                RiskLevel::High,
            ),
        },
    ];

    let base_idx = ((ctx.turn as usize)
        .wrapping_add(generation_id.as_bytes()[0] as usize)
        .wrapping_add(usize::from(ctx.mode == RequestMode::ChoiceRefresh)))
        % CANDIDATES.len();

    let mut chosen = &CANDIDATES[base_idx];
    for offset in 0..CANDIDATES.len() {
        let candidate = &CANDIDATES[(base_idx + offset) % CANDIDATES.len()];
        let labels = [candidate.rel.0, candidate.evi.0, candidate.sit.0];
        let collides_last = ctx.last_choice_label.as_deref().is_some_and(|last| {
            labels
                .iter()
                .any(|label| label == &last || last.contains(label) || label.contains(last))
        });
        let collides_selected = ctx.selected_choice_label.as_deref().is_some_and(|sel| {
            labels
                .iter()
                .any(|label| label == &sel || sel.contains(label) || label.contains(sel))
        });
        let collides_prompt = ctx.mode == RequestMode::ChoiceRefresh
            && labels.iter().any(|label| ctx.raw_prompt.contains(label));
        if !collides_last && !collides_selected && !collides_prompt {
            chosen = candidate;
            break;
        }
    }

    let mut ordered = vec![
        Choice {
            id: "choice-rel".to_string(),
            label: chosen.rel.0.to_string(),
            intent: chosen.rel.1.to_string(),
            tone: None,
            target: ctx
                .primary_character()
                .map(|(id, _)| CharacterId(id.to_string())),
            risk: chosen.rel.3,
            hidden_requirements: Vec::new(),
            preview: Some(chosen.rel.2.to_string()),
        },
        Choice {
            id: "choice-evi".to_string(),
            label: chosen.evi.0.to_string(),
            intent: chosen.evi.1.to_string(),
            tone: None,
            target: None,
            risk: chosen.evi.3,
            hidden_requirements: Vec::new(),
            preview: Some(chosen.evi.2.to_string()),
        },
        Choice {
            id: "choice-sit".to_string(),
            label: chosen.sit.0.to_string(),
            intent: chosen.sit.1.to_string(),
            tone: None,
            target: None,
            risk: chosen.sit.3,
            hidden_requirements: Vec::new(),
            preview: Some(chosen.sit.2.to_string()),
        },
    ];

    let turn_rot = (ctx.turn as usize) % ordered.len();
    ordered.rotate_left(turn_rot);

    while ordered.len() > ctx.expected_choices {
        ordered.pop();
    }
    if ordered.len() < ctx.expected_choices {
        let extras = [
            (
                "choice-extra-1",
                "수첩에 메모를 적는다",
                "지금 들은 핵심 단서를 잊지 않게 기록한다",
                RiskLevel::Safe,
                "후속 추리에 쓸 기록이 남는다.",
            ),
            (
                "choice-extra-2",
                "동행에게 도움을 요청한다",
                "바깥에서 대기 중인 인물에게 지원을 청한다",
                RiskLevel::Medium,
                "제삼자가 장면에 개입할 수 있다.",
            ),
            (
                "choice-extra-3",
                "열쇠를 건넨다",
                "보관함 열쇠를 상대 손에 직접 넘긴다",
                RiskLevel::High,
                "주도권의 균형이 달라진다.",
            ),
        ];
        for (id, label, intent, risk, preview) in extras {
            if ordered.len() >= ctx.expected_choices {
                break;
            }
            ordered.push(Choice {
                id: id.to_string(),
                label: label.to_string(),
                intent: intent.to_string(),
                tone: None,
                target: None,
                risk,
                hidden_requirements: Vec::new(),
                preview: Some(preview.to_string()),
            });
        }
    }

    ordered
}

fn seed_index(ctx: &PromptContextInfo, generation_id: Uuid) -> usize {
    let bytes = generation_id.as_bytes();
    (ctx.turn as usize)
        .wrapping_mul(7)
        .wrapping_add(bytes[0] as usize)
        .wrapping_add((bytes[15] as usize) << 1)
}

fn quality_conflict_score(candidate: &str, recent: &[String]) -> usize {
    use std::collections::{BTreeMap, BTreeSet};

    fn normalize_tok(word: &str) -> Option<String> {
        const PARTICLES: &[&str] = &[
            "에서는",
            "으로는",
            "에게는",
            "까지는",
            "부터는",
            "들은",
            "에게",
            "에서",
            "으로",
            "처럼",
            "보다",
            "마저",
            "조차",
            "이가",
            "은",
            "는",
            "이",
            "가",
            "을",
            "를",
            "에",
            "의",
            "도",
            "와",
            "과",
            "로",
            "만",
        ];
        let mut s = word
            .trim_matches(|c: char| !c.is_alphanumeric())
            .to_lowercase();
        for p in PARTICLES {
            if let Some(stem) = s.strip_suffix(p)
                && stem.chars().count() >= 2
            {
                s = stem.to_string();
                break;
            }
        }
        let s = s.trim_matches(|c: char| !c.is_alphanumeric()).to_string();
        if s.chars().count() < 2 { None } else { Some(s) }
    }

    fn tokens(text: &str) -> BTreeSet<String> {
        text.split_whitespace().filter_map(normalize_tok).collect()
    }

    fn fingerprint(text: &str) -> String {
        text.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    }

    let window_start = recent.len().saturating_sub(4);
    let window = &recent[window_start..];
    if window.is_empty() {
        return 0;
    }

    let cand_fp = fingerprint(candidate);
    let cand_sentences: BTreeSet<String> = candidate
        .split(['.', '。', '!', '?', '\n'])
        .map(fingerprint)
        .filter(|s| s.chars().count() >= 32)
        .collect();

    let mut sim_penalty = 0usize;
    let mut freq: BTreeMap<String, usize> = BTreeMap::new();
    for prev in window {
        let prev_fp = fingerprint(prev);
        if !cand_fp.is_empty() && cand_fp == prev_fp {
            sim_penalty += 100;
        }
        let prev_sentences: BTreeSet<String> = prev
            .split(['.', '。', '!', '?', '\n'])
            .map(fingerprint)
            .filter(|s| s.chars().count() >= 32)
            .collect();
        if cand_sentences.iter().any(|s| prev_sentences.contains(s)) {
            sim_penalty += 100;
        }
        for tok in tokens(prev) {
            *freq.entry(tok).or_default() += 1;
        }
    }

    let cand_tokens = tokens(candidate);
    let reused = freq
        .into_iter()
        .filter(|(tok, count)| *count >= 2 && cand_tokens.contains(tok))
        .count();
    let motif_penalty = reused.saturating_sub(1);
    sim_penalty + motif_penalty
}

fn estimate_usage(prompt: &str, completion: &str) -> TokenUsage {
    let prompt_tokens = (prompt.chars().count() / 3).max(1) as u32;
    let completion_tokens = (completion.chars().count() / 3).max(1) as u32;
    TokenUsage {
        prompt_tokens,
        completion_tokens,
        total_tokens: prompt_tokens.saturating_add(completion_tokens),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::LlmProvider;

    #[test]
    fn builtin_config_reports_ready_and_loopback() {
        let cfg = builtin_provider_config();
        assert!(is_builtin_config(&cfg));
        assert!(crate::openai_compatible::is_loopback_base_url(
            &cfg.base_url
        ));
        assert_eq!(cfg.model, BUILTIN_DEFAULT_MODEL);
    }

    #[tokio::test]
    async fn builtin_provider_lists_models_and_checks_health() {
        let provider = BuiltinStoryProvider::new(builtin_provider_config());
        assert_eq!(provider.id(), BUILTIN_PROVIDER_ID);
        assert_eq!(provider.model_name(), BUILTIN_DEFAULT_MODEL);
        assert_eq!(provider.health().await.unwrap(), ProviderHealth::Ready);
        let models = provider.list_models().await.unwrap();
        assert!(models.contains(&BUILTIN_DEFAULT_MODEL.to_string()));
        assert!(models.contains(&"builtin-story-fast".to_string()));
    }
}
