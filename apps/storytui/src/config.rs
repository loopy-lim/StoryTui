use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use story_provider::ProviderConfig;
use story_tts::TtsConfig;

#[derive(Clone, Debug, Deserialize)]
pub struct FileConfig {
    #[serde(default)]
    pub provider: ProviderConfig,
    /// Additional named provider profiles (`[[providers]]`). The `[provider]`
    /// table above stays the default; entries here are selectable in the TUI.
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    /// Optional model override for the Director (planning & choice generation) step.
    #[serde(default)]
    pub director_model: Option<String>,
    /// Optional lightweight model used only when rejected choices are rewritten.
    #[serde(default)]
    pub choice_model: Option<String>,
    /// Optional model override for the Writer (narration & prose stream) step.
    #[serde(default)]
    pub writer_model: Option<String>,
    /// Narration text-to-speech (`v` toggle at runtime). Absent block means
    /// the say backend, disabled.
    #[serde(default)]
    pub tts: TtsConfig,
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
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub director_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choice_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub writer_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_budget_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Narration read-aloud preference from the `v` toggle. `None` inherits
    /// the `[tts] enabled` base value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tts_enabled: Option<bool>,
    /// Scene image preference from the Settings `i` toggle. `None` means on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images_enabled: Option<bool>,
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
            providers: Vec::new(),
            director_model: None,
            choice_model: None,
            writer_model: None,
            tts: TtsConfig::default(),
            save_dir: default_save_dir(),
            player: PlayerProfile::default(),
            use_director_writer: false,
        }
    }
}

fn default_save_dir() -> PathBuf {
    PathBuf::from("saves")
}

fn preferred_config_path(local: &Path, public: &Path) -> PathBuf {
    if local.exists() {
        local.to_path_buf()
    } else {
        public.to_path_buf()
    }
}

fn preferred_default_config_path() -> PathBuf {
    let local = PathBuf::from("config/storytui.local.toml");
    let public = PathBuf::from("config/storytui.toml");
    preferred_config_path(&local, &public)
}

#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    /// The active profile. `model` may be empty — that means "not picked
    /// yet": the host preflight discovers a local server and auto-selects a
    /// chat model (see `host::settings::discover_session_provider`) instead
    /// of failing the whole launch.
    pub provider: ProviderConfig,
    /// Every configured provider profile (base first), resolved and ready to
    /// be swapped in at runtime by the TUI provider picker.
    pub providers: Vec<ProviderConfig>,
    /// Base-profile model used as a safe fallback when a persisted user model
    /// is no longer advertised by the local server.
    pub fallback_model: String,
    pub director_model: Option<String>,
    pub choice_model: Option<String>,
    pub writer_model: Option<String>,
    /// Narration TTS profile with the secret already resolved in memory.
    pub tts: TtsConfig,
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

/// Read an `api_key_file`-style secret: absolute path, or relative to the
/// config file's directory. The value is trimmed and must be non-empty.
/// `label` prefixes the Korean error message (e.g. `provider.api_key_file`).
fn read_secret_file(
    secret_path: &str,
    source_path: Option<&Path>,
    label: &str,
) -> Result<String, String> {
    let resolved = if Path::new(secret_path).is_absolute() {
        PathBuf::from(secret_path)
    } else {
        source_path
            .and_then(Path::parent)
            .unwrap_or_else(|| Path::new("."))
            .join(secret_path)
    };
    let secret = fs::read_to_string(&resolved)
        .map_err(|e| format!("{label}을 읽지 못했습니다 ({}): {e}", resolved.display()))?;
    let secret = secret.trim().to_string();
    if secret.is_empty() {
        return Err(format!("{label}이 비어 있습니다: {}", resolved.display()));
    }
    Ok(secret)
}

