use std::collections::BTreeMap;
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
    /// Each entry may also carry per-profile role model defaults.
    #[serde(default)]
    pub providers: Vec<ProviderProfile>,
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
    /// Base default for the two-stage Director → Writer path (ADR-005). It
    /// stays `false` until the local-model quality gate passes; players opt
    /// in per machine through the Settings `g` toggle, which persists to
    /// `user-settings.toml` and overrides this base value.
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

/// A `[[providers]]` entry: the adapter config plus optional per-profile role
/// model defaults. `flatten` keeps the TOML shape of `ProviderConfig`
/// untouched — the role fields are additive siblings of `base_url` etc.
#[derive(Clone, Debug, Deserialize)]
pub struct ProviderProfile {
    #[serde(flatten)]
    pub config: ProviderConfig,
    /// Optional Director (planning & choice generation) model default.
    #[serde(default)]
    pub director_model: Option<String>,
    /// Optional lightweight choice-rewrite model default.
    #[serde(default)]
    pub choice_model: Option<String>,
    /// Optional Writer (narration & prose stream) model default.
    #[serde(default)]
    pub writer_model: Option<String>,
}

/// Role model assignments for one provider profile. Every absent field
/// inherits the next precedence level: the `[[providers]]` TOML default,
/// then the file-wide default.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct StageModels {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub director: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub writer: Option<String>,
}

impl StageModels {
    fn is_empty(&self) -> bool {
        self.director.is_none() && self.choice.is_none() && self.writer.is_none()
    }
}

/// A named sampling taste the Settings `x` cycle switches between. One key
/// replaces SillyTavern-style raw sampler sliders: players who never touch it
/// stay on 자동 — the profile's own tuning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SamplingStyle {
    /// Looser, more varied narration.
    Creative,
    /// The shipped default triple (Tech Spec §10.2).
    Balanced,
    /// Tighter, more deterministic narration.
    Precise,
}

impl SamplingStyle {
    /// Cycle order: 자동 → 창의적 → 균형 → 정밀 → 자동. `None` = 자동.
    pub fn next_after(current: Option<Self>) -> Option<Self> {
        match current {
            None => Some(Self::Creative),
            Some(Self::Creative) => Some(Self::Balanced),
            Some(Self::Balanced) => Some(Self::Precise),
            Some(Self::Precise) => None,
        }
    }

    /// Stable id persisted in `user-settings.toml`.
    pub fn id(self) -> &'static str {
        match self {
            Self::Creative => "creative",
            Self::Balanced => "balanced",
            Self::Precise => "precise",
        }
    }

    /// Korean label for the Settings row.
    pub fn label(self) -> &'static str {
        match self {
            Self::Creative => "창의적",
            Self::Balanced => "균형",
            Self::Precise => "정밀",
        }
    }

    /// Parse a persisted id; anything unrecognized means 자동 (`None`).
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "creative" => Some(Self::Creative),
            "balanced" => Some(Self::Balanced),
            "precise" => Some(Self::Precise),
            _ => None,
        }
    }

    /// The preset's `(temperature, top_p, top_k)`.
    pub fn sampling(self) -> (f32, f32, u32) {
        match self {
            Self::Creative => (1.0, 0.95, 60),
            Self::Balanced => (0.85, 0.92, 40),
            Self::Precise => (0.4, 0.8, 20),
        }
    }
}

/// Korean label for the 자동 state of the style cycle.
pub const SAMPLING_STYLE_AUTO_LABEL: &str = "자동";

/// Label shown on the Settings style row for a persisted style (or 자동).
pub fn sampling_style_label(style: Option<SamplingStyle>) -> String {
    style
        .map(SamplingStyle::label)
        .unwrap_or(SAMPLING_STYLE_AUTO_LABEL)
        .to_string()
}

/// Effective `(temperature, top_p, top_k)`: an active preset wins entirely;
/// 자동 falls back to the legacy user temperature override, then the active
/// profile's base values (the `base_sampling` snapshot taken before any
/// override was applied).
pub fn resolve_sampling(
    preset: Option<SamplingStyle>,
    base: (f32, f32, u32),
    user_temperature: Option<f32>,
) -> (f32, f32, u32) {
    match preset {
        Some(style) => style.sampling(),
        None => (user_temperature.unwrap_or(base.0), base.1, base.2),
    }
}

