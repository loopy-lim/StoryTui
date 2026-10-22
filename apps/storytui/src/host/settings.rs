//! Host-owned provider settings operations: safe UI projection, model
//! discovery, and persistence of player-selected model overrides.

use std::path::Path;
use std::sync::Arc;

use story_model::ProviderHealth;
use story_provider::{LlmProvider, OpenAiCompatibleProvider, ProviderConfig};
use story_tts::{TtsService, TtsStatus};
use story_tui::App;

use storytui::config::{RuntimeConfig, UserSettings, save_user_settings};
use storytui::diagnostic::provider_config_safe_with_models;

#[allow(clippy::too_many_arguments)] // the projection stays explicit at the host boundary
pub fn refresh_view(
    app: &mut App,
    provider_config: &ProviderConfig,
    health: ProviderHealth,
    director_model: Option<String>,
    choice_model: Option<String>,
    writer_model: Option<String>,
    tts: &TtsStatus,
    images_enabled: bool,
    image_backend: Option<String>,
    use_director_writer: bool,
    expressions_enabled: bool,
) {
    app.set_settings(
        provider_config_safe_with_models(
            provider_config,
            director_model,
            choice_model,
            writer_model,
        ),
        health,
        provider_config.temperature,
        provider_config.top_p,
        provider_config.top_k,
        provider_config.max_tokens,
        storytui_tts_display(tts),
        images_enabled,
        image_backend,
        use_director_writer,
        expressions_enabled,
    );
}

/// Map the service status onto the UI's plain display type.
fn storytui_tts_display(tts: &TtsStatus) -> story_tui::app::TtsDisplay {
    story_tui::app::TtsDisplay {
        enabled: tts.enabled,
        backend: tts.backend.as_str().to_string(),
        voice: tts.voice.clone(),
    }
}

pub async fn refresh_model_list(
    app: &mut App,
    provider: &OpenAiCompatibleProvider,
    provider_config: &ProviderConfig,
    preserve_selection: bool,
) {
    match provider.list_models().await {
        Ok(models) => {
            if preserve_selection {
                app.refresh_models(models, &provider_config.model);
            } else {
                app.set_models(models, &provider_config.model);
            }
        }
        Err(error) => {
            tracing::warn!(%error, "provider model list could not be refreshed");
            // Some compatible servers expose chat completions but not
            // `/v1/models`; retain the known configured model in that case.
            if preserve_selection {
                app.refresh_models(vec![provider_config.model.clone()], &provider_config.model);
            } else {
                app.set_models(vec![provider_config.model.clone()], &provider_config.model);
            }
        }
    }
}

/// Select an advertised model without treating a stale persisted value as a
/// server capability. Prefer the current model, then the checked-in profile
/// fallback, then the first stable server-advertised id.
pub fn reconcile_active_model(
    current: &str,
    fallback: &str,
    advertised: &[String],
) -> Option<String> {
    if advertised.iter().any(|model| model == current) {
        return Some(current.to_string());
    }
    if advertised.iter().any(|model| model == fallback) {
        return Some(fallback.to_string());
    }
    advertised.first().cloned()
}

/// Common local OpenAI-compatible endpoints probed when no model is selected
/// yet and the configured endpoint does not answer. Deliberately generic —
/// no product names: anything that answers `/v1/models` with a chat model
/// wins, whatever server happens to be running there.
pub const AUTO_SETUP_CANDIDATE_BASE_URLS: &[&str] = &[
    "http://127.0.0.1:8080",
    "http://127.0.0.1:1234",
    "http://127.0.0.1:11434",
];

/// Model ids containing these markers cannot drive story generation —
/// embedding, reranking, and audio endpoints also advertise themselves via
/// `/v1/models`, so auto-setup must never pick one.
const NON_CHAT_MODEL_MARKERS: &[&str] =
    &["embed", "rerank", "whisper", "tts", "clip", "moderation"];

/// Drop advertised ids that cannot serve chat completions.
pub fn filter_chat_models(advertised: &[String]) -> Vec<String> {
    advertised
        .iter()
        .filter(|model| {
            let lower = model.to_lowercase();
            !NON_CHAT_MODEL_MARKERS
                .iter()
                .any(|marker| lower.contains(marker))
        })
        .cloned()
        .collect()
}

