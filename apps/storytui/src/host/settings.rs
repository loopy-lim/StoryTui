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
    use super::reconcile_active_model;

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
}