/// Effective player identity: user-settings fields win per-field over the
/// base `[player]` profile. `None` inherits; `Some("")` clears.
pub fn effective_player_profile(settings: &UserSettings, base: &PlayerProfile) -> PlayerProfile {
    PlayerProfile {
        name: settings.player_name.clone().or_else(|| base.name.clone()),
        persona: settings
            .player_persona
            .clone()
            .or_else(|| base.persona.clone()),
    }
}

/// Per-user generation preferences stored independently from the project
/// provider configuration.
///
/// Every field is optional so an absent value continues to inherit the StoryTUI
/// default or the checked-in configuration file. This file intentionally never
/// contains endpoint or authentication settings.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct UserSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Legacy flat stage assignments from before they became per-profile.
    /// Deserialized for older files; `fold_legacy_stage_models` migrates them
    /// into [`UserSettings::stage_models`] and these fields are never written
    /// again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub director_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choice_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub writer_model: Option<String>,
    /// Per-profile role model assignments keyed by provider profile name
    /// (`[stage_models.<name>]`). The active profile's entry wins over the
    /// `[[providers]]` TOML defaults and the file-wide defaults.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub stage_models: BTreeMap<String, StageModels>,
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
    /// Expression sprite preference from the Settings `f` toggle. `None`
    /// means on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expressions_enabled: Option<bool>,
    /// Image render protocol forced from the Settings `b` cycle (`kitty` /
    /// `sixel` / `iterm2` / `halfblocks`). `None` — or any unrecognized
    /// value — means auto: follow the terminal capability detected at
    /// startup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_protocol: Option<String>,
    /// Generation-path preference from the Settings `g` toggle (`true` = the
    /// two-stage Director → Writer streaming path). `None` inherits the base
    /// config's `use_director_writer` (ADR-005 default: `false`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_director_writer: Option<bool>,
    /// Single-pass SSE streaming-render preference from the Settings `s`
    /// toggle. `None` inherits the provider profile's
    /// `[provider] stream_turns`. Meaningful only on the single-pass path —
    /// the two-stage path always streams its Writer stage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_turns: Option<bool>,
    /// Response-style preset id from the Settings `x` cycle
    /// (`creative` / `balanced` / `precise`). `None` — or any unrecognized
    /// value — means 자동: the active profile's own sampling stays in charge.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampling_preset: Option<String>,
    /// Player identity override from the Settings `a` persona modal, winning
    /// per-field over the base `[player]` profile. `Some("")` explicitly
    /// clears a field; `None` inherits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub player_name: Option<String>,
    /// Persona text override (see [`UserSettings::player_name`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub player_persona: Option<String>,
    /// Author's note (SillyTavern-style steering note) from the Settings `t`
    /// modal, injected verbatim into every turn's system prompt. `Some("")`
    /// is stored by an explicit clear; an absent field means "no note".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author_note: Option<String>,
    /// TOML-only display post-processing rules applied to newly shown
    /// transcript text (`[[display_rules]]`). No UI by design — raw regex is
    /// a power-user knob like the provider URL. Order is significant.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub display_rules: Vec<DisplayRule>,
    /// TOML-only quick actions (`[quick_actions]`): `"1" = "텍스트"` fills
    /// the free-input buffer via Alt+1..Alt+9. No UI by design; absent map
    /// means the shortcuts do nothing.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub quick_actions: BTreeMap<String, String>,
    /// Resume the previous autosaved session right after startup instead of
    /// showing the home menu first. Absent field means on (`true`) so a
    /// returning player lands one keypress from play; `false` keeps the
    /// classic home-first flow.
    #[serde(default = "default_auto_resume")]
    pub auto_resume: bool,
}

fn default_auto_resume() -> bool {
    true
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            provider: None,
            model: None,
            director_model: None,
            choice_model: None,
            writer_model: None,
            stage_models: BTreeMap::new(),
            max_tokens: None,
            context_budget_tokens: None,
            temperature: None,
            tts_enabled: None,
            images_enabled: None,
            expressions_enabled: None,
            image_protocol: None,
            use_director_writer: None,
            stream_turns: None,
            sampling_preset: None,
            player_name: None,
            player_persona: None,
            author_note: None,
            display_rules: Vec::new(),
            quick_actions: BTreeMap::new(),
            auto_resume: default_auto_resume(),
        }
    }
}