/// Provider roster for the import subcommand: the base `[provider]` first,
/// then every `[[providers]]` entry, with `api_key_file` secrets resolved.
/// Deliberately lighter than [`load`] — no pack directory or model checks, so
/// importing works on a machine whose story config points elsewhere.
pub fn provider_roster(config_path: Option<&Path>) -> Result<Vec<ProviderConfig>, String> {
    let path = config_path
        .map(Path::to_path_buf)
        .unwrap_or_else(preferred_default_config_path);
    if !path.exists() {
        return Err(format!(
            "설정 파일이 없습니다: {} — --provider-config로 경로를 지정하세요",
            path.display()
        ));
    }
    let text = fs::read_to_string(&path)
        .map_err(|e| format!("설정 파일을 읽지 못했습니다 ({}): {e}", path.display()))?;
    let file: FileConfig =
        toml::from_str(&text).map_err(|e| format!("설정 TOML이 올바르지 않습니다: {e}"))?;
    let source_path = Some(path);

    let mut base = file.provider;
    if let Some(secret_path) = base.api_key_file.clone() {
        base.api_key = Some(read_secret_file(
            &secret_path,
            source_path.as_deref(),
            "provider.api_key_file",
        )?);
    }
    let mut roster = vec![base];
    for mut entry in file.providers {
        if let Some(secret_path) = entry.api_key_file.clone() {
            let label = format!("provider '{}'의 api_key_file", entry.name);
            entry.api_key = Some(read_secret_file(
                &secret_path,
                source_path.as_deref(),
                &label,
            )?);
        }
        roster.push(entry);
    }
    Ok(roster)
}