/// Pick the model auto-setup should activate: the current model, then the
/// profile fallback, then the first advertised chat model — the same chain
/// as `reconcile_active_model`, just with non-chat ids filtered out first.
pub fn pick_auto_model(current: &str, fallback: &str, advertised: &[String]) -> Option<String> {
    reconcile_active_model(current, fallback, &filter_chat_models(advertised))
}

/// Candidate endpoints for auto-setup: the configured URL first, then the
/// common local ports, deduplicated in order.
pub fn probe_candidates(configured_base_url: &str) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::with_capacity(AUTO_SETUP_CANDIDATE_BASE_URLS.len() + 1);
    let configured = configured_base_url.trim().trim_end_matches('/');
    if !configured.is_empty() {
        candidates.push(configured.to_string());
    }
    for candidate in AUTO_SETUP_CANDIDATE_BASE_URLS {
        if !candidates.iter().any(|existing| existing == candidate) {
            candidates.push((*candidate).to_string());
        }
    }
    candidates
}

/// Result of the first-run server discovery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiscoveryOutcome {
    /// A server answered and a chat model was auto-selected. The URL is
    /// session-only (never persisted — the user-settings policy keeps
    /// endpoints out); the model is persisted by the caller.
    Connected { base_url: String, model: String },
    /// A server answered but exposes no usable chat model.
    NoChatModel,
    /// No candidate endpoint answered.
    NoServer,
}

/// Probe the configured endpoint and the common local candidates, adopting
/// the first one that advertises a usable chat model. Only called while no
/// model is selected (fresh clone / wiped user settings).
///
/// Each probe uses a short connect timeout and no retries: loopback
/// refusals are instant, and an unresponsive local listener costs at most
/// one `list_models` probe timeout.
pub async fn discover_session_provider(configured: &ProviderConfig) -> DiscoveryOutcome {
    let mut remembered = DiscoveryOutcome::NoServer;
    for base_url in probe_candidates(&configured.base_url) {
        let mut candidate = configured.clone();
        candidate.base_url = base_url.clone();
        candidate.connect_timeout_ms = 1_000;
        candidate.max_retries = 0;
        let provider = match OpenAiCompatibleProvider::new(candidate) {
            Ok(provider) => provider,
            Err(error) => {
                tracing::debug!(%error, "auto-setup candidate provider build failed");
                continue;
            }
        };
        match provider.list_models().await {
            Ok(advertised) => {
                if let Some(model) = pick_auto_model("", "", &advertised) {
                    return DiscoveryOutcome::Connected { base_url, model };
                }
                if remembered == DiscoveryOutcome::NoServer {
                    remembered = DiscoveryOutcome::NoChatModel;
                }
            }
            Err(error) => {
                tracing::debug!(%error, "auto-setup probe failed");
            }
        }
    }
    remembered
}

/// Plain-Korean line for the start menu while no model is selected yet.
/// Attention rows (`true`) render yellow, the rest gray.
pub fn unconfigured_home_status(outcome: &DiscoveryOutcome) -> (String, bool) {
    let line = match outcome {
        DiscoveryOutcome::NoChatModel => {
            "서버에 채팅 모델이 보이지 않아요 — 모델을 로드한 뒤 설정을 열면 이어집니다."
        }
        _ => "모델 서버를 찾지 못했어요 — 서버를 켠 뒤 설정을 열면 자동으로 연결됩니다.",
    };
    (line.to_string(), true)
}

/// Plain-Korean line for a returning user whose persisted model's server is
/// unreachable (gray — the menu stays fully usable).
pub fn server_down_home_status() -> (String, bool) {
    (
        "모델 서버에 연결할 수 없어요 — 서버를 켠 뒤 설정을 열어 주세요.".to_string(),
        false,
    )
}