/// One display post-processing rule: `pattern` is replaced with
/// `replacement` in newly displayed transcript text, in declaration order.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct DisplayRule {
    pub pattern: String,
    pub replacement: String,
    /// A disabled rule is kept in the file but skipped at apply time.
    #[serde(default = "default_display_rule_enabled")]
    pub enabled: bool,
}

fn default_display_rule_enabled() -> bool {
    true
}

/// Upper bound on applied display rules — a runaway `[[display_rules]]`
/// table must not be able to make the transcript crawl.
pub const MAX_DISPLAY_RULES: usize = 16;

/// Upper bound (grapheme-cluster intent, char count here) on one
/// quick-action text.
pub const QUICK_ACTION_MAX_CHARS: usize = 200;

impl UserSettings {
    /// The note injected into every turn's prompt: the trimmed stored value,
    /// or `None` when unset or explicitly cleared with an empty string.
    pub fn effective_author_note(&self) -> Option<&str> {
        self.author_note
            .as_deref()
            .map(str::trim)
            .filter(|note| !note.is_empty())
    }

    /// The quick-action text bound to `key` (`"1"`..`"9"`), or `None` for
    /// anything else — unknown keys, oversized values, and blank text all
    /// mean "no action".
    pub fn quick_action(&self, key: &str) -> Option<&str> {
        if !matches!(key, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9") {
            return None;
        }
        self.quick_actions
            .get(key)
            .map(|text| text.trim())
            .filter(|text| !text.is_empty() && text.chars().count() <= QUICK_ACTION_MAX_CHARS)
    }