pub fn load(cli: &CliOptions) -> Result<RuntimeConfig, String> {
    let config_path = cli
        .provider_config
        .clone()
        .or_else(|| Some(preferred_default_config_path()));
    let (mut file, source_path) = match config_path.filter(|p| p.exists()) {
        Some(path) => {
            let text = fs::read_to_string(&path)
                .map_err(|e| format!("설정 파일을 읽지 못했습니다 ({}): {e}", path.display()))?;
            let parsed: FileConfig =
                toml::from_str(&text).map_err(|e| format!("설정 TOML이 올바르지 않습니다: {e}"))?;
            (parsed, Some(path))
        }
        None => (FileConfig::default(), None),
    };

    if file.provider.kind != "openai_compatible" {
        tracing::warn!(
            kind = %file.provider.kind,
            "unknown provider kind; only 'openai_compatible' is implemented and the value is ignored"
        );
    }

    // The CLI save directory determines both saves and the user-settings
    // namespace. Apply it before reading user-settings so an isolated run
    // cannot accidentally inherit the project's normal saved model.
    if let Some(value) = &cli.save_dir {
        file.save_dir = value.clone();
    }
    // Resolve the base profile's key file BEFORE the roster is assembled so
    // the roster's base entry carries the resolved credential.
    if cli.api_key_env.is_none()
        && let Some(secret_path) = file.provider.api_key_file.clone()
    {
        file.provider.api_key = Some(read_secret_file(
            &secret_path,
            source_path.as_deref(),
            "provider.api_key_file",
        )?);
    }
    // The provider roster: the base `[provider]` first, then `[[providers]]`
    // entries. Names must be unique — the TUI picker and the persisted
    // user preference address profiles by name.
    let mut providers = std::mem::take(&mut file.providers);
    for extra in &providers {
        if extra.name == file.provider.name {
            return Err(format!(
                "provider 이름이 중복됩니다: '{}' — [provider]와 [[providers]] 항목은 서로 다른 name을 가져야 합니다.",
                extra.name
            ));
        }
    }
    {
        let mut seen = std::collections::BTreeSet::new();
        for extra in &providers {
            if !seen.insert(extra.name.as_str()) {
                return Err(format!(
                    "provider 이름이 중복됩니다: '{}' — [[providers]] 항목끼리 name을 구별해 주세요.",
                    extra.name
                ));
            }
        }
    }
    // Resolve per-profile key files up front so the TUI can switch to any
    // profile without a restart; env-var credentials stay lazy (resolved per
    // provider construction).
    for entry in providers.iter_mut() {
        if let Some(secret_path) = entry.api_key_file.clone() {
            let label = format!("provider '{}'의 api_key_file", entry.name);
            entry.api_key = Some(read_secret_file(
                &secret_path,
                source_path.as_deref(),
                &label,
            )?);
        }
    }
    // The base profile is switchable too — without this, one runtime switch
    // would make the original `[provider]` unreachable until restart.
    providers.insert(0, file.provider.clone());

    let fallback_model = file.provider.model.clone();
    let settings = load_user_settings(&file.save_dir)?;
    // A persisted provider choice selects the active profile before model
    // overrides are applied, so models always resolve against the right
    // server's list.
    if let Some(name) = &settings.provider
        && let Some(selected) = providers.iter().find(|entry| &entry.name == name)
    {
        file.provider = selected.clone();
    } else if settings.provider.is_some() {
        tracing::warn!(
            provider = %settings.provider.clone().unwrap_or_default(),
            "persisted provider no longer exists; keeping the base profile"
        );
    }
    settings.apply_to(&mut file.provider);
    if let Some(dm) = &settings.director_model {
        file.director_model = Some(dm.clone());
    }
    if let Some(cm) = &settings.choice_model {
        file.choice_model = Some(cm.clone());
    }
    if let Some(wm) = &settings.writer_model {
        file.writer_model = Some(wm.clone());
    }

    if let Some(value) = &cli.model {
        file.provider.model = value.clone();
    }
    if let Some(value) = &cli.base_url {
        file.provider.base_url = value.clone();
    }
    if let Some(value) = &cli.api_key_env {
        file.provider.api_key_env = Some(value.clone());
        // An explicit environment credential must be able to replace a stale
        // or missing file credential from the base profile.
        file.provider.api_key_file = None;
        file.provider.api_key = None;
    }

    // TTS credentials: own file > own env var > the provider's resolved key
    // (an OpenAI-compatible cloud endpoint and the TTS backend usually
    // share one key).
    if let Some(secret_path) = file.tts.api_key_file.clone() {
        file.tts.api_key = Some(read_secret_file(
            &secret_path,
            source_path.as_deref(),
            "tts.api_key_file",
        )?);
    } else if let Ok(value) = std::env::var(&file.tts.api_key_env) {
        let value = value.trim().to_string();
        if !value.is_empty() {
            file.tts.api_key = Some(value);
        }
    }
    if file.tts.api_key.is_none() {
        file.tts.api_key = file.provider.api_key.clone();
    }

    let pack_dir = cli
        .pack
        .clone()
        .unwrap_or_else(|| PathBuf::from("stories/rainy-reunion"));
    if !Path::new(&pack_dir).is_dir() {
        return Err(format!(
            "Story Pack 디렉터리가 없습니다: {}",
            pack_dir.display()
        ));
    }

    Ok(RuntimeConfig {
        provider: file.provider,
        providers,
        fallback_model,
        director_model: file.director_model,
        choice_model: file.choice_model,
        writer_model: file.writer_model,
        tts: file.tts,
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

    use story_provider::MaxTokensField;

    use super::{
        CliOptions, FileConfig, UserSettings, load, load_user_settings, preferred_config_path,
        save_user_settings, user_settings_path,
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
    fn ignored_local_profile_is_preferred_over_public_template() {
        let dir = test_dir("preferred-local-profile");
        let public = dir.join("storytui.toml");
        let local = dir.join("storytui.local.toml");
        fs::write(&public, "[provider]\n").expect("public profile");

        assert_eq!(preferred_config_path(&local, &public), public);

        fs::write(&local, "[provider]\n").expect("local profile");
        assert_eq!(preferred_config_path(&local, &public), local);

        fs::remove_dir_all(dir).expect("clean test directory");
    }

    #[test]
    fn hosted_style_base_url_passes_through_to_runtime() {
        let dir = test_dir("hosted-url");
        let pack_dir = dir.join("pack");
        let config_path = dir.join("storytui.toml");
        fs::create_dir_all(&pack_dir).expect("pack directory");
        fs::write(
            &config_path,
            format!(
                concat!(
                    "save_dir = \"{}\"\n",
                    "[provider]\n",
                    "base_url = \"https://openrouter.ai/api/v1\"\n",
                    "model = \"임의-호환-모델\"\n",
                    "api_key_env = \"MY_COMPATIBLE_KEY\"\n",
                    "max_tokens_field = \"max_completion_tokens\"\n",
                ),
                dir.join("saves").display()
            ),
        )
        .expect("config");

        let runtime = load(&CliOptions {
            provider_config: Some(config_path),
            pack: Some(pack_dir),
            ..CliOptions::default()
        })
        .expect("load config");

        // The URL is the only required change; paths stay at their defaults
        // here and the provider adapts them at construction time.
        assert_eq!(runtime.provider.base_url, "https://openrouter.ai/api/v1");
        assert_eq!(runtime.provider.chat_path, "/v1/chat/completions");
        assert_eq!(
            runtime.provider.max_tokens_field,
            MaxTokensField::MaxCompletionTokens
        );
        assert_eq!(runtime.provider.model, "임의-호환-모델");
        fs::remove_dir_all(dir).expect("clean test directory");
    }

    #[test]
    fn load_succeeds_with_empty_model_and_defers_to_host_preflight() {
        let dir = test_dir("empty-model");
        let pack_dir = dir.join("pack");
        let config_path = dir.join("storytui.toml");
        fs::create_dir_all(&pack_dir).expect("pack directory");
        fs::write(
            &config_path,
            format!(
                concat!(
                    "save_dir = \"{}\"\n",
                    "[provider]\n",
                    "base_url = \"http://127.0.0.1:8000\"\n",
                    // No `model` line: the checked-in template looks like
                    // this on a fresh clone.
                ),
                dir.join("saves").display()
            ),
        )
        .expect("config");

        let runtime = load(&CliOptions {
            provider_config: Some(config_path),
            pack: Some(pack_dir),
            ..CliOptions::default()
        })
        .expect("an empty model must not abort the launch");

        assert_eq!(runtime.provider.model, "");
        assert_eq!(runtime.fallback_model, "");
        fs::remove_dir_all(dir).expect("clean test directory");
    }

    #[test]
    fn tts_config_loads_with_key_fallbacks() {
        let dir = test_dir("tts-config");
        let pack_dir = dir.join("pack");
        let key_file = dir.join("tts-key");
        let config_path = dir.join("storytui.toml");
        fs::create_dir_all(&pack_dir).expect("pack directory");
        fs::write(&key_file, "tts-file-secret\n").expect("key file");
        fs::write(
            &config_path,
            format!(
                concat!(
                    "save_dir = \"{}\"\n",
                    "[provider]\n",
                    "model = \"base-model\"\n",
                    "[tts]\n",
                    "enabled = true\n",
                    "backend = \"openai\"\n",
                    "voice = \"shimmer\"\n",
                    "api_key_file = \"tts-key\"\n"
                ),
                dir.join("saves").display()
            ),
        )
        .expect("config");

        let runtime = load(&CliOptions {
            provider_config: Some(config_path),
            pack: Some(pack_dir),
            ..CliOptions::default()
        })
        .expect("load config");

        assert!(runtime.tts.enabled);
        assert_eq!(runtime.tts.backend, story_tts::TtsBackendKind::OpenAi);
        assert_eq!(runtime.tts.voice.as_deref(), Some("shimmer"));
        assert_eq!(runtime.tts.api_key.as_deref(), Some("tts-file-secret"));
        fs::remove_dir_all(dir).expect("clean test directory");
    }

    #[test]
    fn tts_key_falls_back_to_provider_file_key() {
        let dir = test_dir("tts-key-fallback");
        let pack_dir = dir.join("pack");
        let key_file = dir.join("provider-key");
        let config_path = dir.join("storytui.toml");
        fs::create_dir_all(&pack_dir).expect("pack directory");
        fs::write(&key_file, "provider-secret\n").expect("key file");
        fs::write(
            &config_path,
            format!(
                concat!(
                    "save_dir = \"{}\"\n",
                    "[provider]\n",
                    "base_url = \"https://api.openai.com/v1\"\n",
                    "model = \"gpt-5.5-mini\"\n",
                    "api_key_file = \"provider-key\"\n",
                    "[tts]\n",
                    "backend = \"openai\"\n",
                    "api_key_env = \"STORYTUI_TEST_UNSET_KEY\"\n"
                ),
                dir.join("saves").display()
            ),
        )
        .expect("config");

        let runtime = load(&CliOptions {
            provider_config: Some(config_path),
            pack: Some(pack_dir),
            ..CliOptions::default()
        })
        .expect("load config");

        assert_eq!(runtime.tts.api_key.as_deref(), Some("provider-secret"));
        fs::remove_dir_all(dir).expect("clean test directory");
    }

    #[test]
    fn multiple_providers_parse_and_select_by_persisted_name() {
        let dir = test_dir("multi-provider");
        let pack_dir = dir.join("pack");
        let config_path = dir.join("storytui.toml");
        fs::create_dir_all(&pack_dir).expect("pack directory");
        // The save dir lives inside the temp directory: a relative `saves`
        // default would leak user-settings state between test runs.
        fs::write(
            &config_path,
            format!(
                concat!(
                    "save_dir = \"{}\"\n",
                    "[provider]\n",
                    "name = \"omlx-local\"\n",
                    "model = \"base-model\"\n",
                    "[[providers]]\n",
                    "name = \"openrouter\"\n",
                    "base_url = \"https://openrouter.ai/api/v1\"\n",
                    "model = \"openrouter-model\"\n",
                    "api_key_env = \"OPENROUTER_KEY\"\n",
                    "[[providers]]\n",
                    "name = \"groq\"\n",
                    "base_url = \"https://api.groq.com/openai/v1\"\n",
                    "model = \"groq-model\"\n",
                ),
                dir.join("saves").display()
            ),
        )
        .expect("config");

        // Without a persisted choice the base profile stays active.
        let runtime = load(&CliOptions {
            provider_config: Some(config_path.clone()),
            pack: Some(pack_dir.clone()),
            ..CliOptions::default()
        })
        .expect("load config");
        assert_eq!(runtime.provider.name, "omlx-local");
        // The roster includes the base profile first — it must remain
        // switchable at runtime.
        assert_eq!(runtime.providers.len(), 3);
        assert_eq!(runtime.providers[0].name, "omlx-local");
        assert_eq!(runtime.providers[1].name, "openrouter");
        assert_eq!(runtime.providers[2].name, "groq");

        // A persisted provider name activates that profile at startup, and
        // user model overrides apply to the selected profile.
        save_user_settings(
            &runtime.save_dir,
            &UserSettings {
                provider: Some("groq".to_string()),
                ..UserSettings::default()
            },
        )
        .expect("save choice");
        let runtime = load(&CliOptions {
            provider_config: Some(config_path),
            pack: Some(pack_dir),
            ..CliOptions::default()
        })
        .expect("load config");
        assert_eq!(runtime.provider.name, "groq");
        assert_eq!(runtime.provider.base_url, "https://api.groq.com/openai/v1");
        fs::remove_dir_all(dir).expect("clean test directory");
    }

    #[test]
    fn duplicate_provider_names_are_rejected_in_korean() {
        let dir = test_dir("dup-provider");
        let pack_dir = dir.join("pack");
        let config_path = dir.join("storytui.toml");
        fs::create_dir_all(&pack_dir).expect("pack directory");
        fs::write(
            &config_path,
            concat!(
                "[provider]\n",
                "name = \"omlx-local\"\n",
                "model = \"base-model\"\n",
                "[[providers]]\n",
                "name = \"omlx-local\"\n",
                "base_url = \"http://127.0.0.1:9000\"\n",
            ),
        )
        .expect("config");

        let error = load(&CliOptions {
            provider_config: Some(config_path),
            pack: Some(pack_dir),
            ..CliOptions::default()
        })
        .expect_err("duplicate names must fail");
        assert!(error.contains("중복"), "Korean error expected: {error}");
        fs::remove_dir_all(dir).expect("clean test directory");
    }

    #[test]
    fn missing_persisted_provider_falls_back_to_base_with_warning() {
        let dir = test_dir("stale-provider");
        let pack_dir = dir.join("pack");
        let config_path = dir.join("storytui.toml");
        fs::create_dir_all(&pack_dir).expect("pack directory");
        // save_dir is pinned inside the tempdir: the cwd-relative "saves"
        // default would make this test read unrelated state (or none).
        fs::write(
            &config_path,
            format!(
                "save_dir = \"{}\"\n[provider]\nname = \"omlx-local\"\nmodel = \"base-model\"\n",
                dir.join("saves").display()
            ),
        )
        .expect("config");
        save_user_settings(
            &dir.join("saves"),
            &UserSettings {
                provider: Some("deleted-profile".to_string()),
                ..UserSettings::default()
            },
        )
        .expect("save stale choice");

        let runtime = load(&CliOptions {
            provider_config: Some(config_path),
            pack: Some(pack_dir),
            ..CliOptions::default()
        })
        .expect("load config");
        assert_eq!(runtime.provider.name, "omlx-local", "base profile wins");
        fs::remove_dir_all(dir).expect("clean test directory");
    }

    #[test]
    fn user_settings_tts_enabled_round_trips() {
        let dir = test_dir("tts-toggle-persist");
        let settings = UserSettings {
            tts_enabled: Some(true),
            images_enabled: Some(false),
            ..UserSettings::default()
        };
        save_user_settings(&dir, &settings).expect("save");
        assert_eq!(load_user_settings(&dir).expect("load"), settings);
        fs::remove_dir_all(dir).expect("clean test directory");
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
                provider: None,
                model: Some("saved-model".to_string()),
                director_model: None,
                choice_model: Some("saved-choice-model".to_string()),
                writer_model: None,
                max_tokens: Some(300),
                context_budget_tokens: Some(400),
                temperature: Some(0.7),
                tts_enabled: None,
                images_enabled: None,
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
        assert_eq!(runtime.choice_model.as_deref(), Some("saved-choice-model"));
        fs::remove_dir_all(dir).expect("clean test directory");
    }

    #[test]
    fn saves_only_user_overrides_and_replaces_existing_file() {
        let dir = test_dir("save");
        let settings = UserSettings {
            provider: None,
            model: Some("qwen-local".to_string()),
            director_model: None,
            choice_model: Some("qwen-choice".to_string()),
            writer_model: None,
            max_tokens: Some(1_800),
            context_budget_tokens: None,
            temperature: Some(0.2),
            tts_enabled: Some(true),
            images_enabled: Some(false),
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
        assert!(text.contains("choice_model = \"qwen-choice\""));
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

    #[test]
    fn cli_save_dir_selects_the_user_settings_namespace() {
        let dir = test_dir("cli-save-dir");
        let base_saves = dir.join("base-saves");
        let cli_saves = dir.join("cli-saves");
        let pack_dir = dir.join("pack");
        let config_path = dir.join("storytui.toml");
        fs::create_dir_all(&pack_dir).expect("pack directory");
        fs::write(
            &config_path,
            format!(
                "save_dir = \"{}\"\n[provider]\nmodel = \"base-model\"\n",
                base_saves.display()
            ),
        )
        .expect("base config");
        save_user_settings(
            &base_saves,
            &UserSettings {
                model: Some("stale-base-save-model".to_string()),
                ..UserSettings::default()
            },
        )
        .expect("base settings");
        save_user_settings(
            &cli_saves,
            &UserSettings {
                model: Some("cli-save-model".to_string()),
                ..UserSettings::default()
            },
        )
        .expect("cli settings");

        let runtime = load(&CliOptions {
            provider_config: Some(config_path),
            pack: Some(pack_dir),
            save_dir: Some(cli_saves.clone()),
            ..CliOptions::default()
        })
        .expect("load config");

        assert_eq!(runtime.save_dir, cli_saves);
        assert_eq!(runtime.provider.model, "cli-save-model");
        assert_eq!(runtime.fallback_model, "base-model");
        fs::remove_dir_all(dir).expect("clean test directory");
    }

    #[test]
    fn cli_api_key_env_bypasses_a_missing_base_key_file() {
        let dir = test_dir("cli-api-key-env");
        let pack_dir = dir.join("pack");
        let config_path = dir.join("storytui.toml");
        fs::create_dir_all(&pack_dir).expect("pack directory");
        fs::write(
            &config_path,
            "[provider]\nmodel = \"base-model\"\napi_key_file = \"missing.secret\"\n",
        )
        .expect("base config");

        let runtime = load(&CliOptions {
            provider_config: Some(config_path),
            pack: Some(pack_dir),
            api_key_env: Some("STORYTUI_TEST_TOKEN".to_string()),
            ..CliOptions::default()
        })
        .expect("load config");

        assert_eq!(
            runtime.provider.api_key_env.as_deref(),
            Some("STORYTUI_TEST_TOKEN")
        );
        assert_eq!(runtime.provider.api_key_file, None);
        assert_eq!(runtime.provider.api_key, None);
        fs::remove_dir_all(dir).expect("clean test directory");
    }
}
