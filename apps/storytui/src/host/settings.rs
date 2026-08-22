//! Host-owned provider settings operations: safe UI projection, model
//! discovery, and persistence of player-selected model overrides.

use std::path::Path;
use std::sync::Arc;

use story_model::ProviderHealth;
use story_provider::{LlmProvider, OpenAiCompatibleProvider, ProviderConfig};
use story_tui::App;

use storytui::config::{RuntimeConfig, UserSettings, save_user_settings};
use storytui::diagnostic::provider_config_safe_with_models;

pub fn refresh_view(
    app: &mut App,
    provider_config: &ProviderConfig,
    health: ProviderHealth,
    director_model: Option<String>,
    choice_model: Option<String>,
    writer_model: Option<String>,
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
    );
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

/// Persist and activate a newly selected model. This creates a fresh provider
/// so later generation requests cannot retain the prior model configuration.
pub async fn apply_model(
    app: &mut App,
    model: String,
    provider_config: &mut ProviderConfig,
    provider: &mut Arc<OpenAiCompatibleProvider>,
    user_settings: &mut UserSettings,
    save_dir: &Path,
) -> Result<(), String> {
    provider_config.model = model.clone();
    user_settings.model = Some(model);
    save_user_settings(save_dir, user_settings)?;
    *provider = Arc::new(
        OpenAiCompatibleProvider::new(provider_config.clone())
            .map_err(|e| format!("선택한 모델을 적용하지 못했습니다: {e}"))?,
    );
    let health = provider
        .health()
        .await
        .unwrap_or(ProviderHealth::Unavailable);
    refresh_view(
        app,
        provider_config,
        health,
        user_settings.director_model.clone(),
        user_settings.choice_model.clone(),
        user_settings.writer_model.clone(),
    );
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
) -> Result<(), String> {
    config.director_model = Some(model.clone());
    user_settings.director_model = Some(model);
    save_user_settings(&config.save_dir, user_settings)?;
    let health = provider
        .health()
        .await
        .unwrap_or(ProviderHealth::Unavailable);
    refresh_view(
        app,
        provider_config,
        health,
        user_settings.director_model.clone(),
        user_settings.choice_model.clone(),
        user_settings.writer_model.clone(),
    );
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
) -> Result<(), String> {
    config.choice_model = Some(model.clone());
    user_settings.choice_model = Some(model);
    save_user_settings(&config.save_dir, user_settings)?;
    let health = provider
        .health()
        .await
        .unwrap_or(ProviderHealth::Unavailable);
    refresh_view(
        app,
        provider_config,
        health,
        user_settings.director_model.clone(),
        user_settings.choice_model.clone(),
        user_settings.writer_model.clone(),
    );
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
) -> Result<(), String> {
    config.writer_model = Some(model.clone());
    user_settings.writer_model = Some(model);
    save_user_settings(&config.save_dir, user_settings)?;
    let health = provider
        .health()
        .await
        .unwrap_or(ProviderHealth::Unavailable);
    refresh_view(
        app,
        provider_config,
        health,
        user_settings.director_model.clone(),
        user_settings.choice_model.clone(),
        user_settings.writer_model.clone(),
    );
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
