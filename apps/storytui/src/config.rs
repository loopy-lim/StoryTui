use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use story_provider::ProviderConfig;

#[derive(Clone, Debug, Deserialize)]
pub struct FileConfig {
    #[serde(default)]
    pub provider: ProviderConfig,
    /// Optional model override for the Director (planning & choice generation) step.
    #[serde(default)]
    pub director_model: Option<String>,
    /// Optional model override for the Writer (narration & prose stream) step.
    #[serde(default)]
    pub writer_model: Option<String>,
    #[serde(default = "default_save_dir")]
    pub save_dir: PathBuf,
    #[serde(default)]
    pub player: PlayerProfile,
    /// Maintainer-only two-stage Director → Writer rollout switch. It remains
    /// off until the local-model quality gate has been satisfied; it is not a
    /// player-facing story control.
    #[serde(default)]
    pub use_director_writer: bool,
}

/// Player identity injected into the model prompt for this local session.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct PlayerProfile {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub persona: Option<String>,
}

/// Per-user generation preferences stored independently from the project
/// provider configuration.
///
/// Every field is optional so an absent value continues to inherit the StoryTUI
/// default or the checked-in configuration file. This file intentionally never
/// contains endpoint or authentication settings.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct UserSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_budget_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
}

impl UserSettings {
    fn apply_to(&self, provider: &mut ProviderConfig) {
        if let Some(model) = &self.model {
            provider.model = model.clone();
        }
        if let Some(max_tokens) = self.max_tokens {
            provider.max_tokens = max_tokens;
        }
        if let Some(context_budget_tokens) = self.context_budget_tokens {
            provider.context_budget_tokens = context_budget_tokens;
        }
        if let Some(temperature) = self.temperature {
            provider.temperature = temperature;
        }
    }
}

/// The user-owned settings file under a save directory.
pub fn user_settings_path(save_dir: &Path) -> PathBuf {
    save_dir.join("user-settings.toml")
}

/// Load the persisted player-controlled generation preferences. The host uses
/// this when it updates one field (such as the selected model) so existing
/// overrides are preserved.
pub fn load_user_settings(save_dir: &Path) -> Result<UserSettings, String> {
    let path = user_settings_path(save_dir);
    if !path.exists() {
        return Ok(UserSettings::default());
    }

    let text = fs::read_to_string(&path)
        .map_err(|e| format!("사용자 설정을 읽지 못했습니다 ({}): {e}", path.display()))?;
    toml::from_str(&text).map_err(|e| {
        format!(
            "사용자 설정 TOML이 올바르지 않습니다 ({}): {e}",
            path.display()
        )
    })
}