/// Re-run auto-setup when the settings menu opens while no model is
/// selected: a successfully adopted session provider is persisted (model
/// only — never the URL) and re-projected. `None` means the model was
/// already configured, so there was nothing to discover.
pub async fn reprobe_if_unconfigured(
    app: &mut App,
    provider_config: &mut ProviderConfig,
    provider: &mut Arc<OpenAiCompatibleProvider>,
    user_settings: &mut UserSettings,
    config: &mut RuntimeConfig,
    tts: &TtsService,
) -> Option<DiscoveryOutcome> {
    if !provider_config.model.trim().is_empty() {
        return None;
    }
    let outcome = discover_session_provider(provider_config).await;
    if let DiscoveryOutcome::Connected { base_url, model } = &outcome {
        let mut adopted = provider_config.clone();
        adopted.base_url = base_url.clone();
        adopted.model = model.clone();
        // Build before swapping: a failed construction leaves the old
        // provider fully in charge (same discipline as `apply_provider`).
        match OpenAiCompatibleProvider::new(adopted.clone()) {
            Ok(rebuilt) => {
                *provider = Arc::new(rebuilt);
                *provider_config = adopted;
                config.provider = provider_config.clone();
                user_settings.model = Some(model.clone());
                if let Err(error) = save_user_settings(&config.save_dir, user_settings) {
                    tracing::warn!(%error, "auto-selected model could not be persisted");
                }
                refresh_projection(app, provider_config, provider, user_settings, tts).await;
                refresh_model_list(app, provider, provider_config, false).await;
                tracing::info!(
                    base_url = %base_url,
                    model = %model,
                    "auto-setup adopted a local server"
                );
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    "auto-setup adopted provider build failed; keeping the old provider"
                );
            }
        }
    }
    Some(outcome)
}

/// Re-project the Settings screen after a model-stage activation: probe
/// health, read back the persisted overrides, and refresh the display.
async fn refresh_projection(
    app: &mut App,
    provider_config: &ProviderConfig,
    provider: &OpenAiCompatibleProvider,
    user_settings: &UserSettings,
    tts: &TtsService,
) {
    let health = provider
        .health()
        .await
        .unwrap_or(ProviderHealth::Unavailable);
    let tts_status = tts.status().await;
    refresh_view(
        app,
        provider_config,
        health,
        user_settings.director_model.clone(),
        user_settings.choice_model.clone(),
        user_settings.writer_model.clone(),
        &tts_status,
        app.images_enabled(),
        app.image_backend_label(),
        app.use_director_writer(),
        app.expressions_enabled(),
    );
}