    /// Sampling/context overrides are machine-level tastes: applied at load
    /// and re-applied on provider switches. `model` is deliberately excluded —
    /// a switch repairs the model against the new server's advertised list.
    pub fn apply_sampling_overrides(&self, provider: &mut ProviderConfig) {
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

    /// The style preset persisted by the `x` cycle (unknown id = 자동).
    pub fn sampling_style(&self) -> Option<SamplingStyle> {
        self.sampling_preset
            .as_deref()
            .and_then(SamplingStyle::from_id)
    }

    /// Machine-level sampling taste onto a profile: the legacy per-field
    /// overrides, then the style preset — an active preset decides
    /// temperature/top_p/top_k outright, the coarser and more recent knob.
    /// Applied at load and re-applied on provider switches.
    pub fn apply_sampling_taste(&self, provider: &mut ProviderConfig) {
        self.apply_sampling_overrides(provider);
        if let Some(style) = self.sampling_style() {
            let (temperature, top_p, top_k) = style.sampling();
            provider.temperature = temperature;
            provider.top_p = top_p;
            provider.top_k = top_k;
        }
    }

    /// Fold the legacy flat stage fields into the active profile's map entry
    /// (only when the player has not already curated that profile) and clear
    /// the flat fields so the next save completes the migration.
    pub fn fold_legacy_stage_models(&mut self, active_profile: &str) {
        if !self.stage_models.contains_key(active_profile) {
            let legacy = StageModels {
                director: self.director_model.clone(),
                choice: self.choice_model.clone(),
                writer: self.writer_model.clone(),
            };
            if !legacy.is_empty() {
                self.stage_models.insert(active_profile.to_string(), legacy);
            }
        }
        self.director_model = None;
        self.choice_model = None;
        self.writer_model = None;
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
    /// Per-profile role model defaults from the `[[providers]]` TOML entries,
    /// keyed by profile name. The user-settings `stage_models` map outranks
    /// these; both outrank the file-wide defaults.
    pub stage_defaults: BTreeMap<String, StageModels>,
    /// Base-profile model used as a safe fallback when a persisted user model
    /// is no longer advertised by the local server.
    pub fallback_model: String,
    /// The active profile's pristine `(temperature, top_p, top_k)`, snapshotted
    /// before any user override or style preset is applied. The style cycle's
    /// 자동 state restores these values; refreshed whenever a provider switch
    /// clones a fresh profile.
    pub base_sampling: (f32, f32, u32),
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
    for entry in file.providers {
        let mut config = entry.config;
        if let Some(secret_path) = config.api_key_file.clone() {
            let label = format!("provider '{}'의 api_key_file", config.name);
            config.api_key = Some(read_secret_file(
                &secret_path,
                source_path.as_deref(),
                &label,
            )?);
        }
        roster.push(config);
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
    let mut extras = std::mem::take(&mut file.providers);
    for extra in &extras {
        if extra.config.name == file.provider.name {
            return Err(format!(
                "provider 이름이 중복됩니다: '{}' — [provider]와 [[providers]] 항목은 서로 다른 name을 가져야 합니다.",
                extra.config.name
            ));
        }
    }
    {
        let mut seen = std::collections::BTreeSet::new();
        for extra in &extras {
            if !seen.insert(extra.config.name.as_str()) {
                return Err(format!(
                    "provider 이름이 중복됩니다: '{}' — [[providers]] 항목끼리 name을 구별해 주세요.",
                    extra.config.name
                ));
            }
        }
    }
    // Resolve per-profile key files up front so the TUI can switch to any
    // profile without a restart; env-var credentials stay lazy (resolved per
    // provider construction).
    for entry in extras.iter_mut() {
        if let Some(secret_path) = entry.config.api_key_file.clone() {
            let label = format!("provider '{}'의 api_key_file", entry.config.name);
            entry.config.api_key = Some(read_secret_file(
                &secret_path,
                source_path.as_deref(),
                &label,
            )?);
        }
    }
    // Per-profile role model defaults, keyed for the precedence chain
    // (user-settings map first, then these).
    let mut stage_defaults: BTreeMap<String, StageModels> = BTreeMap::new();
    let mut providers = Vec::with_capacity(extras.len() + 1);
    for entry in extras {
        stage_defaults.insert(
            entry.config.name.clone(),
            StageModels {
                director: entry.director_model,
                choice: entry.choice_model,
                writer: entry.writer_model,
            },
        );
        providers.push(entry.config);
    }
    // The base profile is switchable too — without this, one runtime switch
    // would make the original `[provider]` unreachable until restart.
    providers.insert(0, file.provider.clone());

    let fallback_model = file.provider.model.clone();
    let mut settings = load_user_settings(&file.save_dir)?;
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
    if let Some(model) = &settings.model {
        file.provider.model = model.clone();
    }
    // Snapshot the profile's own tuning before any user taste is layered on —
    // the style cycle's 자동 restores exactly these values.
    let base_sampling = (
        file.provider.temperature,
        file.provider.top_p,
        file.provider.top_k,
    );
    settings.apply_sampling_taste(&mut file.provider);
    // Stage assignments: the active profile's saved choice first, then the
    // `[[providers]]` TOML default, then the file-wide default. Legacy flat
    // user-settings fields fold into the active profile's entry first so an
    // old file keeps working and migrates on the next save.
    let active_profile = file.provider.name.clone();
    settings.fold_legacy_stage_models(&active_profile);
    let saved = settings.stage_models.get(&active_profile);
    let default = stage_defaults.get(&active_profile);
    file.director_model = saved
        .and_then(|s| s.director.clone())
        .or_else(|| default.and_then(|d| d.director.clone()))
        .or(file.director_model.take());
    file.choice_model = saved
        .and_then(|s| s.choice.clone())
        .or_else(|| default.and_then(|d| d.choice.clone()))
        .or(file.choice_model.take());
    file.writer_model = saved
        .and_then(|s| s.writer.clone())
        .or_else(|| default.and_then(|d| d.writer.clone()))
        .or(file.writer_model.take());

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
        stage_defaults,
        fallback_model,
        base_sampling,
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

    use story_provider::{MaxTokensField, ProviderConfig};

    use super::{
        CliOptions, FileConfig, PlayerProfile, QUICK_ACTION_MAX_CHARS, SamplingStyle, StageModels,
        UserSettings, effective_player_profile, load, load_user_settings, preferred_config_path,
        resolve_sampling, sampling_style_label, save_user_settings, user_settings_path,
    };
    use std::collections::BTreeMap;

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
    fn note_rules_and_quick_actions_round_trip_and_validate() {
        let dir = test_dir("note-rules-quick");
        let mut settings = UserSettings {
            author_note: Some("  장면은 조용하게, 문장은 짧게.  ".to_string()),
            ..UserSettings::default()
        };

        // Author's note: trimmed text is effective, empty is an explicit clear.
        assert_eq!(
            settings.effective_author_note(),
            Some("장면은 조용하게, 문장은 짧게.")
        );
        settings.author_note = Some("   ".to_string());
        assert_eq!(settings.effective_author_note(), None);

        // Quick actions: only "1".."9" resolve, blank and oversized are ignored.
        settings.quick_actions.insert(
            "2".to_string(),
            format!("{}넘는 텍스트", "아".repeat(QUICK_ACTION_MAX_CHARS)),
        );
        settings
            .quick_actions
            .insert("3".to_string(), "   ".to_string());
        settings
            .quick_actions
            .insert("x".to_string(), "해킹".to_string());
        assert!(settings.quick_action("2").is_none(), "oversized is ignored");
        assert!(settings.quick_action("3").is_none(), "blank is ignored");
        assert!(
            settings.quick_action("x").is_none(),
            "non-digit key is ignored"
        );
        settings
            .quick_actions
            .insert("1".to_string(), "  조심스럽게 문을 연다. ".to_string());
        assert_eq!(settings.quick_action("1"), Some("조심스럽게 문을 연다."));

        // Round trip through user-settings.toml.
        settings.author_note = Some("조용한 장면".to_string());
        save_user_settings(&dir, &settings).expect("save");
        let loaded = load_user_settings(&dir).expect("load");
        assert_eq!(loaded.effective_author_note(), Some("조용한 장면"));
        assert_eq!(loaded.quick_action("1"), Some("조심스럽게 문을 연다."));
        assert_eq!(
            loaded.display_rules, settings.display_rules,
            "empty rules stay absent"
        );

        // An explicit [[display_rules]] table parses with the enabled default.
        fs::write(
            user_settings_path(&dir),
            "[[display_rules]]\npattern = \"\\\\d+\"\nreplacement = \"N\"\n[[display_rules]]\npattern = \"x\"\nreplacement = \"y\"\nenabled = false\n",
        )
        .expect("write rules file");
        let loaded = load_user_settings(&dir).expect("load rules");
        assert_eq!(loaded.display_rules.len(), 2);
        assert!(loaded.display_rules[0].enabled);
        assert!(!loaded.display_rules[1].enabled);

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
    fn user_settings_image_protocol_round_trips_and_tolerates_unknown() {
        let dir = test_dir("image-protocol-persist");
        let settings = UserSettings {
            image_protocol: Some("halfblocks".to_string()),
            ..UserSettings::default()
        };
        save_user_settings(&dir, &settings).expect("save");
        assert_eq!(load_user_settings(&dir).expect("load"), settings);
        // A stale/unknown value must not break the whole settings file: the
        // value round-trips verbatim and the host treats it as auto.
        let path = user_settings_path(&dir);
        fs::write(&path, "image_protocol = \"kity\"\n").expect("write unknown value");
        let loaded = load_user_settings(&dir).expect("unknown value still parses");
        assert_eq!(loaded.image_protocol.as_deref(), Some("kity"));
        fs::remove_dir_all(dir).expect("clean test directory");
    }

    #[test]
    fn user_settings_generation_mode_round_trips_and_absent_inherits() {
        let dir = test_dir("generation-toggle-persist");
        // Absent field: a pre-toggle file (or none at all) loads as `None`,
        // which means "inherit the base config default".
        assert!(
            load_user_settings(&dir)
                .expect("missing file")
                .use_director_writer
                .is_none()
        );
        let settings = UserSettings {
            use_director_writer: Some(true),
            stream_turns: None,
            ..UserSettings::default()
        };
        save_user_settings(&dir, &settings).expect("save");
        assert_eq!(load_user_settings(&dir).expect("load"), settings);
        fs::remove_dir_all(dir).expect("clean test directory");
    }

    #[test]
    fn user_settings_expression_toggle_round_trips_and_absent_means_on() {
        let dir = test_dir("expression-toggle-persist");
        // Absent field: a pre-toggle file (or none at all) loads as `None`,
        // which means "expression sprites on".
        assert!(
            load_user_settings(&dir)
                .expect("missing file")
                .expressions_enabled
                .is_none()
        );
        let settings = UserSettings {
            expressions_enabled: Some(false),
            ..UserSettings::default()
        };
        save_user_settings(&dir, &settings).expect("save");
        assert_eq!(load_user_settings(&dir).expect("load"), settings);
        fs::remove_dir_all(dir).expect("clean test directory");
    }

    #[test]
    fn user_settings_auto_resume_round_trips_and_absent_means_on() {
        let dir = test_dir("auto-resume-persist");
        // Missing file: the in-memory default is already "resume".
        assert!(load_user_settings(&dir).expect("missing file").auto_resume);

        // An explicit false survives the round trip...
        let off = UserSettings {
            auto_resume: false,
            ..UserSettings::default()
        };
        save_user_settings(&dir, &off).expect("save off");
        assert_eq!(load_user_settings(&dir).expect("load off"), off);

        // ...as does an explicit true.
        let on = UserSettings {
            auto_resume: true,
            ..UserSettings::default()
        };
        save_user_settings(&dir, &on).expect("save on");
        assert_eq!(load_user_settings(&dir).expect("load on"), on);

        // A pre-field file (or a hand-trimmed one) parses as on.
        let path = user_settings_path(&dir);
        fs::write(&path, "model = \"base-model\"\n").expect("write pre-field file");
        let loaded = load_user_settings(&dir).expect("absent field still parses");
        assert!(loaded.auto_resume, "absent auto_resume means on");
        assert_eq!(loaded.model.as_deref(), Some("base-model"));

        // A wrongly typed value is a malformed settings file: it fails with
        // the standard Korean parse error instead of silently flipping on.
        fs::write(&path, "auto_resume = \"yes\"\n").expect("write invalid value");
        let error = load_user_settings(&dir).expect_err("non-bool must fail");
        assert!(
            error.contains("올바르지 않습니다"),
            "Korean parse error expected: {error}"
        );

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
                stage_models: BTreeMap::new(),
                max_tokens: Some(300),
                context_budget_tokens: Some(400),
                temperature: Some(0.7),
                tts_enabled: None,
                images_enabled: None,
                expressions_enabled: None,
                image_protocol: None,
                use_director_writer: None,
                stream_turns: None,
                sampling_preset: None,
                player_name: None,
                player_persona: None,
                author_note: None,
                display_rules: Vec::new(),
                quick_actions: BTreeMap::new(),
                auto_resume: true,
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
            choice_model: None,
            writer_model: None,
            stage_models: BTreeMap::from([(
                "omlx-local".to_string(),
                StageModels {
                    choice: Some("qwen-choice".to_string()),
                    ..StageModels::default()
                },
            )]),
            max_tokens: Some(1_800),
            context_budget_tokens: None,
            temperature: Some(0.2),
            tts_enabled: Some(true),
            images_enabled: Some(false),
            expressions_enabled: Some(false),
            image_protocol: Some("halfblocks".to_string()),
            use_director_writer: Some(true),
            stream_turns: None,
            sampling_preset: Some("precise".to_string()),
            player_name: Some("민준".to_string()),
            player_persona: None,
            author_note: None,
            display_rules: Vec::new(),
            quick_actions: BTreeMap::new(),
            auto_resume: true,
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
        assert!(text.contains("[stage_models.omlx-local]"));
        assert!(text.contains("choice = \"qwen-choice\""));
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
    fn stream_turns_override_round_trips() {
        // The Settings `s` toggle persists `Some(bool)`; an untouched setup
        // keeps the field absent so the profile's `stream_turns` stays in
        // charge.
        let dir = test_dir("stream-roundtrip");
        let settings = UserSettings {
            stream_turns: Some(false),
            ..UserSettings::default()
        };
        save_user_settings(&dir, &settings).expect("save settings");

        let path = user_settings_path(&dir);
        let text = fs::read_to_string(&path).expect("saved settings");
        assert!(text.contains("stream_turns = false"));
        assert_eq!(
            toml::from_str::<UserSettings>(&text).expect("parse settings"),
            settings
        );

        // Absent field → None → profile inheritance on the next startup.
        fs::write(&path, "").expect("empty settings");
        let loaded = load_user_settings(&dir).expect("load settings");
        assert_eq!(loaded.stream_turns, None);
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

    #[test]
    fn profile_toml_role_fields_become_stage_defaults_and_seed_active_profile() {
        let dir = test_dir("profile-stage-defaults");
        let pack_dir = dir.join("pack");
        let config_path = dir.join("storytui.toml");
        fs::create_dir_all(&pack_dir).expect("pack directory");
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
                    "model = \"or-main\"\n",
                    "api_key_env = \"OPENROUTER_KEY\"\n",
                    "director_model = \"or-director\"\n",
                    "writer_model = \"or-writer\"\n",
                ),
                dir.join("saves").display()
            ),
        )
        .expect("config");

        let runtime = load(&CliOptions {
            provider_config: Some(config_path.clone()),
            pack: Some(pack_dir.clone()),
            ..CliOptions::default()
        })
        .expect("load config");
        // Defaults are keyed by profile name regardless of activity.
        let defaults = runtime.stage_defaults.get("openrouter").expect("defaults");
        assert_eq!(defaults.director.as_deref(), Some("or-director"));
        assert_eq!(defaults.writer.as_deref(), Some("or-writer"));
        assert_eq!(defaults.choice, None);

        // A persisted selection activates the profile; its TOML defaults seed
        // the runtime stage fields where no saved choice exists.
        save_user_settings(
            &runtime.save_dir,
            &UserSettings {
                provider: Some("openrouter".to_string()),
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
        assert_eq!(runtime.provider.name, "openrouter");
        assert_eq!(runtime.director_model.as_deref(), Some("or-director"));
        assert_eq!(runtime.writer_model.as_deref(), Some("or-writer"));
        assert_eq!(runtime.choice_model, None);
        fs::remove_dir_all(dir).expect("clean test directory");
    }

    #[test]
    fn user_settings_stage_models_outrank_profile_toml_defaults() {
        let dir = test_dir("stage-precedence");
        let pack_dir = dir.join("pack");
        let config_path = dir.join("storytui.toml");
        fs::create_dir_all(&pack_dir).expect("pack directory");
        fs::write(
            &config_path,
            format!(
                concat!(
                    "save_dir = \"{}\"\n",
                    "[provider]\n",
                    "name = \"omlx-local\"\n",
                    "model = \"base-model\"\n",
                    "[[providers]]\n",
                    "name = \"cloud\"\n",
                    "base_url = \"https://api.groq.com/openai/v1\"\n",
                    "model = \"cloud-main\"\n",
                    "director_model = \"toml-director\"\n",
                    "choice_model = \"toml-choice\"\n",
                ),
                dir.join("saves").display()
            ),
        )
        .expect("config");
        save_user_settings(
            &dir.join("saves"),
            &UserSettings {
                provider: Some("cloud".to_string()),
                stage_models: BTreeMap::from([(
                    "cloud".to_string(),
                    StageModels {
                        director: Some("saved-director".to_string()),
                        ..StageModels::default()
                    },
                )]),
                ..UserSettings::default()
            },
        )
        .expect("save settings");

        let runtime = load(&CliOptions {
            provider_config: Some(config_path),
            pack: Some(pack_dir),
            ..CliOptions::default()
        })
        .expect("load config");

        // Saved choice wins over the TOML default; a field the saved entry
        // omits falls through to the TOML default.
        assert_eq!(runtime.director_model.as_deref(), Some("saved-director"));
        assert_eq!(runtime.choice_model.as_deref(), Some("toml-choice"));
        fs::remove_dir_all(dir).expect("clean test directory");
    }

    #[test]
    fn style_cycle_round_trips_through_auto_and_parses_unknown_as_auto() {
        // 자동 → 창의적 → 균형 → 정밀 → 자동: the full cycle closes.
        let cycle = SamplingStyle::next_after(None);
        assert_eq!(cycle, Some(SamplingStyle::Creative));
        assert_eq!(
            SamplingStyle::next_after(cycle),
            Some(SamplingStyle::Balanced)
        );
        assert_eq!(
            SamplingStyle::next_after(SamplingStyle::next_after(cycle)),
            Some(SamplingStyle::Precise)
        );
        assert_eq!(
            SamplingStyle::next_after(SamplingStyle::from_id("precise")),
            None
        );
        // Round-trip by id and the 자동 fallback for stale values.
        for style in [
            SamplingStyle::Creative,
            SamplingStyle::Balanced,
            SamplingStyle::Precise,
        ] {
            assert_eq!(SamplingStyle::from_id(style.id()), Some(style));
        }
        assert_eq!(SamplingStyle::from_id("kity"), None);
        assert_eq!(sampling_style_label(None), "자동");
        assert_eq!(
            sampling_style_label(SamplingStyle::from_id("creative")),
            "창의적"
        );
    }

    #[test]
    fn resolve_sampling_preset_wins_and_auto_restores_base_taste() {
        let base = (0.85, 0.92, 40);
        // An active preset decides the whole triple, ignoring a legacy
        // per-field temperature override.
        assert_eq!(
            resolve_sampling(SamplingStyle::from_id("precise"), base, Some(0.1)),
            (0.4, 0.8, 20)
        );
        // 자동: the legacy temperature override still applies to the base.
        assert_eq!(resolve_sampling(None, base, Some(0.1)), (0.1, 0.92, 40));
        // 자동 without a legacy override is exactly the profile's own tuning.
        assert_eq!(resolve_sampling(None, base, None), base);
    }

    #[test]
    fn effective_player_profile_overrides_and_inherits_per_field() {
        let base = PlayerProfile {
            name: Some("토마스".to_string()),
            persona: Some("기본 페르소나".to_string()),
        };
        // Nothing set: both fields inherit.
        let settings = UserSettings::default();
        let effective = effective_player_profile(&settings, &base);
        assert_eq!(effective.name.as_deref(), Some("토마스"));
        assert_eq!(effective.persona.as_deref(), Some("기본 페르소나"));
        // Each field wins independently; Some("") explicitly clears.
        let settings = UserSettings {
            player_name: Some(String::new()),
            player_persona: Some("조용한 관찰자".to_string()),
            ..UserSettings::default()
        };
        let effective = effective_player_profile(&settings, &base);
        assert_eq!(effective.name.as_deref(), Some(""));
        assert_eq!(effective.persona.as_deref(), Some("조용한 관찰자"));
    }

    #[test]
    fn apply_sampling_taste_preset_overrides_sampling_but_not_limits() {
        let mut provider = ProviderConfig::defaults();
        provider.temperature = 0.85;
        provider.top_p = 0.92;
        provider.top_k = 40;
        provider.max_tokens = 1800;
        let settings = UserSettings {
            max_tokens: Some(2_000),
            temperature: Some(0.1),
            sampling_preset: Some("creative".to_string()),
            ..UserSettings::default()
        };
        settings.apply_sampling_taste(&mut provider);
        // The preset decides temperature/top_p/top_k outright...
        assert_eq!(provider.temperature, 1.0);
        assert_eq!(provider.top_p, 0.95);
        assert_eq!(provider.top_k, 60);
        // ...while the legacy max_tokens override is untouched.
        assert_eq!(provider.max_tokens, 2_000);
    }

    #[test]
    fn user_settings_style_and_persona_fields_round_trip() {
        let dir = test_dir("style-persona-persist");
        let settings = UserSettings {
            sampling_preset: Some("creative".to_string()),
            player_name: Some("민준".to_string()),
            player_persona: Some("말수가 적고 관찰력이 좋다".to_string()),
            ..UserSettings::default()
        };
        save_user_settings(&dir, &settings).expect("save");
        assert_eq!(load_user_settings(&dir).expect("load"), settings);
        // A stale preset id parses as a file but resolves to 자동.
        let path = user_settings_path(&dir);
        fs::write(&path, "sampling_preset = \"kity\"\n").expect("write stale id");
        let loaded = load_user_settings(&dir).expect("stale id still parses");
        assert_eq!(loaded.sampling_style(), None);
        fs::remove_dir_all(dir).expect("clean test directory");
    }

    #[test]
    fn legacy_flat_stage_fields_fold_into_the_active_profile() {
        let dir = test_dir("legacy-stage-fold");
        let pack_dir = dir.join("pack");
        let config_path = dir.join("storytui.toml");
        fs::create_dir_all(&pack_dir).expect("pack directory");
        fs::write(
            &config_path,
            format!(
                "save_dir = \"{}\"\n[provider]\nname = \"omlx-local\"\nmodel = \"base-model\"\n",
                dir.join("saves").display()
            ),
        )
        .expect("config");
        // Old-style flat file: no stage_models map, flat fields only.
        save_user_settings(
            &dir.join("saves"),
            &UserSettings {
                choice_model: Some("old-choice".to_string()),
                ..UserSettings::default()
            },
        )
        .expect("save legacy settings");

        let runtime = load(&CliOptions {
            provider_config: Some(config_path),
            pack: Some(pack_dir),
            ..CliOptions::default()
        })
        .expect("load config");

        // The flat value seeds the active profile's assignment...
        assert_eq!(runtime.choice_model.as_deref(), Some("old-choice"));
        // ...and the same fold the host performs would persist it per profile.
        let mut migrated = load_user_settings(&runtime.save_dir).expect("reload");
        migrated.fold_legacy_stage_models(&runtime.provider.name);
        assert_eq!(
            migrated
                .stage_models
                .get("omlx-local")
                .and_then(|s| s.choice.as_deref()),
            Some("old-choice")
        );
        assert_eq!(migrated.choice_model, None, "flat fields are cleared");
        fs::remove_dir_all(dir).expect("clean test directory");
    }
}