/// Atomically persist the user-selectable provider preferences.
///
/// The output deliberately includes only model and sampling/context overrides;
/// base URLs, credentials, and project defaults remain in the base config.
pub fn save_user_settings(save_dir: &Path, settings: &UserSettings) -> Result<(), String> {
    fs::create_dir_all(save_dir).map_err(|e| {
        format!(
            "세이브 디렉터리를 만들지 못했습니다 ({}): {e}",
            save_dir.display()
        )
    })?;

    let path = user_settings_path(save_dir);
    let body = toml::to_string_pretty(settings)
        .map_err(|e| format!("사용자 설정을 직렬화하지 못했습니다: {e}"))?;
    let unique = format!(
        ".user-settings-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let temporary = save_dir.join(unique);

    let write_result = (|| -> Result<(), String> {
        let mut file = fs::File::create(&temporary).map_err(|e| {
            format!(
                "사용자 설정 임시 파일을 만들지 못했습니다 ({}): {e}",
                temporary.display()
            )
        })?;
        file.write_all(body.as_bytes()).map_err(|e| {
            format!(
                "사용자 설정 임시 파일을 쓰지 못했습니다 ({}): {e}",
                temporary.display()
            )
        })?;
        file.sync_all().map_err(|e| {
            format!(
                "사용자 설정 임시 파일을 동기화하지 못했습니다 ({}): {e}",
                temporary.display()
            )
        })?;
        fs::rename(&temporary, &path).map_err(|e| {
            format!(
                "사용자 설정을 적용하지 못했습니다 ({}): {e}",
                path.display()
            )
        })
    })();

    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

impl Default for FileConfig {
    fn default() -> Self {
        Self {
            provider: ProviderConfig::defaults(),
            director_model: None,
            writer_model: None,
            save_dir: default_save_dir(),
            player: PlayerProfile::default(),
            use_director_writer: false,
        }
    }
}

fn default_save_dir() -> PathBuf {
    PathBuf::from("saves")
}

#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    pub provider: ProviderConfig,
    pub director_model: Option<String>,
    pub writer_model: Option<String>,
    pub save_dir: PathBuf,
    pub player: PlayerProfile,
    pub use_director_writer: bool,
    pub pack_dir: PathBuf,
    pub source_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct CliOptions {
    pub provider_config: Option<PathBuf>,
    pub pack: Option<PathBuf>,
    pub save_dir: Option<PathBuf>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub api_key_env: Option<String>,
}

impl CliOptions {
    pub fn parse(args: Vec<String>) -> Result<Self, String> {
        let mut out = Self::default();
        let mut it = args.into_iter();
        while let Some(arg) = it.next() {
            let value = |name: &str, it: &mut std::vec::IntoIter<String>| {
                it.next().ok_or_else(|| format!("{name} 값이 필요합니다"))
            };
            match arg.as_str() {
                "-h" | "--help" => return Err(Self::usage()),
                "--provider-config" => {
                    out.provider_config = Some(PathBuf::from(value("--provider-config", &mut it)?));
                }
                "--pack" => out.pack = Some(PathBuf::from(value("--pack", &mut it)?)),
                "--save-dir" => out.save_dir = Some(PathBuf::from(value("--save-dir", &mut it)?)),
                "--model" => out.model = Some(value("--model", &mut it)?),
                "--base-url" => out.base_url = Some(value("--base-url", &mut it)?),
                "--api-key-env" => out.api_key_env = Some(value("--api-key-env", &mut it)?),
                other => return Err(format!("알 수 없는 인자: {other}\n\n{}", Self::usage())),
            }
        }
        Ok(out)
    }

    pub fn usage() -> String {
        "사용법: storytui [옵션]\n\n  --provider-config PATH  provider TOML\n  --pack PATH             Story Pack 디렉터리\n  --save-dir PATH         세이브 디렉터리\n  --model ID              provider.model 덮어쓰기\n  --base-url URL          provider.base_url 덮어쓰기\n  --api-key-env NAME      Bearer token을 읽을 환경변수\n  -h, --help              도움말".to_string()
    }
}

pub fn load(cli: &CliOptions) -> Result<RuntimeConfig, String> {
    let config_path = cli
        .provider_config
        .clone()
        .or_else(|| Some(PathBuf::from("config/storytui.toml")));
    let (mut file, source_path) = match config_path.filter(|p| p.exists()) {
        Some(path) => {
            let text = fs::read_to_string(&path)
                .map_err(|e| format!("설정 파일을 읽지 못했습니다 ({}): {e}", path.display()))?;
            let parsed: FileConfig =
                toml::from_str(&text).map_err(|e| format!("설정 TOML이 올바르지 않습니다: {e}"))?;
            let mut parsed = parsed;
            if let Some(secret_path) = parsed.provider.api_key_file.clone() {
                let resolved = if Path::new(&secret_path).is_absolute() {
                    PathBuf::from(&secret_path)
                } else {
                    path.parent()
                        .unwrap_or_else(|| Path::new("."))
                        .join(secret_path)
                };
                let secret = fs::read_to_string(&resolved).map_err(|e| {
                    format!(
                        "provider.api_key_file을 읽지 못했습니다 ({}): {e}",
                        resolved.display()
                    )
                })?;
                let secret = secret.trim().to_string();
                if secret.is_empty() {
                    return Err(format!(
                        "provider.api_key_file이 비어 있습니다: {}",
                        resolved.display()
                    ));
                }
                parsed.provider.api_key = Some(secret);
            }
            (parsed, Some(path))
        }
        None => (FileConfig::default(), None),
    };

    let settings = load_user_settings(&file.save_dir)?;
    settings.apply_to(&mut file.provider);

    if let Some(value) = &cli.model {
        file.provider.model = value.clone();
    }
    if let Some(value) = &cli.base_url {
        file.provider.base_url = value.clone();
    }
    if let Some(value) = &cli.api_key_env {
        file.provider.api_key_env = Some(value.clone());
    }
    if let Some(value) = &cli.save_dir {
        file.save_dir = value.clone();
    }

    let pack_dir = cli
        .pack
        .clone()
        .unwrap_or_else(|| PathBuf::from("stories/rainy-reunion"));
    if file.provider.model.trim().is_empty() {
        return Err(
            "provider.model이 비어 있습니다. --model 또는 TOML에서 모델을 지정하세요.".to_string(),
        );
    }
    if !Path::new(&pack_dir).is_dir() {
        return Err(format!(
            "Story Pack 디렉터리가 없습니다: {}",
            pack_dir.display()
        ));
    }

    Ok(RuntimeConfig {
        provider: file.provider,
        director_model: file.director_model,
        writer_model: file.writer_model,
        save_dir: file.save_dir,
        player: file.player,
        use_director_writer: file.use_director_writer,
        pack_dir,
        source_path,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::{
        CliOptions, FileConfig, UserSettings, load, save_user_settings, user_settings_path,
    };

    fn test_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "storytui-config-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("create test directory");
        dir
    }

    #[test]
    fn player_profile_is_optional_and_loads_from_toml() {
        let config: FileConfig =
            toml::from_str("[player]\nname = \"민준\"\npersona = \"말수가 적고 관찰력이 좋다\"\n")
                .expect("config");
        assert_eq!(config.player.name.as_deref(), Some("민준"));
        assert_eq!(
            config.player.persona.as_deref(),
            Some("말수가 적고 관찰력이 좋다")
        );
    }

    #[test]
    fn user_settings_override_base_before_cli_overrides() {
        let dir = test_dir("load-order");
        let save_dir = dir.join("saves");
        let pack_dir = dir.join("pack");
        let config_path = dir.join("storytui.toml");
        fs::create_dir_all(&pack_dir).expect("pack directory");
        fs::write(
            &config_path,
            format!(
                "save_dir = \"{}\"\n[provider]\nmodel = \"base-model\"\nmax_tokens = 100\ncontext_budget_tokens = 200\ntemperature = 0.1\n",
                save_dir.display()
            ),
        )
        .expect("base config");
        save_user_settings(
            &save_dir,
            &UserSettings {
                model: Some("saved-model".to_string()),
                max_tokens: Some(300),
                context_budget_tokens: Some(400),
                temperature: Some(0.7),
            },
        )
        .expect("save settings");

        let runtime = load(&CliOptions {
            provider_config: Some(config_path),
            pack: Some(pack_dir),
            model: Some("cli-model".to_string()),
            ..CliOptions::default()
        })
        .expect("load config");

        assert_eq!(runtime.provider.model, "cli-model");
        assert_eq!(runtime.provider.max_tokens, 300);
        assert_eq!(runtime.provider.context_budget_tokens, 400);
        assert_eq!(runtime.provider.temperature, 0.7);
        fs::remove_dir_all(dir).expect("clean test directory");
    }

    #[test]
    fn saves_only_user_overrides_and_replaces_existing_file() {
        let dir = test_dir("save");
        let settings = UserSettings {
            model: Some("qwen-local".to_string()),
            max_tokens: Some(1_800),
            context_budget_tokens: None,
            temperature: Some(0.2),
        };
        let path = user_settings_path(&dir);
        fs::write(&path, "base_url = \"must-not-survive\"\n").expect("old settings");

        save_user_settings(&dir, &settings).expect("save settings");

        let text = fs::read_to_string(&path).expect("saved settings");
        assert_eq!(
            toml::from_str::<UserSettings>(&text).expect("parse settings"),
            settings
        );
        assert!(text.contains("model = \"qwen-local\""));
        assert!(text.contains("max_tokens = 1800"));
        assert!(text.contains("temperature = 0.2"));
        assert!(!text.contains("context_budget_tokens"));
        assert!(!text.contains("base_url"));
        assert_eq!(
            fs::read_dir(&dir)
                .expect("read save directory")
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
                .count(),
            0
        );
        fs::remove_dir_all(dir).expect("clean test directory");
    }
}