/// Activate a different configured provider profile at runtime.
///
/// Rebuilds the provider (fresh endpoint + credentials), re-probes health,
/// repairs models that the new server does not advertise (mirroring the
/// startup repair), persists the choice, and refreshes the picker + settings
/// projection. The engine state is untouched — a provider swap is transparent
/// to the story in progress.
#[allow(clippy::too_many_arguments)] // mirrors the other apply_* activation signatures
pub async fn apply_provider(
    app: &mut App,
    name: &str,
    providers: &[ProviderConfig],
    provider_config: &mut ProviderConfig,
    provider: &mut Arc<OpenAiCompatibleProvider>,
    user_settings: &mut UserSettings,
    config: &mut RuntimeConfig,
    tts: &TtsService,
) -> Result<(), String> {
    let Some(selected) = providers.iter().find(|entry| entry.name == name) else {
        tracing::warn!(provider = %name, "requested provider is not configured");
        return Ok(());
    };
    tracing::info!(
        from = %provider_config.name,
        to = %selected.name,
        base_url = %selected.base_url,
        "switching provider"
    );
    // Build before swapping any state: a failed construction (rare — client
    // build) must leave the old provider fully in charge.
    let rebuilt = Arc::new(
        OpenAiCompatibleProvider::new(selected.clone())
            .map_err(|e| format!("프로바이더 '{name}'를 적용하지 못했습니다: {e}"))?,
    );
    *provider_config = selected.clone();
    config.provider = selected.clone();
    *provider = rebuilt;

    let health = provider
        .health()
        .await
        .unwrap_or(ProviderHealth::Unavailable);
    let advertised = provider.list_models().await.unwrap_or_default();

    // Mirror the startup repair: a model the new server does not advertise
    // falls back to the profile's own model, then to the first advertised id.
    if !advertised.is_empty() {
        let fallback = provider_config.model.clone();
        if let Some(model) = reconcile_active_model(&provider_config.model, &fallback, &advertised)
            && model != provider_config.model
        {
            tracing::warn!(
                stale_model = %provider_config.model,
                fallback_model = %model,
                provider = %name,
                "model is not advertised by the new provider; using an available one"
            );
            provider_config.model = model;
            config.provider = provider_config.clone();
        }
        for stage in [
            &mut config.director_model,
            &mut config.choice_model,
            &mut config.writer_model,
        ] {
            if let Some(current) = stage.clone()
                && !advertised.iter().any(|model| model == &current)
            {
                tracing::warn!(
                    stale_model = %current,
                    provider = %name,
                    "stage model is not advertised by the new provider; using the active model"
                );
                *stage = Some(provider_config.model.clone());
            }
        }
    }

    // Persist the provider choice and the (possibly repaired) models.
    user_settings.provider = Some(name.to_string());
    user_settings.model = Some(provider_config.model.clone());
    user_settings.director_model = config.director_model.clone();
    user_settings.choice_model = config.choice_model.clone();
    user_settings.writer_model = config.writer_model.clone();
    save_user_settings(&config.save_dir, user_settings)?;

    let tts_status = tts.status().await;
    refresh_view(
        app,
        provider_config,
        health,
        user_settings.director_model.clone(),
        user_settings.choice_model.clone(),
        user_settings.writer_model.clone(),
        &tts_status,
        app.images_enabled(),
        app.image_backend_label(),
        app.use_director_writer(),
        app.expressions_enabled(),
    );
    let roster: Vec<(String, String)> = providers
        .iter()
        .map(|entry| (entry.name.clone(), entry.base_url.clone()))
        .collect();
    app.set_providers(&roster, name);
    refresh_model_list(app, provider, provider_config, false).await;
    Ok(())
}

/// Persist and activate a newly selected model. This creates a fresh provider
/// so later generation requests cannot retain the prior model configuration.
pub async fn apply_model(
    app: &mut App,
    model: String,
    provider_config: &mut ProviderConfig,
    provider: &mut Arc<OpenAiCompatibleProvider>,
    user_settings: &mut UserSettings,
    save_dir: &Path,
    tts: &TtsService,
) -> Result<(), String> {
    provider_config.model = model.clone();
    user_settings.model = Some(model);
    save_user_settings(save_dir, user_settings)?;
    *provider = Arc::new(
        OpenAiCompatibleProvider::new(provider_config.clone())
            .map_err(|e| format!("선택한 모델을 적용하지 못했습니다: {e}"))?,
    );
    refresh_projection(app, provider_config, provider, user_settings, tts).await;
    Ok(())
}

/// Persist and activate a newly selected Director model.
pub async fn apply_director_model(
    app: &mut App,
    model: String,
    config: &mut RuntimeConfig,
    provider_config: &ProviderConfig,
    provider: &OpenAiCompatibleProvider,
    user_settings: &mut UserSettings,
    tts: &TtsService,
) -> Result<(), String> {
    config.director_model = Some(model.clone());
    user_settings.director_model = Some(model);
    save_user_settings(&config.save_dir, user_settings)?;
    refresh_projection(app, provider_config, provider, user_settings, tts).await;
    Ok(())
}

/// Persist and activate the lightweight choice-rewrite model.
pub async fn apply_choice_model(
    app: &mut App,
    model: String,
    config: &mut RuntimeConfig,
    provider_config: &ProviderConfig,
    provider: &OpenAiCompatibleProvider,
    user_settings: &mut UserSettings,
    tts: &TtsService,
) -> Result<(), String> {
    config.choice_model = Some(model.clone());
    user_settings.choice_model = Some(model);
    save_user_settings(&config.save_dir, user_settings)?;
    refresh_projection(app, provider_config, provider, user_settings, tts).await;
    Ok(())
}

