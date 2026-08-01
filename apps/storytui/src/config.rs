use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use story_provider::ProviderConfig;

#[derive(Clone, Debug, Deserialize)]
pub struct FileConfig {
    #[serde(default)]
    pub provider: ProviderConfig,
    #[serde(default = "default_save_dir")]
    pub save_dir: PathBuf,
    #[serde(default)]
    pub player: PlayerProfile,
}

/// Player identity injected into the model prompt for this local session.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct PlayerProfile {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub persona: Option<String>,
}

impl Default for FileConfig {
    fn default() -> Self {
        Self {
            provider: ProviderConfig::defaults(),
            save_dir: default_save_dir(),
            player: PlayerProfile::default(),
        }
    }
}

fn default_save_dir() -> PathBuf {
    PathBuf::from("saves")
}

#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    pub provider: ProviderConfig,
    pub save_dir: PathBuf,
    pub player: PlayerProfile,
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
        save_dir: file.save_dir,
        player: file.player,
        pack_dir,
        source_path,
    })
}

#[cfg(test)]
mod tests {
    use super::FileConfig;

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
}