/// Persist and activate a newly selected Writer model.
pub async fn apply_writer_model(
    app: &mut App,
    model: String,
    config: &mut RuntimeConfig,
    provider_config: &ProviderConfig,
    provider: &OpenAiCompatibleProvider,
    user_settings: &mut UserSettings,
    tts: &TtsService,
) -> Result<(), String> {
    config.writer_model = Some(model.clone());
    user_settings.writer_model = Some(model);
    save_user_settings(&config.save_dir, user_settings)?;
    refresh_projection(app, provider_config, provider, user_settings, tts).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        DiscoveryOutcome, filter_chat_models, pick_auto_model, probe_candidates,
        reconcile_active_model, server_down_home_status, unconfigured_home_status,
    };

    #[test]
    fn stale_model_falls_back_to_profile_model() {
        let advertised = vec!["available-a".to_string(), "profile".to_string()];
        assert_eq!(
            reconcile_active_model("removed", "profile", &advertised).as_deref(),
            Some("profile")
        );
    }

    #[test]
    fn stale_model_uses_first_advertised_when_profile_is_missing() {
        let advertised = vec!["available-a".to_string(), "available-b".to_string()];
        assert_eq!(
            reconcile_active_model("removed", "also-removed", &advertised).as_deref(),
            Some("available-a")
        );
    }

    #[test]
    fn chat_model_filter_excludes_embedding_rerank_and_audio_ids() {
        let advertised = vec![
            "text-embedding-nomic-embed-text-v1.5".to_string(),
            "whisper-large-v3".to_string(),
            "kokoro-tts".to_string(),
            "bge-reranker-v2".to_string(),
            "Qwen3.6-27B-Uncensored-Heretic-v2-MLX-4bit".to_string(),
        ];
        assert_eq!(
            filter_chat_models(&advertised),
            vec!["Qwen3.6-27B-Uncensored-Heretic-v2-MLX-4bit".to_string()]
        );
    }

    #[test]
    fn auto_model_pick_prefers_current_then_first_chat_model() {
        let advertised = vec![
            "text-embedding-nomic-embed-text-v1.5".to_string(),
            "chat-a".to_string(),
        ];
        assert_eq!(
            pick_auto_model("", "", &advertised).as_deref(),
            Some("chat-a")
        );
        assert_eq!(
            pick_auto_model("chat-a", "", &advertised).as_deref(),
            Some("chat-a")
        );
        // No chat model at all → nothing to pick.
        let embeddings = vec!["text-embedding-nomic-embed-text-v1.5".to_string()];
        assert_eq!(pick_auto_model("", "", &embeddings), None);
    }

    #[test]
    fn probe_candidates_puts_configured_first_and_dedups() {
        assert_eq!(
            probe_candidates("http://127.0.0.1:8000"),
            vec![
                "http://127.0.0.1:8000".to_string(),
                "http://127.0.0.1:8080".to_string(),
                "http://127.0.0.1:1234".to_string(),
                "http://127.0.0.1:11434".to_string(),
            ]
        );
        // A configured URL that matches a default is not probed twice.
        assert_eq!(
            probe_candidates("http://127.0.0.1:1234/"),
            vec![
                "http://127.0.0.1:1234".to_string(),
                "http://127.0.0.1:8080".to_string(),
                "http://127.0.0.1:11434".to_string(),
            ]
        );
        assert_eq!(probe_candidates("  "), probe_candidates(""));
    }

    #[test]
    fn home_status_lines_distinguish_unconfigured_and_server_down() {
        // Fresh clone, no server anywhere → attention line.
        let (line, attention) = unconfigured_home_status(&DiscoveryOutcome::NoServer);
        assert!(attention);
        assert!(line.contains("모델 서버를 찾지 못했어요"));
        // Server answered, but no chat model.
        let (line, attention) = unconfigured_home_status(&DiscoveryOutcome::NoChatModel);
        assert!(attention);
        assert!(line.contains("채팅 모델"));
        // Returning user with a persisted model but a down server → gray.
        let (line, attention) = server_down_home_status();
        assert!(!attention);
        assert!(line.contains("연결할 수 없어요"));
    }
}
