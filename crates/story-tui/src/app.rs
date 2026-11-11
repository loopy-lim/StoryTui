//! The TUI `App` state + key dispatch (ST-021).
//!
//! [`App`] holds the latest [`AppViewModel`] the engine published, the
//! selection cursor, and a quit flag. [`App::handle_key`] maps a key to an
//! optional [`EngineCommand`] the host should run; [`App::render`] draws the
//! Story screen into a ratatui `Frame`.
//!
//! The App itself never calls the engine or the provider — it only translates
//! input to intents and renders the view it was handed. The host (`apps/
//! storytui`) owns the event loop and the `EngineState`.

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use image::DynamicImage;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui_image::picker::Picker;
use ratatui_image::sliced::SlicedProtocol;
use story_core::engine_cmd::EngineCommand;
use story_model::uuid::Uuid;
use story_model::{ActionType, AppViewModel, Emotion, LocationId, ParsedAction, RelationshipStage};
use story_model::{ProviderConfigSafe, ProviderHealth};
use story_persistence::SlotMeta;

use crate::event::{KeyAction, SELECT_SENTINEL, map_key, parse_direct_select_index};
use crate::input::InputBuffer;
use crate::screens::greeting::greeting_selection_at;
use crate::screens::home::{menu_selection_at, render_home};
use crate::screens::pack::pack_selection_at;
use crate::screens::save_load::{render_save_load, save_load_selection_at};
use crate::screens::settings::{SettingsView, render_settings};
use crate::screens::story::{
    ListMode, ModelRoleHints, StoryBlock, StoryFooterAction, StoryStackEntry, choice_index_at,
    footer_action_at, markdown_lines, model_picker_scroll, render_help_overlay,
    render_history_delete_confirm, render_history_edit_dialog, render_list, render_model_picker,
    render_provider_picker, render_story, story_entry_lines, streaming_entry_lines,
};
use crate::theme::Styles;

/// Which manual save manager the host should open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveLoadMode {
    Save,
    Load,
}

/// A launcher action that requires host-owned state or I/O.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HomeRequest {
    StartNew,
    Continue,
    ReturnToGame,
    /// Home menu "팩 선택": the host scans the pack root and opens the
    /// picker (via `open_pack_picker`).
    PickPack,
}

/// The TTS slice of the Settings projection (plain display data).
#[derive(Clone, Debug)]
pub struct TtsDisplay {
    pub enabled: bool,
    pub backend: String,
    pub voice: Option<String>,
}

/// Card import dialog state: path input → browse → running → report.
#[derive(Clone, Debug)]
enum ImportDialog {
    /// Typing the card file path.
    Input { buffer: InputBuffer },
    /// Browsing directories for a card file. `input` parks the path-entry
    /// buffer so the Tab round-trip never loses typed text.
    Browse {
        dir: String,
        entries: Vec<(String, bool)>,
        selected: usize,
        input: InputBuffer,
    },
    /// The host is running the import (keys ignored).
    Running { path: String },
    /// Finished. `ready` carries the pack dir the player can switch to.
    Report {
        path: String,
        lines: Vec<String>,
        ready: Option<String>,
    },
}

/// Join a browse directory and entry name into a path string.
fn join_browse_path(dir: &str, name: &str) -> String {
    format!("{}/{}", dir.trim_end_matches('/'), name)
}

/// Parent directory of a submitted card path, remembered so the next import
/// dialog can prefill it. `None` for bare names with no parent.
fn parent_dir_of(path: &str) -> Option<String> {
    std::path::Path::new(path)
        .parent()
        .map(|parent| parent.display().to_string())
        .filter(|dir| !dir.is_empty())
}

#[derive(Clone, Debug)]
struct SettingsState {
    provider: ProviderConfigSafe,
    health: ProviderHealth,
    temperature: f32,
    top_p: f32,
    top_k: u32,
    max_tokens: u32,
    tts: TtsDisplay,
    images_enabled: bool,
    image_backend: Option<String>,
    /// Whether `image_backend` was player-forced (Settings `b` cycle).
    image_backend_manual: bool,
    /// Two-stage Director → Writer generation path (streaming narration).
    use_director_writer: bool,
    /// Single-pass SSE streaming-render preference (Settings `s` toggle).
    /// The host's `config.provider.stream_turns` stays authoritative; this
    /// mirror is what the Settings screen renders.
    stream_turns: bool,
    /// Whether the active provider advertises SSE streaming (the row notes
    /// a `켜짐` preference that cannot take effect).
    stream_supported: bool,
    /// Whether expression sprites are currently shown.
    expressions_enabled: bool,
    /// Response-style preset label (`자동` when the profile's own tuning is
    /// in charge) shown at the top of the sampling block.
    sampling_style: String,
    /// Persona row label for the status block (`없음` / name / `설정됨`).
    persona_label: String,
    /// Effective persona values, prefilled into the edit modal. Empty means
    /// the field is off (nothing inherited or explicitly cleared).
    persona_name: String,
    persona_text: String,
}

/// Two-field persona editor opened from the Settings screen (`a`). Plain
/// buffers so the host receives exactly the submitted text.
#[derive(Clone, Debug)]
struct PersonaDialogState {
    name: InputBuffer,
    persona: InputBuffer,
    /// Focused field: 0 = 이름, 1 = 페르소나.
    focus: usize,
}

/// Buffer capacity for the persona modal fields (grapheme clusters).
const PERSONA_NAME_MAX_CHARS: usize = 32;
const PERSONA_TEXT_MAX_CHARS: usize = 400;

/// Single-field author's note editor opened from the Settings screen (`t`).
#[derive(Clone, Debug)]
struct AuthorNoteDialogState {
    note: InputBuffer,
}

/// Buffer capacity for the author's note modal (grapheme clusters).
const AUTHOR_NOTE_MAX_CHARS: usize = 400;

/// Multi-line narration editor opened from History (`Enter`).
#[derive(Clone, Debug)]
struct HistoryEditDialog {
    entry_id: Uuid,
    turn: u32,
    buffer: InputBuffer,
}

/// Buffer capacity for History narration edits (grapheme clusters).
const HISTORY_NARRATION_MAX_CHARS: usize = 8_000;

/// Lorebook data the host pushes for the Settings `w` editor. `persisted`
/// mirrors "a lorebook.toml exists to save into" — when false the dialog
/// labels the edits session-only.
#[derive(Clone, Debug)]
struct LorebookData {
    entries: Vec<story_model::LorebookEntry>,
    persisted: bool,
}

/// Lorebook editor opened from the Settings screen (`w`). Holds a working
/// copy of the entries; every mutation flags `dirty`, and leaving the list
/// while dirty hands the full updated list to the host.
#[derive(Clone, Debug)]
struct LorebookDialogState {
    entries: Vec<story_model::LorebookEntry>,
    selected: usize,
    persisted: bool,
    dirty: bool,
    /// Open single-line editor for the selected entry, if any.
    edit: Option<LorebookEdit>,
}

/// Which field of the selected lorebook entry the single-line editor holds.
#[derive(Clone, Debug)]
enum LorebookEdit {
    Content { buffer: InputBuffer },
    Keys { buffer: InputBuffer },
}

/// Editor capacities for the lorebook modal fields (grapheme clusters).
const LOREBOOK_CONTENT_MAX_CHARS: usize = 2_000;
const LOREBOOK_KEYS_MAX_CHARS: usize = 400;

/// Last-sent prompts the host captured before a provider call. Session
/// memory only — never persisted, like every other host-side capture.
#[derive(Clone, Debug)]
struct PromptPreview {
    system: String,
    user: String,
}

/// A scene image pre-encoded for the detected terminal graphics protocol.
/// Encoding happens once per location when the host resolves it; rendering is
/// cheap per frame. Held behind an `Arc` (the app's clone shares payloads),
/// so the type itself needs no `Clone`.
pub struct SceneImage {
    pub protocol: SlicedProtocol,
    /// Encoded size in terminal cells (aspect-fit into the cell budget).
    pub cols: u16,
    pub rows: u16,
}

impl std::fmt::Debug for SceneImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The encoded payload is opaque protocol bytes; only the cell geometry
        // is meaningful in diagnostics.
        f.debug_struct("SceneImage")
            .field("cols", &self.cols)
            .field("rows", &self.rows)
            .finish()
    }
}

/// Upper bound for an inline scene image so it never dominates the transcript.
const SCENE_IMAGE_MAX_COLS: u16 = 26;
const SCENE_IMAGE_MAX_ROWS: u16 = 14;
/// Tighter portrait budget for expression sprites — an emotion beat must
/// stay visually subordinate to the location art around it.
const EMOTE_IMAGE_MAX_COLS: u16 = 14;
const EMOTE_IMAGE_MAX_ROWS: u16 = 14;

impl SceneImage {
    /// Encode `image` fitted into the inline cell budget. `None` when the
    /// protocol encode fails; the player then simply sees text only.
    ///
    /// The protocol is the `sliced` variant so the renderer can show the
    /// correct slice of a partially scrolled image — plain protocols always
    /// draw from the image's first row.
    pub fn encode(picker: &Picker, image: DynamicImage) -> Option<Self> {
        Self::encode_in(picker, image, SCENE_IMAGE_MAX_COLS, SCENE_IMAGE_MAX_ROWS)
    }

    /// Encode with an explicit cell budget — the expression sprites use a
    /// tighter portrait budget than location art.
    pub fn encode_in(
        picker: &Picker,
        image: DynamicImage,
        max_cols: u16,
        max_rows: u16,
    ) -> Option<Self> {
        let protocol = SlicedProtocol::new_with_resize(
            picker,
            image,
            ratatui::layout::Size::new(max_cols, max_rows),
            ratatui_image::Resize::Fit(None),
        )
        .ok()?;
        let size = protocol.size();
        Some(Self {
            protocol,
            cols: size.width.max(1),
            rows: size.height.max(1),
        })
    }
}

/// One resolved image (location art or expression sprite): the encoded
/// payload plus the source path it came from. The path is the dedupe key —
/// transcript positions resolving to the same file must not repeat it.
#[derive(Clone, Debug)]
struct ResolvedImage {
    source: std::path::PathBuf,
    image: Arc<SceneImage>,
}

/// Filesystem work requested by the UI. The host owns all actual save I/O.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveLoadRequest {
    Open(SaveLoadMode),
    Save { slot_id: String },
    Load { slot_id: String },
}

#[derive(Clone, Debug)]
struct SaveLoadState {
    mode: SaveLoadMode,
    slots: Vec<SlotMeta>,
    selected: usize,
    overwrite_confirmation: bool,
}

/// The TUI's mutable, display-facing state. The engine-facing state lives in
/// the host; the App only renders the published view.
#[derive(Clone, Debug)]
pub struct App {
    /// The latest published view (what we draw).
    pub view: AppViewModel,
    /// Index of the highlighted choice.
    pub selected: usize,
    /// Set when the user requests quit (`q` / Ctrl+C).
    pub should_quit: bool,
    /// Vertical scroll offset for the narration pane.
    pub narration_scroll: u16,
    /// Latest layout-derived maximum narration offset. This lets input clamp
    /// a follow-end sentinel before moving upward.
    narration_max_scroll: Cell<u16>,
    last_render_area: Cell<Rect>,
    /// Whether custom free-text input is active.
    pub input_mode: bool,
    /// Grapheme-safe custom input buffer.
    pub input: InputBuffer,
    /// Currently open auxiliary list, if any.
    pub list_mode: Option<ListMode>,
    /// Display-ready history and memory rows supplied by the host.
    pub history_items: Vec<String>,
    /// Parallel to `history_items` (newest-first): entry ids for edit/delete.
    history_entry_ids: Vec<Uuid>,
    /// Parallel to `history_items`: full narration text for the edit modal.
    history_narrations: Vec<String>,
    /// Parallel to `history_items`: turn numbers for confirm copy.
    history_turns: Vec<u32>,
    /// Parallel to `history_items`: whether delete can restore `turn - 1`.
    history_deletable: Vec<bool>,
    /// Selected row while History list is open.
    history_selected: usize,
    /// Narration edit modal over History.
    history_edit: Option<HistoryEditDialog>,
    /// Delete-from-here confirm modal over History.
    history_delete_confirm: bool,
    pub memory_items: Vec<String>,
    /// Models advertised by the active OpenAI-compatible provider.
    pub model_items: Vec<String>,
    pub model_selected: usize,
    selected_model: Option<String>,
    /// Configured provider profiles (name + base_url hint) for the picker.
    pub provider_items: Vec<(String, String)>,
    pub provider_selected: usize,
    selected_provider: Option<String>,
    selected_director_model: Option<String>,
    selected_choice_model: Option<String>,
    selected_writer_model: Option<String>,
    model_reload_requested: bool,
    /// Set when the player pressed `v`; the host toggles narration TTS.
    tts_toggle_requested: bool,
    /// Terminal graphics capability detected at startup (None = images off).
    image_picker: Option<Picker>,
    /// Human-readable protocol name for the Settings screen (kitty 등).
    image_backend: Option<&'static str>,
    /// Whether the current protocol was player-forced (Settings `b` cycle)
    /// rather than the startup detection.
    image_backend_manual: bool,
    /// Player toggle from the Settings screen — rendering skips when off.
    images_enabled: bool,
    /// Set when the player toggled images in Settings (`i`).
    images_toggle_requested: bool,
    /// Set when the player cycled the image render protocol in Settings (`b`).
    image_backend_cycle_requested: bool,
    /// Set when the player toggled the generation path in Settings (`g`).
    generation_toggle_requested: bool,
    /// Set when the player toggled single-pass streaming render in Settings
    /// (`s`).
    stream_toggle_requested: bool,
    /// Player toggle for expression sprites from the Settings screen (`f`).
    expressions_enabled: bool,
    /// Set when the player toggled expressions in Settings (`f`).
    expressions_toggle_requested: bool,
    /// Set when the player cycled the sampling style preset in Settings (`x`).
    sampling_style_cycle_requested: bool,
    /// Persona edit modal (`a` in Settings); `Some` = modal active.
    persona_dialog: Option<PersonaDialogState>,
    /// Set when the player confirmed the persona modal with the raw
    /// `(name, persona)` text; the host persists the identity and re-applies
    /// it to the pack prompt.
    persona_submit_request: Option<(String, String)>,
    /// TOML-only display post-processing rules (`[[display_rules]]`),
    /// compiled by the host at startup. Applied to transcript text right
    /// before it becomes display lines — never to engine or save state.
    display_rules: Vec<(regex::Regex, String)>,
    /// Author's note modal (`t` in Settings); `Some` = modal active.
    author_note_dialog: Option<AuthorNoteDialogState>,
    /// Set when the player confirmed the note modal — the trimmed note text;
    /// empty string means "clear the note".
    author_note_submit_request: Option<String>,
    /// Current author's note, host-supplied, for the `t` modal prefill.
    author_note: String,
    /// Lorebook editor modal (`w` in Settings); `Some` = modal active.
    lorebook_dialog: Option<LorebookDialogState>,
    /// Working copy + persistence flag supplied via `set_lorebook_data`.
    /// `None` = nothing supplied yet, so Settings `w` is a no-op.
    lorebook_data: Option<LorebookData>,
    /// Set when a lorebook modal closed with edits: the full updated list
    /// for the host to apply to the engine (and to lorebook.toml, if any).
    lorebook_request: Option<Vec<story_model::LorebookEntry>>,
    /// Last-sent prompts captured by the host (session memory only).
    prompt_preview: Option<PromptPreview>,
    /// Prompt preview viewer (`r` in Settings); `Some` = open, holding the
    /// scroll offset.
    prompt_viewer: Option<u16>,
    /// Set when the player pressed story `w` (이어쓰기) while idle; the host
    /// issues the continue generation.
    continue_requested: bool,
    /// Set when the player pressed story `z` (대필) while idle; the host
    /// fetches a suggested action and feeds it back via `set_input_text`.
    suggest_action_requested: bool,
    /// Set when the player pressed story `g` (인물 대화) while idle; the host
    /// issues the group-scene side generation.
    group_scene_requested: bool,
    /// Set when the player pressed `[` (이전 변형) while idle; the host sends
    /// `EngineCommand::CycleTurnVariant { delta: -1 }`.
    cycle_variant_back_requested: bool,
    /// Set when the player pressed `]` (다음 변형) while idle; the host sends
    /// `EngineCommand::CycleTurnVariant { delta: 1 }`.
    cycle_variant_forward_requested: bool,
    /// Digit of an Alt+1..9 quick-action press (story screen, idle), "1".."9".
    /// The host fills the input buffer from the configured quick action.
    quick_action_request: Option<String>,
    /// Per-location resolved images, keyed by location id. `None` = the host
    /// resolved this location and found no usable art (or the encode failed) —
    /// an authoritative absence that is not retried until the host resets it
    /// on a pack switch.
    location_images: HashMap<String, Option<ResolvedImage>>,
    /// Per-emotion resolved expression sprites, keyed by [`Emotion`]. Same
    /// authoritative-absence contract as `location_images`.
    expression_images: HashMap<Emotion, Option<ResolvedImage>>,
    /// The pack's initial location, seeded by `set_lists` alongside the
    /// history it seeds the location fold for.
    initial_location: Option<String>,
    mouse_command: Option<EngineCommand>,
    /// Card import dialog (home menu "카드 가져오기"); `Some` = modal active.
    import_dialog: Option<ImportDialog>,
    /// Set when the player submits a card path; the host runs the import.
    import_request: Option<String>,
    /// Import file-browser navigation request. An empty string means "open
    /// at the host-chosen start directory"; otherwise the directory to list.
    browse_request: Option<String>,
    /// Set when the player confirms the report; the host swaps the pack in.
    pack_swap_request: Option<String>,
    /// Pack delete confirmation modal: the selected index into `pack_dirs`.
    pack_delete_confirm: Option<usize>,
    /// Set while the in-game `n` confirmation modal is up (AC: one stray
    /// keypress must not reset a run).
    new_game_confirm: bool,
    /// The confirm was opened from the home menu's "새 게임" row; on Enter the
    /// original home arm (greeting picker / home_request) replays instead of
    /// the in-game engine command.
    new_game_confirm_from_home: bool,
    /// Set when the player confirms deletion; the host validates and removes.
    pack_delete_request: Option<String>,
    /// Alternate openings for the current pack: index 0 is the pack opening.
    /// Empty = no picker; 새 게임 starts immediately.
    pub(crate) greetings: Vec<String>,
    /// Greeting picker modal (`Some` = visible).
    greeting_picker: Option<usize>,
    /// The greeting the player picked; the host applies it before starting.
    greeting_selection: Option<usize>,
    /// Chronological narration stack shown in the main story pane. Writes
    /// MUST go through [`Self::replace_story_stack`] (or `set_lists`) so the
    /// render cache is invalidated — the version field keys that cache.
    pub(crate) story_stack: Vec<StoryStackEntry>,
    /// Bumped on every `story_stack` replacement; keys the render cache.
    story_stack_version: u64,
    /// Per-entry composed lines for `story_stack_version` — building them
    /// means markdown-parsing the whole transcript, which must not happen per
    /// frame. The outer Vec indexes 1:1 with `story_stack`.
    stack_lines_cache: Option<(u64, Vec<Vec<Line<'static>>>)>,

    /// Ephemeral Writer text while the internal Director → Writer path is
    /// streaming. It is never saved and is replaced by the accepted turn.
    streaming_narration: Option<String>,
    pub list_scroll: u16,
    save_load: Option<SaveLoadState>,
    save_load_request: Option<SaveLoadRequest>,
    home_visible: bool,
    home_selected: usize,
    can_continue: bool,
    home_continue_returns_to_game: bool,
    home_request: Option<HomeRequest>,
    /// One-line start-menu status (guidance or notice) below the rows:
    /// `(line, attention)` — attention renders yellow, the rest gray.
    home_status: Option<(String, bool)>,
    /// One-line story-screen notice rendered as a subtle tail line of the
    /// narration blocks (same surface as the variant badge). Host-set for
    /// automation guides ("이전 세션을 이어요", "자동 재시도…(1/1)", "✎ 이어씀")
    /// — no new keys, no footer rows. Cleared when a new turn commits.
    story_notice: Option<String>,
    /// Directory of the active pack; the host updates it after a swap and
    /// reads it back to scan the pack root for the picker.
    current_pack_dir: String,
    /// `?` help overlay (`Some` story-screen context only).
    help_visible: bool,
    /// Directory of the most recent card import, used to prefill the next
    /// import dialog so repeat imports don't retype the path.
    import_last_dir: Option<String>,
    /// Pack picker modal (`Some` = visible): selected index into
    /// `pack_dirs`/`pack_labels`.
    pack_picker: Option<usize>,
    pack_dirs: Vec<String>,
    pack_labels: Vec<String>,
    /// One-line notice shown inside the pack picker (failed deletion);
    /// cleared by the next `open_pack_picker`.
    pack_notice: Option<String>,
    settings: Option<SettingsState>,
    settings_visible: bool,
    settings_opened: bool,
    settings_returns_home: bool,
    generation_models: BTreeMap<Uuid, String>,
    /// Wall-clock seconds each completed generation took, keyed by
    /// `generation_id`. Captured when a turn's loading indicator clears so the
    /// per-turn header can show "몇 분 만에 생성되었는지" after the fact.
    generation_durations: BTreeMap<Uuid, u64>,
    pub generation_start_time: Option<std::time::Instant>,
}

impl App {
    /// Build an App from the first published view.
    pub fn new(view: AppViewModel) -> Self {
        let loading = view.loading.is_some();
        Self {
            view,
            selected: 0,
            should_quit: false,
            narration_scroll: 0,
            narration_max_scroll: Cell::new(0),
            last_render_area: Cell::new(Rect::default()),
            input_mode: false,
            input: InputBuffer::new(500),
            list_mode: None,
            history_items: Vec::new(),
            history_entry_ids: Vec::new(),
            history_narrations: Vec::new(),
            history_turns: Vec::new(),
            history_deletable: Vec::new(),
            history_selected: 0,
            history_edit: None,
            history_delete_confirm: false,
            memory_items: Vec::new(),
            model_items: Vec::new(),
            model_selected: 0,
            selected_model: None,
            provider_items: Vec::new(),
            provider_selected: 0,
            selected_provider: None,
            selected_director_model: None,
            selected_choice_model: None,
            selected_writer_model: None,
            model_reload_requested: false,
            tts_toggle_requested: false,
            image_picker: None,
            image_backend: None,
            image_backend_manual: false,
            images_enabled: true,
            images_toggle_requested: false,
            image_backend_cycle_requested: false,
            generation_toggle_requested: false,
            stream_toggle_requested: false,
            expressions_enabled: true,
            expressions_toggle_requested: false,
            location_images: HashMap::new(),
            expression_images: HashMap::new(),
            initial_location: None,
            mouse_command: None,
            import_dialog: None,
            import_request: None,
            browse_request: None,
            pack_delete_confirm: None,
            new_game_confirm: false,
            new_game_confirm_from_home: false,
            pack_delete_request: None,
            pack_swap_request: None,
            sampling_style_cycle_requested: false,
            persona_dialog: None,
            persona_submit_request: None,
            display_rules: Vec::new(),
            author_note_dialog: None,
            author_note_submit_request: None,
            author_note: String::new(),
            lorebook_dialog: None,
            lorebook_data: None,
            lorebook_request: None,
            prompt_preview: None,
            prompt_viewer: None,
            continue_requested: false,
            suggest_action_requested: false,
            group_scene_requested: false,
            cycle_variant_back_requested: false,
            cycle_variant_forward_requested: false,
            quick_action_request: None,
            greetings: Vec::new(),
            greeting_picker: None,
            greeting_selection: None,
            story_stack: Vec::new(),
            story_stack_version: 0,
            stack_lines_cache: None,
            streaming_narration: None,
            list_scroll: 0,
            save_load: None,
            save_load_request: None,
            home_visible: false,
            home_selected: 0,
            can_continue: false,
            home_continue_returns_to_game: false,
            home_request: None,
            home_status: None,
            story_notice: None,
            current_pack_dir: String::new(),
            help_visible: false,
            import_last_dir: None,
            pack_picker: None,
            pack_dirs: Vec::new(),
            pack_labels: Vec::new(),
            pack_notice: None,
            settings: None,
            settings_visible: false,
            settings_opened: false,
            settings_returns_home: false,
            generation_models: BTreeMap::new(),
            generation_durations: BTreeMap::new(),
            generation_start_time: if loading {
                Some(std::time::Instant::now())
            } else {
                None
            },
        }
    }

    /// Supply the provider profiles for the picker and highlight the active
    /// one by name.
    pub fn set_providers(&mut self, providers: &[(String, String)], current: &str) {
        self.provider_items = providers.to_vec();
        self.provider_selected = providers
            .iter()
            .position(|(name, _)| name == current)
            .unwrap_or(0);
    }

    /// Consume a provider picked in the provider picker.
    pub fn take_selected_provider(&mut self) -> Option<String> {
        self.selected_provider.take()
    }

    pub fn set_models(&mut self, models: Vec<String>, current_model: &str) {
        self.model_items = models;
        self.model_selected = self
            .model_items
            .iter()
            .position(|id| id == current_model)
            .unwrap_or(0);
    }

    /// Replace the advertised models after a manual reload without needlessly
    /// moving the highlighted row when that model remains available.
    pub fn refresh_models(&mut self, models: Vec<String>, current_model: &str) {
        let highlighted = self.model_items.get(self.model_selected).cloned();
        self.model_items = models;
        self.model_selected = highlighted
            .as_deref()
            .and_then(|id| {
                self.model_items
                    .iter()
                    .position(|candidate| candidate == id)
            })
            .or_else(|| self.model_items.iter().position(|id| id == current_model))
            .unwrap_or(0);
    }

    /// Current role assignments for the model picker's badges. The live
    /// provider projection wins (the host applies picks asynchronously);
    /// pending in-App picks fill any gap so the badge updates the moment a
    /// key is pressed.
    fn model_role_hints(&self) -> ModelRoleHints {
        let provider = self.settings.as_ref().map(|settings| &settings.provider);
        ModelRoleHints {
            base: provider
                .map(|p| p.model.clone())
                .or_else(|| self.selected_model.clone()),
            director: provider
                .and_then(|p| p.director_model.clone())
                .or_else(|| self.selected_director_model.clone()),
            choice: provider
                .and_then(|p| p.choice_model.clone())
                .or_else(|| self.selected_choice_model.clone()),
            writer: provider
                .and_then(|p| p.writer_model.clone())
                .or_else(|| self.selected_writer_model.clone()),
        }
    }

    pub fn take_selected_model(&mut self) -> Option<String> {
        self.selected_model.take()
    }

    pub fn take_selected_director_model(&mut self) -> Option<String> {
        self.selected_director_model.take()
    }

    pub fn take_selected_writer_model(&mut self) -> Option<String> {
        self.selected_writer_model.take()
    }

    pub fn take_selected_choice_model(&mut self) -> Option<String> {
        self.selected_choice_model.take()
    }

    /// Consume a model-picker request to refresh the server list.
    pub fn take_model_reload_requested(&mut self) -> bool {
        std::mem::take(&mut self.model_reload_requested)
    }

    /// Whether the player asked to toggle narration TTS (`v`).
    pub fn take_tts_toggle_request(&mut self) -> bool {
        std::mem::take(&mut self.tts_toggle_requested)
    }

    /// Consume an engine command requested through a mouse click.
    pub fn take_mouse_command(&mut self) -> Option<EngineCommand> {
        self.mouse_command.take()
    }

    /// Open the card import dialog from the home menu. The typed path starts
    /// from the last import's directory so repeat imports stay a small edit.
    pub fn open_import_dialog(&mut self) {
        let mut buffer = InputBuffer::new(500);
        if let Some(dir) = &self.import_last_dir {
            buffer.insert_str(&format!("{dir}/"));
        }
        self.import_dialog = Some(ImportDialog::Input { buffer });
    }

    /// Consume a submitted card path; the host runs the import.
    pub fn take_import_request(&mut self) -> Option<String> {
        self.import_request.take()
    }

    /// Consume a confirmed pack directory; the host swaps it in.
    pub fn take_pack_swap_request(&mut self) -> Option<String> {
        self.pack_swap_request.take()
    }

    /// Supply the alternate-opening labels (index 0 = pack opening). Call at
    /// startup and after a pack swap.
    pub fn set_greetings(&mut self, greetings: Vec<String>) {
        self.greetings = greetings;
    }

    /// Consume the picked greeting index (`None` = default opening).
    pub fn take_greeting_selection(&mut self) -> Option<usize> {
        self.greeting_selection.take()
    }

    /// Set (or clear) the one-line status under the home menu rows.
    pub fn set_home_status(&mut self, status: Option<(String, bool)>) {
        self.home_status = status;
    }

    /// The home status line text, for host-side comparison (AC-4 recovery
    /// clears exactly the server-down/unconfigured notices it set).
    pub fn home_status_text(&self) -> Option<&str> {
        self.home_status.as_ref().map(|(line, _)| line.as_str())
    }

    /// The provider health last projected onto the Settings state (AC-4's
    /// periodic reprobe reads it to decide whether a probe can change
    /// anything). `None` before the first projection.
    pub fn provider_health(&self) -> Option<ProviderHealth> {
        self.settings.as_ref().map(|settings| settings.health)
    }

    /// Set (or clear) the one-line story notice (subtle tail row of the
    /// narration blocks — the variant-badge surface, no new keys). Cleared
    /// automatically when a later turn commits.
    pub fn set_story_notice(&mut self, notice: Option<&str>) {
        self.story_notice = notice.map(str::to_string);
    }

    /// The current story notice text (test/diagnostic accessor).
    pub fn story_notice(&self) -> Option<&str> {
        self.story_notice.as_deref()
    }

    /// The directory of the active pack (host bookkeeping for pack scans).
    pub fn current_pack_dir(&self) -> &str {
        &self.current_pack_dir
    }

    /// Record the active pack directory. Call at startup and after a swap.
    pub fn set_current_pack_dir(&mut self, dir: &str) {
        self.current_pack_dir = dir.to_string();
    }

    /// Open the pack picker. The host supplies scan results; an empty list
    /// still opens (the modal explains there is nothing to load). Labels
    /// matching `current_dir` get a " (현재)" suffix.
    pub fn open_pack_picker(&mut self, dirs: Vec<String>, labels: Vec<String>, current_dir: &str) {
        let mut labels = labels;
        for (index, (label, dir)) in labels.iter_mut().zip(dirs.iter()).enumerate() {
            if dir == current_dir {
                *label = format!("{label} (현재)");
                self.pack_picker = Some(index);
            }
        }
        self.pack_dirs = dirs;
        self.pack_labels = labels;
        self.pack_notice = None;
        if self.pack_picker.is_none() {
            self.pack_picker = Some(0);
        }
    }

    /// Show a notice inside the open pack picker (e.g. a failed deletion).
    /// Cleared by the next `open_pack_picker`.
    pub fn set_pack_picker_notice(&mut self, notice: Option<String>) {
        self.pack_notice = notice;
    }

    /// Keys while the pack picker owns the screen.
    fn handle_pack_picker_key(&mut self, key: KeyEvent) {
        let Some(selected) = self.pack_picker.as_mut() else {
            return;
        };
        let last = self.pack_dirs.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => self.pack_picker = None,
            KeyCode::Up | KeyCode::Char('k' | 'ㅏ') => {
                *selected = selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j' | 'ㅓ') => {
                *selected = (*selected + 1).min(last);
            }
            KeyCode::Enter => {
                if let Some(dir) = self.pack_dirs.get(*selected) {
                    let dir = dir.clone();
                    self.pack_picker = None;
                    self.pack_swap_request = Some(dir);
                }
            }
            KeyCode::Char('d' | 'ㅇ') => {
                // Deletion is confirmed in its own modal; the active pack
                // is never deletable.
                let selected_index = *selected;
                if let Some(dir) = self.pack_dirs.get(selected_index)
                    && dir != &self.current_pack_dir
                {
                    self.pack_delete_confirm = Some(selected_index);
                }
            }
            _ => {}
        }
    }

    /// Keys while the pack delete confirmation owns the screen.
    fn handle_pack_delete_confirm_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        let Some(index) = self.pack_delete_confirm else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.pack_delete_confirm = None,
            KeyCode::Enter => {
                if let Some(dir) = self.pack_dirs.get(index) {
                    let dir = dir.clone();
                    self.pack_delete_confirm = None;
                    self.pack_picker = None;
                    self.pack_delete_request = Some(dir);
                }
            }
            _ => {}
        }
    }

    /// Open the greeting picker (host already knows the pack has greetings).
    pub fn open_greeting_picker(&mut self) {
        if self.greetings.is_empty() {
            return;
        }
        self.greeting_picker = Some(0);
    }

    /// Keys while the greeting picker owns the screen.
    fn handle_greeting_picker_key(&mut self, key: KeyEvent) {
        let Some(selected) = self.greeting_picker.as_mut() else {
            return;
        };
        let last = self.greetings.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => self.greeting_picker = None,
            KeyCode::Up | KeyCode::Char('k' | 'ㅏ') => {
                *selected = selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j' | 'ㅓ') => {
                *selected = (*selected + 1).min(last);
            }
            KeyCode::Enter => {
                let picked = *selected;
                self.greeting_picker = None;
                self.greeting_selection = Some(picked);
                self.home_request = Some(HomeRequest::StartNew);
            }
            _ => {}
        }
    }

    /// Present a finished import (or its failure) in the dialog.
    pub fn show_import_report(&mut self, path: &str, lines: Vec<String>, ready: Option<String>) {
        self.import_dialog = Some(ImportDialog::Report {
            path: path.to_string(),
            lines,
            ready,
        });
    }

    /// The host supplies a directory listing for the import file browser.
    /// Stale listings (dialog closed meanwhile) are dropped. The parked
    /// path-entry buffer survives the refresh.
    pub fn open_file_browser(&mut self, dir: &str, entries: Vec<(String, bool)>) {
        let Some(ImportDialog::Browse { input, .. }) = self.import_dialog.take() else {
            return;
        };
        self.import_dialog = Some(ImportDialog::Browse {
            dir: dir.to_string(),
            entries,
            selected: 0,
            input,
        });
    }

    /// Consume a pending file-browser listing request. An empty string asks
    /// the host to pick the start directory (HOME or `.`).
    pub fn take_browse_request(&mut self) -> Option<String> {
        self.browse_request.take()
    }

    /// Consume a confirmed pack deletion. The host validates the guard and
    /// removes the directory.
    pub fn take_pack_delete_request(&mut self) -> Option<String> {
        self.pack_delete_request.take()
    }

    /// Keys while the import dialog owns the screen.
    fn handle_import_dialog_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        let Some(dialog) = self.import_dialog.as_mut() else {
            return;
        };
        match dialog {
            ImportDialog::Input { buffer } => match key.code {
                KeyCode::Esc => self.import_dialog = None,
                KeyCode::Enter => {
                    let path = buffer.text().trim().to_string();
                    if path.is_empty() {
                        return;
                    }
                    let parent_dir = parent_dir_of(&path);
                    self.import_request = Some(path.clone());
                    *dialog = ImportDialog::Running { path };
                    self.import_last_dir = parent_dir;
                }
                KeyCode::Tab => {
                    // Park the typed text and switch to the file browser;
                    // continue in the last import's directory when there is
                    // one (`import_last_dir` already holds the folder),
                    // otherwise the host opens at HOME ("" sentinel).
                    let buffer = buffer.clone();
                    let start = self.import_last_dir.clone().unwrap_or_default();
                    self.browse_request = Some(start);
                    *dialog = ImportDialog::Browse {
                        dir: String::new(),
                        entries: Vec::new(),
                        selected: 0,
                        input: buffer,
                    };
                }
                KeyCode::Backspace => buffer.backspace(),
                KeyCode::Delete => buffer.delete(),
                KeyCode::Left => buffer.cursor_left(),
                KeyCode::Right => buffer.cursor_right(),
                KeyCode::Home => buffer.cursor_start(),
                KeyCode::End => buffer.cursor_end(),
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    buffer.insert_grapheme(&c.to_string());
                }
                _ => {}
            },
            ImportDialog::Browse {
                dir,
                entries,
                selected,
                input,
            } => match key.code {
                KeyCode::Esc | KeyCode::Tab => {
                    // Back to path entry; the typed buffer is restored.
                    let buffer = input.clone();
                    *dialog = ImportDialog::Input { buffer };
                }
                KeyCode::Up | KeyCode::Char('k' | 'ㅏ') => {
                    *selected = selected.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j' | 'ㅓ') => {
                    *selected = (*selected + 1).min(entries.len().saturating_sub(1));
                }
                KeyCode::Enter => {
                    let Some((name, is_dir)) = entries.get(*selected) else {
                        return;
                    };
                    let path = join_browse_path(dir, name);
                    if *is_dir {
                        self.browse_request = Some(path);
                    } else {
                        let parent_dir = parent_dir_of(&path);
                        self.import_request = Some(path.clone());
                        *dialog = ImportDialog::Running { path };
                        self.import_last_dir = parent_dir;
                    }
                }
                KeyCode::Backspace | KeyCode::Left => {
                    if let Some(parent) = std::path::Path::new(dir).parent()
                        && parent.display().to_string() != *dir
                        && !dir.is_empty()
                    {
                        self.browse_request = Some(parent.display().to_string());
                    }
                }
                _ => {}
            },
            ImportDialog::Running { .. } => {}
            ImportDialog::Report { ready, .. } => match key.code {
                KeyCode::Esc => self.import_dialog = None,
                KeyCode::Enter if ready.is_some() => {
                    if let Some(pack_dir) = ready.clone() {
                        self.pack_swap_request = Some(pack_dir);
                        self.import_dialog = None;
                    }
                }
                _ => {}
            },
        }
    }

    /// Open the persona editor prefilled with the effective identity the
    /// Settings screen already shows (empty field = off).
    fn open_persona_dialog(&mut self) {
        let Some(settings) = self.settings.as_ref() else {
            return;
        };
        let mut name = InputBuffer::new(PERSONA_NAME_MAX_CHARS);
        let mut persona = InputBuffer::new(PERSONA_TEXT_MAX_CHARS);
        name.insert_str(&settings.persona_name);
        persona.insert_str(&settings.persona_text);
        self.persona_dialog = Some(PersonaDialogState {
            name,
            persona,
            focus: 0,
        });
    }

    /// Keys while the persona modal owns the Settings screen. Tab toggles the
    /// focused field, Enter submits the raw text (empty = off), Esc cancels.
    fn handle_persona_dialog_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        let Some(dialog) = self.persona_dialog.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.persona_dialog = None,
            KeyCode::Enter => {
                let name = dialog.name.text().trim().to_string();
                let persona = dialog.persona.text().trim().to_string();
                self.persona_dialog = None;
                self.persona_submit_request = Some((name, persona));
            }
            KeyCode::Tab | KeyCode::Up | KeyCode::Down => {
                dialog.focus = 1 - dialog.focus;
            }
            KeyCode::Backspace
            | KeyCode::Delete
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Home
            | KeyCode::End => {
                let focused = match dialog.focus {
                    0 => &mut dialog.name,
                    _ => &mut dialog.persona,
                };
                match key.code {
                    KeyCode::Backspace => focused.backspace(),
                    KeyCode::Delete => focused.delete(),
                    KeyCode::Left => focused.cursor_left(),
                    KeyCode::Right => focused.cursor_right(),
                    KeyCode::Home => focused.cursor_start(),
                    _ => focused.cursor_end(),
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                let focused = match dialog.focus {
                    0 => &mut dialog.name,
                    _ => &mut dialog.persona,
                };
                focused.insert_grapheme(&c.to_string());
            }
            _ => {}
        }
    }

    /// Open the author's note editor prefilled with the host-supplied note
    /// (empty = the note is off).
    fn open_author_note_dialog(&mut self) {
        let mut note = InputBuffer::new(AUTHOR_NOTE_MAX_CHARS);
        note.insert_str(&self.author_note);
        self.author_note_dialog = Some(AuthorNoteDialogState { note });
    }

    /// Prefill the History narration editor from the selected row.
    fn open_history_edit(&mut self) {
        let Some(entry_id) = self.history_entry_ids.get(self.history_selected).copied() else {
            return;
        };
        let Some(turn) = self.history_turns.get(self.history_selected).copied() else {
            return;
        };
        let narration = self
            .history_narrations
            .get(self.history_selected)
            .cloned()
            .unwrap_or_default();
        let mut buffer = InputBuffer::new_multiline(HISTORY_NARRATION_MAX_CHARS);
        buffer.insert_str(&narration);
        self.history_edit = Some(HistoryEditDialog {
            entry_id,
            turn,
            buffer,
        });
    }

    /// Delete is offered only when the host reported a ring checkpoint for
    /// `turn - 1` (and never for the opening turn).
    fn history_delete_available(&self) -> bool {
        self.history_deletable
            .get(self.history_selected)
            .copied()
            .unwrap_or(false)
    }

    fn ensure_history_selection_visible(&mut self) {
        let selected = self.history_selected as u16;
        if selected < self.list_scroll {
            self.list_scroll = selected;
        } else if selected >= self.list_scroll.saturating_add(8) {
            self.list_scroll = selected.saturating_sub(7);
        }
    }

    fn handle_history_edit_key(&mut self, key: KeyEvent) -> Option<EngineCommand> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return None;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
            let narration = self.history_edit.as_ref()?.buffer.text();
            if narration.trim().is_empty() {
                self.set_story_notice(Some("내레이션이 비어 있어 적용하지 않았습니다"));
                return None;
            }
            let dialog = self.history_edit.take()?;
            return Some(EngineCommand::EditHistoryNarration {
                entry_id: dialog.entry_id,
                narration,
            });
        }
        let dialog = self.history_edit.as_mut()?;
        match key.code {
            KeyCode::Esc => self.history_edit = None,
            KeyCode::Enter => dialog.buffer.insert_grapheme("\n"),
            KeyCode::Backspace => dialog.buffer.backspace(),
            KeyCode::Delete => dialog.buffer.delete(),
            KeyCode::Left => dialog.buffer.cursor_left(),
            KeyCode::Right => dialog.buffer.cursor_right(),
            KeyCode::Home => dialog.buffer.cursor_start(),
            KeyCode::End => dialog.buffer.cursor_end(),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                dialog.buffer.insert_grapheme(&c.to_string());
            }
            _ => {}
        }
        None
    }

    fn handle_history_delete_confirm_key(&mut self, key: KeyEvent) -> Option<EngineCommand> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return None;
        }
        match key.code {
            KeyCode::Enter => {
                self.history_delete_confirm = false;
                let entry_id = self.history_entry_ids.get(self.history_selected).copied()?;
                Some(EngineCommand::DeleteHistoryFrom { entry_id })
            }
            KeyCode::Esc | KeyCode::Char('d' | 'ㅇ') => {
                self.history_delete_confirm = false;
                None
            }
            KeyCode::Char('q' | 'ㅂ') => {
                self.should_quit = true;
                self.history_delete_confirm = false;
                None
            }
            _ => None,
        }
    }

    /// Keys while the author's note modal owns the Settings screen. Enter
    /// submits the trimmed text (empty = clear the note), Esc cancels.
    fn handle_author_note_dialog_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        let Some(dialog) = self.author_note_dialog.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.author_note_dialog = None,
            KeyCode::Enter => {
                let note = dialog.note.text().trim().to_string();
                self.author_note_dialog = None;
                self.author_note_submit_request = Some(note);
            }
            KeyCode::Backspace => dialog.note.backspace(),
            KeyCode::Delete => dialog.note.delete(),
            KeyCode::Left => dialog.note.cursor_left(),
            KeyCode::Right => dialog.note.cursor_right(),
            KeyCode::Home => dialog.note.cursor_start(),
            KeyCode::End => dialog.note.cursor_end(),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                dialog.note.insert_grapheme(&c.to_string());
            }
            _ => {}
        }
    }

    /// Open the lorebook editor from the host's working copy. Requires
    /// `set_lorebook_data` first — without it the key is a no-op.
    fn open_lorebook_dialog(&mut self) {
        let Some(data) = self.lorebook_data.as_ref() else {
            return;
        };
        self.lorebook_dialog = Some(LorebookDialogState {
            entries: data.entries.clone(),
            selected: 0,
            persisted: data.persisted,
            dirty: false,
            edit: None,
        });
    }

    /// Keys while the lorebook modal owns the Settings screen. Two levels:
    /// the entry list (Up/Down move, Space toggles, Enter edits content, `k`
    /// edits keys) and a single-line editor absorbed by its buffer. Esc backs
    /// out one level; leaving the list while dirty hands the full list back.
    fn handle_lorebook_dialog_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        // Taken out so the editor level can re-borrow `entries` freely; the
        // Esc arm leaves it out (closes the dialog), everything else puts it
        // back.
        let mut dialog = match self.lorebook_dialog.take() {
            Some(dialog) => dialog,
            None => return,
        };
        if let Some(edit) = dialog.edit.as_mut() {
            let (LorebookEdit::Content { buffer } | LorebookEdit::Keys { buffer }) = edit;
            match key.code {
                KeyCode::Esc => dialog.edit = None,
                KeyCode::Enter => {
                    let text = buffer.text();
                    if let Some(entry) = dialog.entries.get_mut(dialog.selected) {
                        match edit {
                            LorebookEdit::Content { .. } => entry.content = text,
                            LorebookEdit::Keys { .. } => {
                                entry.keys = parse_lorebook_keys(&text);
                            }
                        }
                    }
                    dialog.dirty = true;
                    dialog.edit = None;
                }
                KeyCode::Backspace => buffer.backspace(),
                KeyCode::Delete => buffer.delete(),
                KeyCode::Left => buffer.cursor_left(),
                KeyCode::Right => buffer.cursor_right(),
                KeyCode::Home => buffer.cursor_start(),
                KeyCode::End => buffer.cursor_end(),
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    buffer.insert_grapheme(&c.to_string());
                }
                _ => {}
            }
            self.lorebook_dialog = Some(dialog);
            return;
        }
        match key.code {
            KeyCode::Esc => {
                if dialog.dirty {
                    self.lorebook_request = Some(dialog.entries);
                }
                // Already taken out — the dialog stays closed.
                return;
            }
            KeyCode::Up => dialog.selected = dialog.selected.saturating_sub(1),
            KeyCode::Down => {
                dialog.selected = (dialog.selected + 1).min(dialog.entries.len().saturating_sub(1));
            }
            KeyCode::Char(' ') => {
                if let Some(entry) = dialog.entries.get_mut(dialog.selected) {
                    entry.enabled = !entry.enabled;
                    dialog.dirty = true;
                }
            }
            KeyCode::Enter => {
                if let Some(entry) = dialog.entries.get(dialog.selected) {
                    let mut buffer = InputBuffer::new(LOREBOOK_CONTENT_MAX_CHARS);
                    buffer.insert_str(&entry.content);
                    dialog.edit = Some(LorebookEdit::Content { buffer });
                }
            }
            // Same physical key under Korean 2-beolsik: k=ㅏ.
            KeyCode::Char('k' | 'ㅏ') => {
                if let Some(entry) = dialog.entries.get(dialog.selected) {
                    let mut buffer = InputBuffer::new(LOREBOOK_KEYS_MAX_CHARS);
                    buffer.insert_str(&join_lorebook_keys(&entry.keys));
                    dialog.edit = Some(LorebookEdit::Keys { buffer });
                }
            }
            // Same physical key under Korean 2-beolsik: c=ㅊ. Toggling the
            // constant pin edits the entry like any other — dirty, so the
            // same full-list request hands it over on close.
            KeyCode::Char('c' | 'ㅊ') => {
                if let Some(entry) = dialog.entries.get_mut(dialog.selected) {
                    entry.constant = !entry.constant;
                    dialog.dirty = true;
                }
            }
            // Same physical key under Korean 2-beolsik: p=ㅔ. Probability
            // cycles the ST-style preset ladder 100→75→50→25→100; a value
            // off the ladder joins it at 75.
            KeyCode::Char('p' | 'ㅔ') => {
                if let Some(entry) = dialog.entries.get_mut(dialog.selected) {
                    entry.probability = match entry.probability {
                        75 => 50,
                        50 => 25,
                        25 => 100,
                        _ => 75,
                    };
                    dialog.dirty = true;
                }
            }
            _ => {}
        }
        self.lorebook_dialog = Some(dialog);
    }

    /// Open the prompt preview viewer. Session memory only: without a
    /// captured prompt the key is a no-op.
    fn open_prompt_viewer(&mut self) {
        if self.prompt_preview.is_some() {
            self.prompt_viewer = Some(0);
        }
    }

    /// Scroll the prompt viewer by `delta` lines, clamped to the body the
    /// renderer will draw for the captured prompts. The Paragraph wraps text
    /// BEFORE applying the scroll offset, so the clamp counts wrapped visual
    /// rows at the popup's inner width — never raw logical lines.
    fn scroll_prompt_viewer(&mut self, delta: i32) {
        let Some(scroll) = self.prompt_viewer.as_mut() else {
            return;
        };
        let area = self.last_render_area.get();
        let max = self
            .prompt_preview
            .as_ref()
            .map(|preview| {
                let (width, height) = crate::screens::settings::prompt_viewer_frame(area);
                let lines = crate::screens::settings::prompt_viewer_body_lines(
                    &preview.system,
                    &preview.user,
                );
                let rows =
                    crate::screens::story::wrapped_line_count(&lines, width.saturating_sub(2));
                rows.saturating_sub(usize::from(height.saturating_sub(2)))
            })
            .unwrap_or(0);
        *scroll = (*scroll as i32 + delta).clamp(0, max as i32) as u16;
    }

    /// Supply the redacted configuration and health details for the Settings
    /// screen. Secrets never enter this UI state.
    #[allow(clippy::too_many_arguments)] // the projection stays explicit at the host boundary
    pub fn set_settings(
        &mut self,
        provider: ProviderConfigSafe,
        health: ProviderHealth,
        temperature: f32,
        top_p: f32,
        top_k: u32,
        max_tokens: u32,
        tts: TtsDisplay,
        images_enabled: bool,
        image_backend: Option<String>,
        use_director_writer: bool,
        stream_turns: bool,
        stream_supported: bool,
        expressions_enabled: bool,
        sampling_style: String,
        persona_label: String,
        persona_name: String,
        persona_text: String,
    ) {
        self.settings = Some(SettingsState {
            provider,
            health,
            temperature,
            top_p,
            top_k,
            max_tokens,
            tts,
            images_enabled,
            image_backend,
            image_backend_manual: self.image_backend_manual,
            use_director_writer,
            stream_turns,
            stream_supported,
            expressions_enabled,
            sampling_style,
            persona_label,
            persona_name,
            persona_text,
        });
    }

    /// Record the terminal graphics capability. Call once at startup before
    /// the first `set_location_image`. `manual` marks a player-forced
    /// protocol (Settings `b` cycle) as opposed to the detected default.
    pub fn set_image_picker(&mut self, picker: Option<Picker>, manual: bool) {
        self.image_backend = picker.as_ref().map(|picker| match picker.protocol_type() {
            ratatui_image::picker::ProtocolType::Kitty => "kitty",
            ratatui_image::picker::ProtocolType::Sixel => "sixel",
            ratatui_image::picker::ProtocolType::Iterm2 => "iterm2",
            ratatui_image::picker::ProtocolType::Halfblocks => "halfblocks",
        });
        self.image_backend_manual = manual;
        self.image_picker = picker;
    }

    /// Settings-screen image toggle. The per-location images are kept, so
    /// re-enabling is instant — no re-decode.
    pub fn set_images_enabled(&mut self, enabled: bool) {
        self.images_enabled = enabled;
    }

    pub fn images_enabled(&self) -> bool {
        self.images_enabled
    }

    /// Protocol name for display (`kitty` / `sixel` / `halfblocks` / …).
    pub fn image_backend_label(&self) -> Option<String> {
        self.image_backend.map(str::to_string)
    }

    /// Whether the displayed protocol was player-forced rather than detected.
    pub fn image_backend_manual(&self) -> bool {
        self.image_backend_manual
    }

    /// Consume an image toggle request from the Settings screen.
    pub fn take_images_toggle_request(&mut self) -> bool {
        std::mem::take(&mut self.images_toggle_requested)
    }

    /// Consume an image render-protocol cycle request from the Settings screen.
    pub fn take_image_backend_cycle_request(&mut self) -> bool {
        std::mem::take(&mut self.image_backend_cycle_requested)
    }

    /// Consume a generation-path toggle request from the Settings screen.
    pub fn take_generation_toggle_request(&mut self) -> bool {
        std::mem::take(&mut self.generation_toggle_requested)
    }

    /// Consume a single-pass streaming-render toggle request from the
    /// Settings screen.
    pub fn take_stream_toggle_request(&mut self) -> bool {
        std::mem::take(&mut self.stream_toggle_requested)
    }

    /// Store one location's resolved image. Decoding is host-owned; encoding
    /// happens here once so per-frame rendering stays cheap. `None` (no usable
    /// art) and encode failures are both stored as authoritative absence — the
    /// host does not re-resolve this location until the next pack switch.
    /// `source` is the dedupe key shared by locations that resolve to the
    /// same file (portrait fallback).
    pub fn set_location_image(
        &mut self,
        location: &str,
        resolved: Option<(std::path::PathBuf, DynamicImage)>,
    ) {
        let entry = match (resolved, self.image_picker.as_ref()) {
            (Some((source, image)), Some(picker)) => {
                SceneImage::encode(picker, image).map(|image| ResolvedImage {
                    source,
                    image: Arc::new(image),
                })
            }
            _ => None,
        };
        self.location_images.insert(location.to_string(), entry);
    }

    /// Store one emotion's resolved expression sprite. Mirrors
    /// [`Self::set_location_image`] with the tighter portrait cell budget.
    pub fn set_expression_image(
        &mut self,
        emotion: Emotion,
        resolved: Option<(std::path::PathBuf, DynamicImage)>,
    ) {
        let entry = match (resolved, self.image_picker.as_ref()) {
            (Some((source, image)), Some(picker)) => {
                SceneImage::encode_in(picker, image, EMOTE_IMAGE_MAX_COLS, EMOTE_IMAGE_MAX_ROWS)
                    .map(|image| ResolvedImage {
                        source,
                        image: Arc::new(image),
                    })
            }
            _ => None,
        };
        self.expression_images.insert(emotion, entry);
    }

    /// Drop every per-location image. MUST move in lockstep with the host's
    /// `LocationImageStore::clear` — clearing only one side lets the previous
    /// pack's images survive a pack switch (the stale-portrait bug).
    pub fn reset_location_images(&mut self) {
        self.location_images.clear();
    }

    /// Drop every per-emotion sprite. Same lockstep contract as
    /// [`Self::reset_location_images`] (`reset_scene_images` on the host).
    pub fn reset_expression_images(&mut self) {
        self.expression_images.clear();
    }

    /// Settings-screen expression toggle. Per-emotion sprites are kept, so
    /// re-enabling is instant — no re-decode.
    pub fn set_expressions_enabled(&mut self, enabled: bool) {
        self.expressions_enabled = enabled;
    }

    pub fn expressions_enabled(&self) -> bool {
        self.expressions_enabled
    }

    /// Resolved expression sprite entries (diagnostics seam — lets the host
    /// assert the cold-toggle refresh path without rendering).
    pub fn expression_sprite_count(&self) -> usize {
        self.expression_images.len()
    }

    /// Consume an expression toggle request from the Settings screen.
    pub fn take_expressions_toggle_request(&mut self) -> bool {
        std::mem::take(&mut self.expressions_toggle_requested)
    }

    /// Consume a sampling style cycle request from the Settings screen.
    pub fn take_sampling_style_cycle_request(&mut self) -> bool {
        std::mem::take(&mut self.sampling_style_cycle_requested)
    }

    /// Consume a confirmed persona modal submission — the raw `(name,
    /// persona)` text. Empty means "this field is off".
    pub fn take_persona_submit_request(&mut self) -> Option<(String, String)> {
        self.persona_submit_request.take()
    }

    /// Supply the current author's note for the Settings `t` modal prefill.
    /// Empty string = the note is off.
    pub fn set_author_note(&mut self, note: &str) {
        self.author_note = note.to_string();
    }

    /// Consume a confirmed author's note submission — the trimmed note text.
    /// An empty string means "clear the note".
    pub fn take_author_note_submit(&mut self) -> Option<String> {
        self.author_note_submit_request.take()
    }

    /// Supply the lorebook working copy for the Settings `w` editor. The
    /// dialog clones this on open, so later pushes never clobber open edits.
    /// `persisted == false` marks the edits session-only (no lorebook.toml).
    pub fn set_lorebook_data(&mut self, entries: &[story_model::LorebookEntry], persisted: bool) {
        self.lorebook_data = Some(LorebookData {
            entries: entries.to_vec(),
            persisted,
        });
    }

    /// Consume the lorebook edit result: the full updated entry list, handed
    /// over once when a dialog with edits closes. The host applies it to the
    /// engine and (when present) to lorebook.toml.
    pub fn take_lorebook_request(&mut self) -> Option<Vec<story_model::LorebookEntry>> {
        // While a turn is generating the engine drops the hot-swap (identity
        // reduce during pending) — hold the edit until it settles so the
        // modal, the engine, and lorebook.toml cannot diverge.
        if self.view.loading.is_some() {
            return None;
        }
        self.lorebook_request.take()
    }

    /// Capture the last-sent prompts for the Settings `r` viewer. Session
    /// memory only — nothing here is ever persisted.
    pub fn set_prompt_preview(&mut self, system: &str, user: &str) {
        self.prompt_preview = Some(PromptPreview {
            system: system.to_string(),
            user: user.to_string(),
        });
    }

    /// Consume a story-screen `w` (이어쓰기) request.
    pub fn take_continue_request(&mut self) -> bool {
        std::mem::take(&mut self.continue_requested)
    }

    /// Consume a story-screen `z` (대필) request.
    pub fn take_suggest_action_request(&mut self) -> bool {
        std::mem::take(&mut self.suggest_action_requested)
    }

    /// Consume a story-screen `g` (인물 대화) request.
    pub fn take_group_scene_request(&mut self) -> bool {
        std::mem::take(&mut self.group_scene_requested)
    }

    /// Cast roster rows for the `p` overlay: `이름 · 관계단계 · 감정 ·
    /// 호감/신뢰/긴장`. Pure display formatting over the engine-published
    /// [`story_model::CharacterStatusView`]s — the engine view stays the
    /// authority for the underlying numbers.
    fn cast_roster_rows(&self) -> Vec<String> {
        self.view
            .cast
            .iter()
            .map(|member| {
                format!(
                    "{} · {} · {} · 호감 {} / 신뢰 {} / 긴장 {}",
                    member.name,
                    relationship_stage_label(member.relationship_stage),
                    emotion_label(member.current_emotion),
                    member.affection,
                    member.trust,
                    member.tension,
                )
            })
            .collect()
    }

    /// Build the [`ParsedAction`] for a submitted free input. Text starting
    /// with `@` pins the LONGEST cast member whose name follows (matched
    /// case-insensitively, so latin names hit regardless of case) as the
    /// `target`; the mention is stripped from `content` while
    /// `original_text` stays verbatim. No `@` match keeps today's shape —
    /// same as before the cast roster existed.
    fn parsed_free_action(&self, text: &str) -> ParsedAction {
        let trimmed = text.trim();
        let mention = trimmed
            .strip_prefix('@')
            .and_then(|rest| self.mention_target(rest))
            .filter(|(_, content)| !content.is_empty());
        if let Some((member, content)) = mention {
            return ParsedAction {
                action_type: ActionType::GeneralAction,
                target: Some(member.id.clone()),
                intent: None,
                tone: None,
                content,
                original_text: text.to_string(),
                confidence: Some(0.0),
            };
        }
        ParsedAction {
            action_type: ActionType::GeneralAction,
            target: None,
            intent: None,
            tone: None,
            content: trimmed.to_string(),
            original_text: text.to_string(),
            confidence: Some(0.0),
        }
    }

    /// Longest cast member whose name matches `rest` right after the `@`
    /// (`(member, rest-after-name)`), or `None`. Longest wins so `한유나`
    /// beats a shorter `유나`; case-insensitive on the latin part. A `:`
    /// right after the name ("@유나: 안녕") is consumed, and an empty
    /// remainder means no target — a bare `@이름` stays plain text.
    fn mention_target(&self, rest: &str) -> Option<(&story_model::CharacterStatusView, String)> {
        self.view
            .cast
            .iter()
            .filter(|member| !member.name.is_empty())
            .filter(|member| starts_with_case_insensitive(rest, &member.name))
            .max_by_key(|member| member.name.chars().count())
            .map(|member| {
                let consumed = member.name.chars().count();
                let remainder = rest.chars().skip(consumed).collect::<String>();
                let remainder = remainder.trim_start();
                let remainder = remainder.strip_prefix(':').unwrap_or(remainder);
                (member, remainder.trim().to_string())
            })
    }

    /// Consume a story-screen `[` (이전 변형) request — the host dispatches
    /// `EngineCommand::CycleTurnVariant { delta: -1 }`.
    pub fn take_cycle_variant_back_request(&mut self) -> bool {
        std::mem::take(&mut self.cycle_variant_back_requested)
    }

    /// Consume a story-screen `]` (다음 변형) request — the host dispatches
    /// `EngineCommand::CycleTurnVariant { delta: 1 }`.
    pub fn take_cycle_variant_forward_request(&mut self) -> bool {
        std::mem::take(&mut self.cycle_variant_forward_requested)
    }

    /// Consume an Alt+1..9 quick-action request — the pressed digit, "1".."9".
    pub fn take_quick_action_request(&mut self) -> Option<String> {
        self.quick_action_request.take()
    }

    /// Fill the free-input buffer with `text` and open input mode. The host
    /// calls this for suggestion ("대필") and quick-action fills; the player
    /// edits and sends with their own Enter — this never submits.
    pub fn set_input_text(&mut self, text: &str) {
        self.input.clear();
        self.input.insert_str(text);
        // Opening the editor under an open screen or overlay would spring it
        // on the player the moment they close it — surface it only on the
        // bare story screen; the text stays in the buffer either way.
        self.input_mode = !self.settings_visible
            && !self.help_visible
            && !self.home_visible
            && self.list_mode.is_none()
            && self.import_dialog.is_none()
            && self.save_load.is_none();
    }

    /// Patch the sampling block after a style cycle: the new preset label and
    /// the effective triple now in force.
    pub fn update_settings_sampling(
        &mut self,
        sampling_style: String,
        temperature: f32,
        top_p: f32,
        top_k: u32,
    ) {
        if let Some(settings) = self.settings.as_mut() {
            settings.sampling_style = sampling_style;
            settings.temperature = temperature;
            settings.top_p = top_p;
            settings.top_k = top_k;
        }
    }

    /// Patch the persona row and the modal prefill after a confirmed edit.
    pub fn update_settings_persona(&mut self, label: String, name: String, persona: String) {
        if let Some(settings) = self.settings.as_mut() {
            settings.persona_label = label;
            settings.persona_name = name;
            settings.persona_text = persona;
        }
    }

    /// Patch only the expression section of the Settings state (the `f`
    /// toggle).
    pub fn update_settings_expressions(&mut self, expressions_enabled: bool) {
        if let Some(settings) = self.settings.as_mut() {
            settings.expressions_enabled = expressions_enabled;
        }
    }

    /// Patch only the TTS section of the Settings state (used by the `v`
    /// toggle without a full provider re-projection).
    pub fn update_settings_tts(&mut self, tts: TtsDisplay) {
        if let Some(settings) = self.settings.as_mut() {
            settings.tts = tts;
        }
    }

    /// Patch only the image section of the Settings state (the `i` toggle
    /// and the `b` protocol cycle).
    pub fn update_settings_images(&mut self, images_enabled: bool, image_backend: Option<String>) {
        if let Some(settings) = self.settings.as_mut() {
            settings.images_enabled = images_enabled;
            settings.image_backend = image_backend;
            settings.image_backend_manual = self.image_backend_manual;
        }
    }

    /// Patch only the generation-path section of the Settings state (the `g`
    /// toggle).
    pub fn update_settings_generation(&mut self, use_director_writer: bool) {
        if let Some(settings) = self.settings.as_mut() {
            settings.use_director_writer = use_director_writer;
        }
    }

    /// Generation-path value currently shown on the Settings screen. The
    /// host's `config.use_director_writer` stays the authoritative session
    /// value; this mirror lets settings refreshes read it without threading
    /// the runtime config through every `apply_*` path.
    pub fn use_director_writer(&self) -> bool {
        self.settings
            .as_ref()
            .is_some_and(|settings| settings.use_director_writer)
    }

    /// Patch only the single-pass streaming-render section of the Settings
    /// state (the `s` toggle).
    pub fn update_settings_stream(&mut self, stream_turns: bool) {
        if let Some(settings) = self.settings.as_mut() {
            settings.stream_turns = stream_turns;
        }
    }

    /// Single-pass streaming value currently shown on the Settings screen.
    /// Same mirror contract as [`App::use_director_writer`]: the host's
    /// `config.provider.stream_turns` stays the authoritative session value.
    pub fn stream_turns(&self) -> bool {
        self.settings
            .as_ref()
            .is_some_and(|settings| settings.stream_turns)
    }

    pub fn set_can_continue(&mut self, can_continue: bool) {
        self.can_continue = can_continue;
        if can_continue {
            // AC-2: a returning player (autosave present) opens the launcher
            // on the 이어하기 row — 새 게임 stays an explicit choice.
            self.home_selected = 1;
        } else if self.home_selected == 1 {
            self.home_selected = 0;
        }
    }

    /// Show the launcher. Kept explicit so headless Story-screen consumers
    /// can still construct an `App` directly without a launcher transition.
    pub fn show_home(&mut self) {
        self.home_visible = true;
        self.home_continue_returns_to_game = false;
    }

    /// Whether the launcher currently owns the screen (AC-1 evidence: the
    /// auto-resume startup path skips it entirely).
    pub fn home_visible(&self) -> bool {
        self.home_visible
    }

    /// Whether a resumable autosave exists (drives the launcher's 이어하기
    /// row and its default cursor).
    pub fn can_continue(&self) -> bool {
        self.can_continue
    }

    /// Return to the launcher without discarding the current in-memory game.
    pub fn open_home_from_game(&mut self) {
        self.home_visible = true;
        self.can_continue = true;
        self.home_continue_returns_to_game = true;
        self.home_selected = 1;
    }

    /// Leave the launcher and reveal the already-published story view.
    pub fn close_home(&mut self) {
        self.home_visible = false;
    }

    /// Consume the pending launcher action for the host to fulfill.
    pub fn take_home_request(&mut self) -> Option<HomeRequest> {
        self.home_request.take()
    }

    /// Returns true once for each entry into Settings, so the host can refresh
    /// the provider's model list without putting HTTP in the UI crate.
    pub fn take_settings_opened(&mut self) -> bool {
        std::mem::take(&mut self.settings_opened)
    }

    /// Associate a completed response with the provider model that produced it.
    /// Existing saves lack this runtime-only provenance and remain unlabeled.
    pub fn record_generation_model(&mut self, generation_id: Uuid, model: String) {
        self.generation_models.insert(generation_id, model);
    }

    /// Record how long a just-completed generation took, keyed by
    /// `generation_id`. Called by the host on the success path BEFORE the
    /// commit is dispatched, so the per-turn header (built in [`Self::set_lists`])
    /// already sees the value. Reads the live stopwatch; safe to call once per
    /// turn (the single-pass `Generation` and the 2-stage `Writer` finish).
    pub fn record_generation_duration(&mut self, generation_id: Uuid) {
        if let Some(start) = self.generation_start_time {
            self.generation_durations
                .insert(generation_id, start.elapsed().as_secs());
        }
    }

    pub fn generation_elapsed_seconds(&self) -> Option<u64> {
        self.generation_start_time
            .map(|start| start.elapsed().as_secs())
    }

    /// Replace the view (e.g. the host published a new one). Clamps the
    /// selection to the new choice count.
    pub fn set_view(&mut self, mut view: AppViewModel) {
        let previous_turn = self.view.status.turn;
        let count = view.choices.len();
        if count == 0 {
            self.selected = 0;
        } else if self.selected >= count {
            self.selected = count - 1;
        }
        // Display rules also cover the choice surface — a throwaway rewrite
        // of the incoming view copy; the engine view stays authoritative.
        for choice in &mut view.choices {
            choice.label = self.display_text(&choice.label).into_owned();
            if let Some(preview) = &choice.preview {
                choice.preview = Some(self.display_text(preview).into_owned());
            }
        }
        self.view = view;
        if self.view.loading.is_some() {
            if self.generation_start_time.is_none() {
                self.generation_start_time = Some(std::time::Instant::now());
            }
        } else {
            self.generation_start_time = None;
            self.streaming_narration = None;
        }
        // Once a later turn is committed, follow the newest scene at the
        // bottom of the chronological stack. `u16::MAX` is intentionally a
        // sentinel: the renderer clamps it to the exact wrapped-line limit
        // for the current terminal size.
        if self.view.status.turn > previous_turn && previous_turn > 0 {
            // A new turn retires the previous turn's automation guide/badge.
            self.story_notice = None;
            self.narration_scroll = u16::MAX;
        } else if self.view.status.turn < previous_turn {
            self.narration_scroll = 0;
        }
    }

    /// Append an internal Writer stream fragment for the active generation.
    /// Late chunks are ignored, preserving the generation-id safety boundary.
    pub fn push_writer_chunk(&mut self, generation_id: Uuid, text: &str) {
        if self
            .view
            .loading
            .as_ref()
            .map(|loading| loading.generation_id)
            != Some(generation_id)
        {
            return;
        }
        self.streaming_narration
            .get_or_insert_with(String::new)
            .push_str(text);
        self.follow_story_end();
    }

    /// Replace the whole story stack. The ONLY sanctioned write path: the
    /// render line cache keys on `story_stack_version`, which this bumps.
    pub fn replace_story_stack(&mut self, stack: Vec<StoryStackEntry>) {
        self.story_stack_version = self.story_stack_version.wrapping_add(1);
        self.story_stack = stack;
    }

    /// Refresh the auxiliary list data from the host-owned engine state.
    /// `initial_location` is the pack's starting location — the seed for the
    /// per-turn location fold that drives inline scene images. Passing it
    /// atomically with the history keeps the seed and the stack desync-proof.
    pub fn set_lists(
        &mut self,
        history: &[story_model::HistoryEntry],
        memories: &[story_model::MemoryEntry],
        initial_location: &LocationId,
    ) {
        self.set_lists_with_checkpoints(history, memories, initial_location, &[]);
    }

    /// Like [`Self::set_lists`], but `checkpoint_turns` enables History delete
    /// only when a ring snapshot for `entry.turn - 1` is still available.
    pub fn set_lists_with_checkpoints(
        &mut self,
        history: &[story_model::HistoryEntry],
        memories: &[story_model::MemoryEntry],
        initial_location: &LocationId,
        checkpoint_turns: &[u32],
    ) {
        let previous_story_count = self.story_stack.len();
        self.initial_location = Some(initial_location.0.clone());
        let timeline = location_timeline(history, initial_location);
        let emotions = emotion_timeline(history);
        self.history_entry_ids = history.iter().rev().map(|entry| entry.id).collect();
        self.history_narrations = history
            .iter()
            .rev()
            .map(|entry| entry.narration.clone())
            .collect();
        self.history_turns = history.iter().rev().map(|entry| entry.turn).collect();
        self.history_deletable = history
            .iter()
            .rev()
            .map(|entry| {
                entry.turn > 1 && checkpoint_turns.iter().any(|turn| *turn == entry.turn - 1)
            })
            .collect();
        self.history_items = history
            .iter()
            .rev()
            .map(|entry| {
                let preview = entry.narration.lines().next().unwrap_or_default();
                // Display rules rewrite the row at the display boundary only
                // — the engine history and the save stay authoritative.
                let row = format!("턴 {} · {} — {}", entry.turn, entry.scene_id, preview);
                self.display_text(&row).into_owned()
            })
            .collect();
        if self.history_items.is_empty() {
            self.history_selected = 0;
        } else {
            self.history_selected = self
                .history_selected
                .min(self.history_items.len().saturating_sub(1));
        }
        let stack = history
            .iter()
            .zip(timeline)
            .zip(emotions)
            .map(|((entry, location), emotion)| StoryStackEntry {
                turn: entry.turn,
                scene_id: entry.scene_id.clone(),
                model: self.generation_models.get(&entry.generation_id).cloned(),
                generation_secs: self.generation_durations.get(&entry.generation_id).copied(),
                player_action: entry
                    .player_action
                    .as_ref()
                    .map(|action| action.original_text.trim().to_string()),
                narration: entry.narration.clone(),
                location,
                emotion,
            })
            .collect();
        self.replace_story_stack(stack);
        // The host refreshes the chronological stack before it publishes the
        // next view. Following this concrete history change is more reliable
        // than inferring it solely from a turn number.
        if previous_story_count > 0 && self.story_stack.len() > previous_story_count {
            self.narration_scroll = u16::MAX;
        }
        self.memory_items = memories
            .iter()
            .rev()
            .map(|memory| {
                let preview = memory
                    .interpretation
                    .clone()
                    .or_else(|| {
                        memory
                            .facts
                            .first()
                            .map(|f| format!("{} {} {}", f.subject, f.predicate, f.object))
                    })
                    .unwrap_or_else(|| "내용 없음".to_string());
                format!(
                    "턴 {} · {:?}{}",
                    memory.created_turn,
                    memory.kind,
                    if memory.unresolved {
                        " · 미해결"
                    } else {
                        ""
                    }
                ) + " — "
                    + &preview
            })
            .collect();
    }

    /// Move the narration viewport to the latest committed scene. The exact
    /// offset depends on terminal width, so the renderer resolves this
    /// sentinel against its wrapped-line count on the next draw.
    pub fn follow_story_end(&mut self) {
        self.narration_scroll = u16::MAX;
    }

    /// Handle one key. `pending_generation_id` lets `c` resolve to the real
    /// pending id (the keymap emits `Uuid::nil()` and the App fills it in).
    /// Returns `Some(EngineCommand)` when the host should run one.
    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        pending_generation_id: Option<Uuid>,
    ) -> Option<EngineCommand> {
        if self.greeting_picker.is_some() {
            self.handle_greeting_picker_key(key);
            return None;
        }
        if self.pack_delete_confirm.is_some() {
            self.handle_pack_delete_confirm_key(key);
            return None;
        }
        if self.new_game_confirm {
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                self.should_quit = true;
                return None;
            }
            return match key.code {
                KeyCode::Enter => {
                    self.new_game_confirm = false;
                    if self.new_game_confirm_from_home {
                        // Home origin: replay the home arm — the guard lets
                        // the from-home origin through; clear it afterwards
                        // so a cancelled greeting picker re-asks next time.
                        self.choose_home(0);
                        self.new_game_confirm_from_home = false;
                        None
                    } else {
                        Some(EngineCommand::StartNewGame)
                    }
                }
                KeyCode::Esc | KeyCode::Char('n' | 'ㅜ') => {
                    self.new_game_confirm = false;
                    self.new_game_confirm_from_home = false;
                    None
                }
                KeyCode::Char('q' | 'ㅂ') => {
                    self.should_quit = true;
                    self.new_game_confirm = false;
                    self.new_game_confirm_from_home = false;
                    None
                }
                _ => None,
            };
        }
        if self.history_edit.is_some() {
            return self.handle_history_edit_key(key);
        }
        if self.history_delete_confirm {
            return self.handle_history_delete_confirm_key(key);
        }
        if self.pack_picker.is_some() {
            self.handle_pack_picker_key(key);
            return None;
        }
        if self.import_dialog.is_some() {
            self.handle_import_dialog_key(key);
            return None;
        }
        if self.home_visible {
            self.handle_home_key(key);
            return None;
        }
        if self.save_load.is_some() {
            self.handle_save_load_key(key);
            return None;
        }
        if self.help_visible {
            // The help overlay is modal but not a trap: the same key closes
            // it that opened it, Esc works, and quit always works.
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                self.should_quit = true;
            } else {
                match key.code {
                    KeyCode::Char('?') | KeyCode::Esc => self.help_visible = false,
                    KeyCode::Char('q' | 'ㅂ') => self.should_quit = true,
                    _ => {}
                }
            }
            return None;
        }
        if self.view.error.is_some() {
            return match map_key(key, self.view.choices.len(), false, true) {
                KeyAction::None | KeyAction::MoveSelection(_) => None,
                KeyAction::Quit => {
                    self.should_quit = true;
                    None
                }
                KeyAction::Command(command) => self.resolve_command(command, pending_generation_id),
            };
        }
        if self.input_mode {
            return self.handle_input_key(key);
        }
        if self.list_mode.is_some() {
            match key.code {
                KeyCode::Esc => {
                    self.list_mode = None;
                    self.history_edit = None;
                    self.history_delete_confirm = false;
                    return None;
                }
                KeyCode::Char('h' | 'ㅗ') => {
                    self.list_mode = Some(ListMode::History);
                    self.list_scroll = 0;
                    self.history_selected = 0;
                    return None;
                }
                KeyCode::Char('m' | 'ㅡ') => {
                    self.list_mode = Some(ListMode::Memory);
                    self.list_scroll = 0;
                    return None;
                }
                KeyCode::Char('p' | 'ㅔ') => {
                    self.list_mode = Some(ListMode::Cast);
                    self.list_scroll = 0;
                    return None;
                }
                KeyCode::Up | KeyCode::Char('k' | 'ㅏ')
                    if self.list_mode == Some(ListMode::History) =>
                {
                    self.history_selected = self.history_selected.saturating_sub(1);
                    self.ensure_history_selection_visible();
                    return None;
                }
                KeyCode::Down | KeyCode::Char('j' | 'ㅓ')
                    if self.list_mode == Some(ListMode::History) =>
                {
                    if !self.history_items.is_empty() {
                        self.history_selected = (self.history_selected + 1)
                            .min(self.history_items.len().saturating_sub(1));
                        self.ensure_history_selection_visible();
                    }
                    return None;
                }
                KeyCode::Enter if self.list_mode == Some(ListMode::History) => {
                    self.open_history_edit();
                    return None;
                }
                KeyCode::Char('d' | 'ㅇ') if self.list_mode == Some(ListMode::History) => {
                    if self.history_delete_available() {
                        self.history_delete_confirm = true;
                    } else {
                        self.set_story_notice(Some(
                            "이 턴은 삭제할 수 없습니다 (오프닝 또는 되돌리기 링 밖)",
                        ));
                    }
                    return None;
                }
                KeyCode::PageUp => self.list_scroll = self.list_scroll.saturating_sub(5),
                KeyCode::PageDown => self.list_scroll = self.list_scroll.saturating_add(5),
                KeyCode::Char('r' | 'ㄱ') if self.list_mode == Some(ListMode::ModelPicker) => {
                    self.model_reload_requested = true;
                }
                KeyCode::Up | KeyCode::Char('k' | 'ㅏ')
                    if self.list_mode == Some(ListMode::ModelPicker) =>
                {
                    self.model_selected = self.model_selected.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j' | 'ㅓ')
                    if self.list_mode == Some(ListMode::ModelPicker) =>
                {
                    self.model_selected =
                        (self.model_selected + 1).min(self.model_items.len().saturating_sub(1));
                }
                KeyCode::Enter if self.list_mode == Some(ListMode::ModelPicker) => {
                    self.selected_model = self.model_items.get(self.model_selected).cloned();
                    self.list_mode = None;
                }
                KeyCode::Up | KeyCode::Char('k' | 'ㅏ')
                    if self.list_mode == Some(ListMode::ProviderPicker) =>
                {
                    self.provider_selected = self.provider_selected.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j' | 'ㅓ')
                    if self.list_mode == Some(ListMode::ProviderPicker) =>
                {
                    self.provider_selected = (self.provider_selected + 1)
                        .min(self.provider_items.len().saturating_sub(1));
                }
                KeyCode::Enter if self.list_mode == Some(ListMode::ProviderPicker) => {
                    self.selected_provider = self
                        .provider_items
                        .get(self.provider_selected)
                        .map(|(name, _)| name.clone());
                    self.list_mode = None;
                }
                // Role keys keep the picker open — the badge on the row is
                // the feedback, and assigning all three roles should not
                // take three round-trips through the Settings screen.
                KeyCode::Char('d' | 'ㅇ') if self.list_mode == Some(ListMode::ModelPicker) => {
                    self.selected_director_model =
                        self.model_items.get(self.model_selected).cloned();
                }
                KeyCode::Char('c' | 'ㅊ') if self.list_mode == Some(ListMode::ModelPicker) => {
                    self.selected_choice_model = self.model_items.get(self.model_selected).cloned();
                }
                KeyCode::Char('w' | 'ㅈ') if self.list_mode == Some(ListMode::ModelPicker) => {
                    self.selected_writer_model = self.model_items.get(self.model_selected).cloned();
                }
                _ => {}
            }
            return None;
        }
        if self.settings_visible {
            // A modal owns the screen while open — every other settings key
            // is absorbed until it closes.
            if self.persona_dialog.is_some() {
                self.handle_persona_dialog_key(key);
                return None;
            }
            if self.author_note_dialog.is_some() {
                self.handle_author_note_dialog_key(key);
                return None;
            }
            if self.lorebook_dialog.is_some() {
                self.handle_lorebook_dialog_key(key);
                return None;
            }
            if self.prompt_viewer.is_some() {
                match key.code {
                    KeyCode::Esc => self.prompt_viewer = None,
                    KeyCode::Up => self.scroll_prompt_viewer(-1),
                    KeyCode::Down => self.scroll_prompt_viewer(1),
                    KeyCode::PageUp => self.scroll_prompt_viewer(-10),
                    KeyCode::PageDown => self.scroll_prompt_viewer(10),
                    _ => {}
                }
                return None;
            }
            match key.code {
                KeyCode::Esc => {
                    // Keep the last safe settings projection. Esc only changes
                    // navigation; dropping this data made the next Settings
                    // entry fall through to the Story screen.
                    self.settings_visible = false;
                    self.home_visible = self.settings_returns_home;
                }
                KeyCode::Char('p' | 'ㅔ') => {
                    self.list_mode = Some(ListMode::ModelPicker);
                }
                // Same physical key under Korean 2-beolsik: e=ㄷ.
                KeyCode::Char('e' | 'ㄷ') => {
                    self.list_mode = Some(ListMode::ProviderPicker);
                }
                // Same physical key under Korean 2-beolsik: i=ㅑ.
                KeyCode::Char('i' | 'ㅑ') => {
                    self.images_toggle_requested = true;
                }
                // Same physical key under Korean 2-beolsik: g=ㅎ.
                KeyCode::Char('g' | 'ㅎ') => {
                    self.generation_toggle_requested = true;
                }
                // Same physical key under Korean 2-beolsik: s=ㄴ. Only
                // meaningful on the single-pass path (the two-stage Writer
                // always streams); the Settings row says so when `g` picked
                // the two-stage path.
                KeyCode::Char('s' | 'ㄴ') => {
                    self.stream_toggle_requested = true;
                }
                // Same physical key under Korean 2-beolsik: f=ㄹ.
                KeyCode::Char('f' | 'ㄹ') => {
                    self.expressions_toggle_requested = true;
                }
                // Same physical key under Korean 2-beolsik: b=ㅠ.
                KeyCode::Char('b' | 'ㅠ') => {
                    self.image_backend_cycle_requested = true;
                }
                // Same physical key under Korean 2-beolsik: x=ㅌ.
                KeyCode::Char('x' | 'ㅌ') => {
                    self.sampling_style_cycle_requested = true;
                }
                // Same physical key under Korean 2-beolsik: a=ㅁ.
                KeyCode::Char('a' | 'ㅁ') => {
                    self.open_persona_dialog();
                }
                // Same physical key under Korean 2-beolsik: w=ㅈ.
                KeyCode::Char('w' | 'ㅈ') => {
                    self.open_lorebook_dialog();
                }
                // Same physical key under Korean 2-beolsik: t=ㅅ.
                KeyCode::Char('t' | 'ㅅ') => {
                    self.open_author_note_dialog();
                }
                // Same physical key under Korean 2-beolsik: r=ㄱ.
                KeyCode::Char('r' | 'ㄱ') => {
                    self.open_prompt_viewer();
                }
                KeyCode::Char('q' | 'ㅂ') => self.should_quit = true,
                _ => {}
            }
            return None;
        }
        if matches!(key.code, KeyCode::Char('o' | 'ㅐ')) {
            self.settings_visible = true;
            self.settings_opened = true;
            self.settings_returns_home = false;
            return None;
        }
        if key.code == KeyCode::Home && self.view.loading.is_none() {
            self.open_home_from_game();
            return None;
        }
        // While a turn is being generated, choices must stay visually and
        // logically fixed. Only cancellation, re-rolling the turn in flight
        // (`r` → RegenerateCurrentTurn in map_key), and quitting remain
        // available.
        if self.view.loading.is_some()
            && !matches!(
                key.code,
                KeyCode::Char('c' | 'ㅊ' | 'q' | 'ㅂ' | 'r' | 'ㄱ')
            )
            && !key.modifiers.contains(KeyModifiers::CONTROL)
        {
            return None;
        }
        if !self.view.loading.is_some()
            && self.view.error.is_none()
            && !self.view.choices.is_empty()
            && matches!(key.code, KeyCode::Char('i' | 'ㅑ'))
        {
            self.input_mode = true;
            self.input.clear();
            return None;
        }
        if matches!(key.code, KeyCode::Char('h' | 'ㅗ')) {
            self.list_mode = Some(ListMode::History);
            self.list_scroll = 0;
            self.history_selected = 0;
            return None;
        }
        if matches!(key.code, KeyCode::Char('m' | 'ㅡ')) {
            self.list_mode = Some(ListMode::Memory);
            self.list_scroll = 0;
            return None;
        }
        // Full keymap reference — the answer to "what keys even exist".
        if matches!(key.code, KeyCode::Char('?')) {
            self.help_visible = true;
            return None;
        }
        if !self.view.loading.is_some() && self.view.error.is_none() {
            match key.code {
                // Same physical keys under Korean 2-beolsik: s=ㄴ, l=ㅣ, v=ㅓ.
                KeyCode::Char('s' | 'ㄴ') => {
                    self.save_load_request = Some(SaveLoadRequest::Open(SaveLoadMode::Save));
                    return None;
                }
                KeyCode::Char('l' | 'ㅣ') => {
                    self.save_load_request = Some(SaveLoadRequest::Open(SaveLoadMode::Load));
                    return None;
                }
                KeyCode::Char('v' | 'ㅓ') => {
                    self.tts_toggle_requested = true;
                    return None;
                }
                // Same physical key under Korean 2-beolsik: w=ㅈ. Idle-only,
                // like s/l/v — a turn in flight is absorbed above.
                KeyCode::Char('w' | 'ㅈ') => {
                    self.continue_requested = true;
                    return None;
                }
                // Same physical key under Korean 2-beolsik: z=ㅋ. The host
                // fetches a suggested action and feeds it back via
                // `set_input_text`; nothing is sent on this key.
                KeyCode::Char('z' | 'ㅋ') => {
                    self.suggest_action_requested = true;
                    return None;
                }
                // Same physical key under Korean 2-beolsik: g=ㅎ. Idle-only
                // like w/z — the host issues the group-scene side generation.
                KeyCode::Char('g' | 'ㅎ') => {
                    self.group_scene_requested = true;
                    return None;
                }
                // Same physical key under Korean 2-beolsik: p=ㅔ. Read-only
                // cast roster, same list-overlay family as h/m.
                KeyCode::Char('p' | 'ㅔ') => {
                    self.list_mode = Some(ListMode::Cast);
                    self.list_scroll = 0;
                    return None;
                }
                // Variant cycling of the last committed turn: `[` = previous
                // attempt, `]` = next. Brackets are layout-independent
                // special keys, so there is NO Korean 2-beolsik alias (the
                // keymap row notes it) — unlike letter keys.
                KeyCode::Char('[') => {
                    self.cycle_variant_back_requested = true;
                    return None;
                }
                KeyCode::Char(']') => {
                    self.cycle_variant_forward_requested = true;
                    return None;
                }
                KeyCode::Char('n' | 'ㅜ') if self.view.status.turn > 0 => {
                    // One accidental keypress must not wipe an in-progress
                    // run — StartNewGame clears the rewind ring too, so undo
                    // cannot recover it. Ask first. Before the opening
                    // (turn 0) the guard fails and map_key starts directly.
                    self.new_game_confirm = true;
                    self.new_game_confirm_from_home = false;
                    return None;
                }
                _ => {}
            }
        }
        match key.code {
            KeyCode::PageUp => {
                self.scroll_narration_by(-5);
                return None;
            }
            KeyCode::PageDown => {
                self.scroll_narration_by(5);
                return None;
            }
            KeyCode::Home if key.modifiers.contains(KeyModifiers::ALT) => {
                self.narration_scroll = 0;
                return None;
            }
            // Quick-action slots: fill the input from a configured text,
            // never send. The ALT modifier keeps the bare digit row's direct
            // choice-confirm untouched; digits are layout-independent, so no
            // Korean alias is needed.
            KeyCode::Char(c @ '1'..='9') if key.modifiers.contains(KeyModifiers::ALT) => {
                self.quick_action_request = Some(c.to_string());
                return None;
            }
            _ => {}
        }
        let loading = self.view.loading.is_some();
        let has_error = self.view.error.is_some();
        let choice_count = self.view.choices.len();
        match map_key(key, choice_count, loading, has_error) {
            KeyAction::None => None,
            KeyAction::Quit => {
                self.should_quit = true;
                None
            }
            KeyAction::MoveSelection(delta) => {
                self.move_selection(delta);
                None
            }
            KeyAction::Command(cmd) => self.resolve_command(cmd, pending_generation_id),
        }
    }

    /// Insert a terminal bracketed-paste payload into the active free-input
    /// editor. Paste events outside input mode are intentionally ignored.
    pub fn handle_paste(&mut self, text: &str) {
        if let Some(ImportDialog::Input { buffer }) = self.import_dialog.as_mut() {
            buffer.insert_str(text);
            return;
        }
        if let Some(dialog) = self.persona_dialog.as_mut() {
            let focused = match dialog.focus {
                0 => &mut dialog.name,
                _ => &mut dialog.persona,
            };
            focused.insert_str(text);
            return;
        }
        if let Some(dialog) = self.author_note_dialog.as_mut() {
            dialog.note.insert_str(text);
            return;
        }
        if let Some(dialog) = self.lorebook_dialog.as_mut()
            && let Some(edit) = dialog.edit.as_mut()
        {
            let (LorebookEdit::Content { buffer } | LorebookEdit::Keys { buffer }) = edit;
            buffer.insert_str(text);
            return;
        }
        if self.input_mode {
            self.input.insert_str(text);
        }
    }

    /// Refresh the per-entry line cache if the stack changed. Split from
    /// [`Self::story_blocks`] so the cache fill's mutable borrow ends before
    /// the returned blocks borrow `self` immutably — `render` needs `&self`
    /// for the view while the blocks are alive.
    fn fill_block_cache(&mut self) {
        if self.story_stack.is_empty() {
            return;
        }
        // The stored version IS the cache key: a set_lists bump invalidates
        // the cache on the very next frame.
        let cache_matches = self
            .stack_lines_cache
            .as_ref()
            .is_some_and(|(version, _)| *version == self.story_stack_version);
        if !cache_matches {
            let lines: Vec<Vec<Line<'static>>> = self
                .story_stack
                .iter()
                .enumerate()
                .map(|(index, entry)| {
                    // Display rules rewrite a throwaway clone; the stack
                    // entry itself stays authoritative.
                    let mut entry = entry.clone();
                    entry.narration = self.display_text(&entry.narration).into_owned();
                    story_entry_lines(&entry, index == 0)
                })
                .collect();
            self.stack_lines_cache = Some((self.story_stack_version, lines));
        }
    }

    /// Apply the display rules to a piece of transcript text. No rules means
    /// a borrowed passthrough — the common case allocates nothing.
    fn display_text<'a>(&self, text: &'a str) -> std::borrow::Cow<'a, str> {
        if self.display_rules.is_empty() {
            return std::borrow::Cow::Borrowed(text);
        }
        let mut current = text.to_string();
        for (pattern, replacement) in &self.display_rules {
            if current.len() > 1_000_000 {
                break; // pathological output growth: stop rewriting, keep showing
            }
            current = pattern
                .replace_all(&current, replacement.as_str())
                .into_owned();
        }
        std::borrow::Cow::Owned(current)
    }

    /// Install the host-compiled display rules (see
    /// `[[display_rules]]` in user-settings.toml). Replaces any prior set.
    pub fn set_display_rules(&mut self, rules: Vec<(regex::Regex, String)>) {
        self.display_rules = rules;
    }

    /// Blocks for the narration pane, in transcript order: a scene image
    /// before each turn whose location's image differs from the previous
    /// turn's, an expression sprite before each turn whose emotion's sprite
    /// differs, then the turn's text.
    fn story_blocks(&self) -> Vec<StoryBlock<'_>> {
        let mut blocks: Vec<StoryBlock<'_>> = Vec::new();
        // The dedupe key is the resolved SOURCE PATH, not the location or
        // emotion id: a pack whose locations all fall back to the same
        // character portrait must not repeat that portrait at every boundary,
        // while genuinely different artwork always shows. A location without
        // art breaks the run, so returning to imaged art shows it again.
        let mut previous_source: Option<&std::path::PathBuf> = None;
        if self.story_stack.is_empty() {
            // The opening turn shows the initial location's image, if the
            // pack ships one — the transcript's first scene boundary.
            if self.images_enabled
                && let Some(initial) = self.initial_location.as_deref()
                && let Some(Some(image)) = self.location_images.get(initial)
            {
                blocks.push(StoryBlock::Image(&image.image));
                previous_source = Some(&image.source);
            }
            // The opening narration's emotion is the first expression beat;
            // it dedupes against the scene image above.
            if self.expressions_enabled
                && let Some(Some(image)) = self
                    .expression_images
                    .get(&classify_emotion(&self.view.narration))
                && previous_source != Some(&image.source)
            {
                blocks.push(StoryBlock::Image(&image.image));
            }
            blocks.push(StoryBlock::Text(std::borrow::Cow::Owned(markdown_lines(
                &self.display_text(&self.view.narration),
            ))));
            if let Some(streaming) = self.streaming_narration.as_deref() {
                let streaming = self.display_text(streaming).into_owned();
                blocks.push(StoryBlock::Text(std::borrow::Cow::Owned(
                    streaming_entry_lines(self.view.status.turn, &streaming),
                )));
            }
            push_variant_badge(&mut blocks, self.view.variant_label.as_deref());
            push_story_notice(&mut blocks, self.story_notice.as_deref());
            return blocks;
        }
        let cache = &self
            .stack_lines_cache
            .as_ref()
            .expect("fill_block_cache ran before story_blocks")
            .1;
        let mut blocks: Vec<StoryBlock<'_>> = Vec::with_capacity(cache.len() + 4);
        if !self.images_enabled && !self.expressions_enabled {
            for lines in cache {
                blocks.push(StoryBlock::Text(std::borrow::Cow::Borrowed(lines)));
            }
        } else {
            // Two independent dedupe runs — scenery and expression each
            // repeat only when their own visual actually changes — plus one
            // shared rule: neither kind may repeat the source the other kind
            // is currently showing (portrait-fallback collisions).
            let mut scene_run: Option<&std::path::PathBuf> = None;
            let mut expression_run: Option<&std::path::PathBuf> = None;
            for (entry, lines) in self.story_stack.iter().zip(cache) {
                if self.images_enabled {
                    let resolved = entry
                        .location
                        .as_deref()
                        .and_then(|location| self.location_images.get(location))
                        .and_then(|resolved| resolved.as_ref());
                    if let Some(image) = resolved {
                        if Some(&image.source) != scene_run && Some(&image.source) != expression_run
                        {
                            blocks.push(StoryBlock::Image(&image.image));
                        }
                        scene_run = Some(&image.source);
                    } else {
                        // No art at this boundary: break the scenery run so
                        // returning to imaged art shows it again.
                        scene_run = None;
                    }
                } else {
                    scene_run = None;
                }
                if self.expressions_enabled {
                    let sprite = self
                        .expression_images
                        .get(&entry.emotion)
                        .and_then(|resolved| resolved.as_ref());
                    match sprite {
                        Some(image) => {
                            if Some(&image.source) != expression_run
                                && Some(&image.source) != scene_run
                            {
                                blocks.push(StoryBlock::Image(&image.image));
                            }
                            expression_run = Some(&image.source);
                        }
                        // No usable sprite this turn: break the expression
                        // run so a returning sprite shows again.
                        None => expression_run = None,
                    }
                } else {
                    expression_run = None;
                }
                blocks.push(StoryBlock::Text(std::borrow::Cow::Borrowed(lines)));
            }
        }
        if let Some(streaming) = self.streaming_narration.as_deref() {
            let streaming = self.display_text(streaming).into_owned();
            blocks.push(StoryBlock::Text(std::borrow::Cow::Owned(
                streaming_entry_lines(self.view.status.turn, &streaming),
            )));
        }
        push_variant_badge(&mut blocks, self.view.variant_label.as_deref());
        push_story_notice(&mut blocks, self.story_notice.as_deref());
        blocks
    }

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        self.render_base(frame, area);
        // The import dialog draws last so it overlays the base screen. The
        // cursor split is computed first so its owned strings outlive the view.
        let cursor_split = if let Some(ImportDialog::Input { buffer }) = self.import_dialog.as_ref()
        {
            Some(buffer.split_at_cursor())
        } else {
            None
        };
        let view = match self.import_dialog.as_ref() {
            Some(ImportDialog::Input { .. }) => {
                let (before, after) = cursor_split.as_ref().expect("input split set above");
                Some(crate::screens::import::ImportDialogView::Input { before, after })
            }
            Some(ImportDialog::Browse {
                dir,
                entries,
                selected,
                ..
            }) => Some(crate::screens::import::ImportDialogView::Browse {
                dir,
                entries,
                selected: *selected,
            }),
            Some(ImportDialog::Running { path }) => {
                Some(crate::screens::import::ImportDialogView::Running { path })
            }
            Some(ImportDialog::Report { path, lines, ready }) => {
                Some(crate::screens::import::ImportDialogView::Report {
                    path,
                    lines,
                    ready: ready.is_some(),
                })
            }
            None => None,
        };
        if let Some(view) = view {
            crate::screens::import::render_import_dialog(frame, area, view);
        }
        if let Some(selected) = self.greeting_picker {
            crate::screens::greeting::render_greeting_picker(
                frame,
                area,
                &self.greetings,
                selected,
            );
        }
        if let Some(selected) = self.pack_picker {
            crate::screens::pack::render_pack_picker(
                frame,
                area,
                &self.pack_labels,
                selected,
                self.pack_notice.as_deref(),
            );
        }
        if let Some(index) = self.pack_delete_confirm {
            let title = self
                .pack_labels
                .get(index)
                .map(String::as_str)
                .unwrap_or("");
            crate::screens::pack::render_pack_delete_confirm(frame, area, title);
        }
        if self.new_game_confirm {
            crate::screens::story::render_new_game_confirm(frame, area, self.view.status.turn);
        }
    }

    fn render_base(&mut self, frame: &mut Frame, area: Rect) {
        self.last_render_area.set(area);
        if self.home_visible {
            render_home(
                frame,
                area,
                self.home_selected,
                self.can_continue,
                self.home_status
                    .as_ref()
                    .map(|(line, attention)| (line.as_str(), *attention)),
            );
            return;
        }
        if let Some(state) = &self.save_load {
            let (title, footer) = save_load_copy(state);
            // Save mode names exactly what Enter will write; load mode has
            // nothing to name yet.
            let target = match state.mode {
                SaveLoadMode::Save => Some(format!(
                    "저장 대상: {} · 턴 {}",
                    self.view.story_title, self.view.status.turn
                )),
                SaveLoadMode::Load => None,
            };
            render_save_load(
                frame,
                area,
                title,
                &state.slots,
                state.selected,
                footer,
                target.as_deref(),
            );
            return;
        }
        if let Some(mode) = self.list_mode {
            if mode == ListMode::ModelPicker {
                render_model_picker(
                    frame,
                    area,
                    &self.model_items,
                    self.model_selected,
                    &self.model_role_hints(),
                );
                return;
            }
            if mode == ListMode::ProviderPicker {
                render_provider_picker(frame, area, &self.provider_items, self.provider_selected);
                return;
            }
            let cast_items;
            let items: &[String] = match mode {
                ListMode::History => &self.history_items,
                ListMode::Memory => &self.memory_items,
                ListMode::Cast => {
                    cast_items = self.cast_roster_rows();
                    &cast_items
                }
                ListMode::ModelPicker | ListMode::ProviderPicker => unreachable!("rendered above"),
            };
            let selected = (mode == ListMode::History).then_some(self.history_selected);
            render_list(frame, area, mode, items, self.list_scroll, selected);
            if mode == ListMode::History {
                if let Some(dialog) = self.history_edit.as_ref() {
                    let (before, after) = dialog.buffer.split_at_cursor();
                    render_history_edit_dialog(frame, area, dialog.turn, &before, &after);
                } else if self.history_delete_confirm {
                    let turn = self
                        .history_turns
                        .get(self.history_selected)
                        .copied()
                        .unwrap_or(0);
                    render_history_delete_confirm(frame, area, turn);
                }
            }
            return;
        }
        if self.settings_visible
            && let Some(settings) = &self.settings
        {
            render_settings(
                frame,
                area,
                &SettingsView {
                    provider: &settings.provider,
                    health: settings.health,
                    temperature: settings.temperature,
                    top_p: settings.top_p,
                    top_k: settings.top_k,
                    max_tokens: settings.max_tokens,
                    tts_enabled: settings.tts.enabled,
                    tts_backend: settings.tts.backend.clone(),
                    tts_voice: settings.tts.voice.clone(),
                    images_enabled: settings.images_enabled,
                    image_backend: settings.image_backend.clone(),
                    image_backend_manual: settings.image_backend_manual,
                    use_director_writer: settings.use_director_writer,
                    stream_turns: settings.stream_turns,
                    stream_supported: settings.stream_supported,
                    expressions_enabled: settings.expressions_enabled,
                    sampling_style: &settings.sampling_style,
                    persona_label: &settings.persona_label,
                    theme_name: "default",
                    returns_home: self.settings_returns_home,
                },
            );
            // The persona modal draws last so it overlays the settings screen.
            if let Some(dialog) = self.persona_dialog.as_ref() {
                let name = dialog.name.split_at_cursor();
                let persona = dialog.persona.split_at_cursor();
                crate::screens::settings::render_persona_dialog(
                    frame,
                    area,
                    (&name.0, &name.1),
                    (&persona.0, &persona.1),
                    dialog.focus,
                );
            }
            // The other settings modals draw last so they overlay the screen.
            if let Some(dialog) = self.author_note_dialog.as_ref() {
                let note = dialog.note.split_at_cursor();
                crate::screens::settings::render_author_note_dialog(
                    frame,
                    area,
                    (&note.0, &note.1),
                );
            }
            if let Some(dialog) = self.lorebook_dialog.as_ref() {
                let edit = dialog.edit.as_ref().map(|edit| {
                    let (label, buffer) = match edit {
                        LorebookEdit::Content { buffer } => ("내용", buffer),
                        LorebookEdit::Keys { buffer } => ("키", buffer),
                    };
                    let (before, after) = buffer.split_at_cursor();
                    (label, before, after)
                });
                crate::screens::settings::render_lorebook_dialog(
                    frame,
                    area,
                    &dialog.entries,
                    dialog.selected,
                    dialog.persisted,
                    edit.as_ref()
                        .map(|(label, before, after)| (*label, before.as_str(), after.as_str())),
                );
            }
            if let (Some(preview), Some(scroll)) = (&self.prompt_preview, self.prompt_viewer) {
                crate::screens::settings::render_prompt_viewer(
                    frame,
                    area,
                    &preview.system,
                    &preview.user,
                    scroll,
                );
            }
            return;
        }
        self.fill_block_cache();
        let blocks = self.story_blocks();
        let max_scroll = render_story(
            frame,
            area,
            &self.view,
            self.selected,
            self.narration_scroll,
            self.input_mode,
            &self.input,
            &blocks,
            self.streaming_narration.as_deref(),
            self.generation_elapsed_seconds(),
        );
        self.narration_max_scroll.set(max_scroll);
        if self.help_visible {
            render_help_overlay(frame, area);
        }
    }

    /// Open the slot chooser after the host has read the available saves.
    pub fn open_save_load(&mut self, mode: SaveLoadMode, slots: Vec<SlotMeta>) {
        self.save_load = Some(SaveLoadState {
            mode,
            slots,
            selected: 0,
            overwrite_confirmation: false,
        });
    }

    /// Consume pending filesystem work requested by the save/load UI.
    pub fn take_save_load_request(&mut self) -> Option<SaveLoadRequest> {
        self.save_load_request.take()
    }

    /// Close the slot chooser after the host completed or rejected an action.
    pub fn close_save_load(&mut self) {
        self.save_load = None;
    }

    /// Handle mouse clicks and scrolling across all screens and modals.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            let area = self.last_render_area.get();

            // 0. The new-game confirm owns the screen: clicks must not drive
            // the story (footer undo/home/...) from behind the modal.
            if self.new_game_confirm {
                return;
            }

            // 1. Import dialog is keyboard-focused for path input.
            if self.import_dialog.is_some() {
                return;
            }

            // 2. Greeting picker modal click.
            if self.greeting_picker.is_some() {
                if let Some(picked) = greeting_selection_at(area, mouse.row, self.greetings.len()) {
                    self.greeting_picker = None;
                    self.greeting_selection = Some(picked);
                    self.home_request = Some(HomeRequest::StartNew);
                }
                return;
            }

            // 3. Pack picker modal click.
            if let Some(selected_idx) = self.pack_picker {
                if let Some(clicked_idx) =
                    pack_selection_at(area, mouse.row, self.pack_labels.len(), false)
                {
                    if clicked_idx == selected_idx {
                        if let Some(dir) = self.pack_dirs.get(clicked_idx) {
                            let dir = dir.clone();
                            self.pack_picker = None;
                            self.pack_swap_request = Some(dir);
                        }
                    } else {
                        self.pack_picker = Some(clicked_idx);
                    }
                }
                return;
            }

            // 4. Save/Load modal click.
            if let Some(state) = self.save_load.as_mut() {
                if let Some(clicked_slot) =
                    save_load_selection_at(area, mouse.row, state.slots.len())
                {
                    state.selected = clicked_slot;
                    let slot = &state.slots[clicked_slot];
                    match state.mode {
                        SaveLoadMode::Load if !slot.story_id.is_empty() => {
                            self.save_load_request = Some(SaveLoadRequest::Load {
                                slot_id: slot.slot_id.clone(),
                            });
                        }
                        SaveLoadMode::Save
                            if !slot.story_id.is_empty() && !state.overwrite_confirmation =>
                        {
                            state.overwrite_confirmation = true;
                        }
                        SaveLoadMode::Save => {
                            self.save_load_request = Some(SaveLoadRequest::Save {
                                slot_id: slot.slot_id.clone(),
                            });
                        }
                        SaveLoadMode::Load => {}
                    }
                }
                return;
            }

            // 5. Home screen menu click.
            if self.home_visible {
                if let Some(selection) = menu_selection_at(area, mouse.row) {
                    self.home_selected = selection;
                    self.choose_home(selection);
                }
                return;
            }

            // 6. Model picker modal click.
            if self.list_mode == Some(ListMode::ModelPicker) {
                let scroll = model_picker_scroll(area, self.model_selected) as usize;
                let index = scroll + mouse.row.saturating_sub(area.y.saturating_add(1)) as usize;
                if let Some(model) = self.model_items.get(index).cloned() {
                    self.model_selected = index;
                    self.selected_model = Some(model);
                    self.list_mode = None;
                }
                return;
            }

            // 7. Provider picker modal click.
            if self.list_mode == Some(ListMode::ProviderPicker) {
                let scroll = model_picker_scroll(area, self.provider_selected) as usize;
                let index = scroll + mouse.row.saturating_sub(area.y.saturating_add(1)) as usize;
                if let Some((name, _)) = self.provider_items.get(index).cloned() {
                    self.provider_selected = index;
                    self.selected_provider = Some(name);
                    self.list_mode = None;
                }
                return;
            }

            // 8. Settings screen clicks.
            if self.settings_visible && mouse.row == area.y.saturating_add(4) {
                // The model row is the second line in the provider block.
                self.list_mode = Some(ListMode::ModelPicker);
                return;
            }

            // 9. Story screen footer shortcuts click.
            if let Some(action) =
                footer_action_at(area, &self.view, self.input_mode, mouse.column, mouse.row)
            {
                match action {
                    StoryFooterAction::Home => {
                        self.open_home_from_game();
                    }
                    StoryFooterAction::NewGame => {
                        self.choose_home(0);
                    }
                    StoryFooterAction::Input => {
                        if self.view.loading.is_none()
                            && self.view.error.is_none()
                            && !self.view.choices.is_empty()
                        {
                            self.input_mode = true;
                        }
                    }
                    StoryFooterAction::Save => {
                        if self.view.loading.is_none() && self.view.error.is_none() {
                            self.save_load_request =
                                Some(SaveLoadRequest::Open(SaveLoadMode::Save));
                        }
                    }
                    StoryFooterAction::Load => {
                        if self.view.loading.is_none() && self.view.error.is_none() {
                            self.save_load_request =
                                Some(SaveLoadRequest::Open(SaveLoadMode::Load));
                        }
                    }
                    StoryFooterAction::Settings => {
                        if self.view.loading.is_none() && self.view.error.is_none() {
                            self.settings_visible = true;
                        }
                    }
                    StoryFooterAction::History => {
                        if self.view.loading.is_none() && self.view.error.is_none() {
                            self.list_mode = Some(ListMode::History);
                            self.list_scroll = 0;
                            self.history_selected = 0;
                        }
                    }
                    StoryFooterAction::Memory => {
                        if self.view.loading.is_none() && self.view.error.is_none() {
                            self.list_mode = Some(ListMode::Memory);
                        }
                    }
                    StoryFooterAction::Regenerate => {
                        if self.view.loading.is_none()
                            && self.view.error.is_none()
                            && !self.view.choices.is_empty()
                        {
                            self.mouse_command = Some(EngineCommand::RegenerateChoices);
                        }
                    }
                    StoryFooterAction::Undo => {
                        if self.view.loading.is_none()
                            && self.view.error.is_none()
                            && let Some(checkpoint_id) = self.view.undo_checkpoint_id
                        {
                            self.mouse_command =
                                Some(EngineCommand::RestoreCheckpoint { checkpoint_id });
                        }
                    }
                    StoryFooterAction::Help => {
                        self.help_visible = true;
                    }
                    StoryFooterAction::InputSubmit => {
                        if self.input_mode {
                            let text = self.input.text();
                            if !text.trim().is_empty() {
                                self.input_mode = false;
                                self.input.clear();
                                self.mouse_command = Some(EngineCommand::SubmitFreeAction {
                                    action: Box::new(self.parsed_free_action(&text)),
                                });
                            }
                        }
                    }
                    StoryFooterAction::InputCancel => {
                        if self.input_mode {
                            self.input.clear();
                            self.input_mode = false;
                        }
                    }
                }
                return;
            }

            // 10. Story choice click.
            if self.view.error.is_none()
                && let Some(index) = choice_index_at(area, &self.view, self.input_mode, mouse.row)
                && let Some(choice) = self.view.choices.get(index)
            {
                self.selected = index;
                self.mouse_command = Some(EngineCommand::SubmitChoice {
                    choice_id: choice.id.clone(),
                });
                return;
            }
        }
        let delta: i32 = match mouse.kind {
            // Trackpads can emit wheel events much faster than a terminal can
            // repaint. Moving only one wrapped line per event makes a fast
            // flick feel as though input was dropped. Three lines retains
            // reading precision while keeping up with a quick wheel gesture.
            MouseEventKind::ScrollUp => -3,
            MouseEventKind::ScrollDown => 3,
            _ => return,
        };
        if self.list_mode.is_some() {
            if delta < 0 {
                self.list_scroll = self.list_scroll.saturating_sub(delta.unsigned_abs() as u16);
            } else {
                self.list_scroll = self.list_scroll.saturating_add(delta as u16);
            }
        } else {
            self.scroll_narration_by(delta);
        }
    }

    fn scroll_narration_by(&mut self, delta: i32) {
        let max_scroll = self.narration_max_scroll.get();
        // `u16::MAX` means "follow the bottom" after a new turn. Convert it
        // to the real, layout-derived bottom before applying an upward wheel
        // event so the first upward scroll responds immediately.
        let current = self.narration_scroll.min(max_scroll);
        self.narration_scroll = if delta < 0 {
            current.saturating_sub(delta.unsigned_abs() as u16)
        } else {
            // Reaching the known bottom restores the sentinel. This keeps the
            // transcript pinned after a resize or a new render discovers more
            // wrapped lines than the previous frame did.
            let next = current.saturating_add(delta as u16);
            if self.narration_scroll == u16::MAX || next >= max_scroll {
                u16::MAX
            } else {
                next
            }
        };
    }

    fn handle_save_load_key(&mut self, key: KeyEvent) {
        let requested_mode = match key.code {
            KeyCode::Char('s' | 'ㄴ') => Some(SaveLoadMode::Save),
            KeyCode::Char('l' | 'ㅣ') => Some(SaveLoadMode::Load),
            _ => None,
        };
        if let Some(mode) = requested_mode {
            self.save_load_request = Some(SaveLoadRequest::Open(mode));
            return;
        }
        let Some(state) = self.save_load.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.save_load = None,
            KeyCode::Up | KeyCode::Char('k' | 'ㅏ') => {
                state.selected = state.selected.saturating_sub(1);
                state.overwrite_confirmation = false;
            }
            KeyCode::Down | KeyCode::Char('j' | 'ㅓ') => {
                state.selected = (state.selected + 1).min(state.slots.len().saturating_sub(1));
                state.overwrite_confirmation = false;
            }
            KeyCode::Enter => {
                let Some(slot) = state.slots.get(state.selected) else {
                    return;
                };
                match state.mode {
                    SaveLoadMode::Load if !slot.story_id.is_empty() => {
                        self.save_load_request = Some(SaveLoadRequest::Load {
                            slot_id: slot.slot_id.clone(),
                        });
                    }
                    SaveLoadMode::Save
                        if !slot.story_id.is_empty() && !state.overwrite_confirmation =>
                    {
                        state.overwrite_confirmation = true;
                    }
                    SaveLoadMode::Save => {
                        self.save_load_request = Some(SaveLoadRequest::Save {
                            slot_id: slot.slot_id.clone(),
                        });
                    }
                    SaveLoadMode::Load => {}
                }
            }
            _ => {}
        }
    }

    fn handle_home_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('q' | 'ㅂ') => self.should_quit = true,
            KeyCode::Up | KeyCode::Char('k' | 'ㅏ') => {
                self.home_selected = self.home_selected.saturating_sub(1);
                if !self.can_continue && self.home_selected == 1 {
                    self.home_selected = 0;
                }
            }
            KeyCode::Down | KeyCode::Char('j' | 'ㅓ') => {
                self.home_selected = (self.home_selected + 1).min(5);
                if !self.can_continue && self.home_selected == 1 {
                    self.home_selected = 2;
                }
            }
            KeyCode::Char('n' | 'ㅜ') => self.choose_home(0),
            KeyCode::Char('c' | 'ㅊ') if self.can_continue => self.choose_home(1),
            KeyCode::Char('s' | 'ㄴ') => self.choose_home(2),
            KeyCode::Enter => self.choose_home(self.home_selected),
            _ => {}
        }
    }

    fn choose_home(&mut self, selection: usize) {
        match selection {
            0 => {
                // The destructive default row: with a run in progress, a
                // stray Enter (or click, or `n`) on the home menu must not
                // reset it — same confirm modal as the in-game `n`. The
                // from-home origin replays this exact arm after Enter.
                if self.view.status.turn > 0 && !self.new_game_confirm_from_home {
                    self.new_game_confirm = true;
                    self.new_game_confirm_from_home = true;
                    return;
                }
                // Alternate openings gate the start — but with a single
                // option there is nothing to choose; skip the modal.
                if self.greetings.len() > 1 {
                    self.open_greeting_picker();
                    return;
                }
                self.home_visible = false;
                self.home_request = Some(HomeRequest::StartNew);
            }
            1 if self.can_continue => {
                self.home_visible = false;
                self.home_request = Some(if self.home_continue_returns_to_game {
                    HomeRequest::ReturnToGame
                } else {
                    HomeRequest::Continue
                });
            }
            2 => {
                self.home_visible = false;
                self.settings_visible = true;
                self.settings_opened = true;
                self.settings_returns_home = true;
            }
            3 => self.open_import_dialog(),
            4 => {
                // The picker overlays the home menu, which stays visible
                // underneath.
                self.home_request = Some(HomeRequest::PickPack);
            }
            5 => self.should_quit = true,
            _ => {}
        }
    }

    fn handle_input_key(&mut self, key: KeyEvent) -> Option<EngineCommand> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return None;
        }
        match key.code {
            KeyCode::Esc => {
                self.input_mode = false;
                self.input.clear();
            }
            KeyCode::Enter => {
                let text = self.input.text();
                if text.trim().is_empty() {
                    return None;
                }
                self.input_mode = false;
                self.input.clear();
                return Some(EngineCommand::SubmitFreeAction {
                    action: Box::new(self.parsed_free_action(&text)),
                });
            }
            KeyCode::Backspace => self.input.backspace(),
            KeyCode::Delete => self.input.delete(),
            KeyCode::Left => self.input.cursor_left(),
            KeyCode::Right => self.input.cursor_right(),
            KeyCode::Home => self.input.cursor_start(),
            KeyCode::End => self.input.cursor_end(),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.input.insert_grapheme(&c.to_string());
            }
            _ => {}
        }
        None
    }

    fn move_selection(&mut self, delta: i32) {
        let count = self.view.choices.len();
        if count == 0 {
            self.selected = 0;
            return;
        }
        let mut next = self.selected as i32 + delta;
        if next < 0 {
            next = 0;
        }
        if next as usize >= count {
            next = (count - 1) as i32;
        }
        self.selected = next as usize;
    }

    /// Rewrite the sentinel `SubmitChoice` / `CancelGeneration` placeholders
    /// emitted by [`crate::event::map_key`] into real engine commands.
    fn resolve_command(&self, cmd: EngineCommand, pending: Option<Uuid>) -> Option<EngineCommand> {
        match cmd {
            EngineCommand::SubmitChoice { choice_id } => {
                // Number-row direct confirm? `__select__{idx}`.
                if let Some(idx) = parse_direct_select_index(&choice_id) {
                    return self
                        .view
                        .choices
                        .get(idx)
                        .map(|c| EngineCommand::SubmitChoice {
                            choice_id: c.id.clone(),
                        });
                }
                // Enter on highlighted selection? bare `__select__`.
                if choice_id == SELECT_SENTINEL {
                    return self.view.choices.get(self.selected).map(|c| {
                        EngineCommand::SubmitChoice {
                            choice_id: c.id.clone(),
                        }
                    });
                }
                Some(EngineCommand::SubmitChoice { choice_id })
            }
            EngineCommand::CancelGeneration { .. } => {
                pending.map(|gid| EngineCommand::CancelGeneration { generation_id: gid })
            }
            EngineCommand::RestoreCheckpoint { checkpoint_id } if checkpoint_id.is_nil() => self
                .view
                .undo_checkpoint_id
                .map(|id| EngineCommand::RestoreCheckpoint { checkpoint_id: id }),
            other => Some(other),
        }
    }
}

/// Per-entry location after folding `applied_patch.next_location` over the
/// history in order, seeded by the pack's initial location.
///
/// `HistoryEntry` carries no location of its own — the reducer's state patch
/// does — so the timeline is a pure function of (history, seed). Because the
/// save payload mirrors history, the same fold reproduces the timeline after a
/// load; the host reuses this for its "which locations to load" pass.
pub fn location_timeline(
    history: &[story_model::HistoryEntry],
    initial: &LocationId,
) -> Vec<Option<String>> {
    let mut current = initial.0.clone();
    history
        .iter()
        .map(|entry| {
            if let Some(next) = &entry.applied_patch.next_location {
                current = next.0.clone();
            }
            Some(current.clone())
        })
        .collect()
}

/// Narration cues per [`Emotion`], Korean-led with a few English cues. Table
/// order is the tie-break order when two emotions score equally.
///
/// This is the zero-inference stand-in for SillyTavern's separate classifier
/// model: a weak local setup pays no extra generation for expression
/// sprites, and the worst case of a miss is "no sprite / neutral", never
/// wrong story state.
const EMOTION_CUES: &[(Emotion, &[&str])] = &[
    (
        Emotion::Happy,
        &[
            "기쁘",
            "즐거",
            "행복",
            "환한 미소",
            "방긋",
            "신나",
            "웃음",
            "smiles",
        ],
    ),
    (
        Emotion::Sad,
        &[
            "슬프",
            "슬픈",
            "눈물",
            "울먹",
            "씁쓸",
            "가라앉",
            "서러",
            "먹먹",
            "흐느",
            "tears",
        ],
    ),
    (
        Emotion::Angry,
        &[
            "분노",
            "노기",
            "성난",
            "화를 내",
            "화가 나",
            "째려",
            "이를 갈",
            "snaps",
        ],
    ),
    (
        Emotion::Anxious,
        &[
            "불안",
            "초조",
            "긴장",
            "걱정",
            "조마조마",
            "떨리는 숨",
            "restless",
        ],
    ),
    (
        Emotion::Surprised,
        &["놀라", "깜짝", "벌떡", "숨을 삼키", "눈을 동그랗", "gasps"],
    ),
    (
        Emotion::Affectionate,
        &[
            "다정",
            "애정",
            "사랑",
            "설렘",
            "껴안",
            "살갑",
            "살며시 안",
            "hugs",
        ],
    ),
    (
        Emotion::Cold,
        &["차갑", "차가", "싸늘", "무심", "서늘", "냉랭", "coldly"],
    ),
    (
        Emotion::Embarrassed,
        &["붉어", "빨개", "부끄", "머쓱", "당황", "허둥", "blushes"],
    ),
    (
        Emotion::Guilty,
        &["죄책", "미안", "참회", "고개를 숙", "apolog"],
    ),
];

/// Classify the dominant emotion of one narration passage. Pure keyword
/// counting — no model call, fully deterministic. Highest hit count wins;
/// equal scores resolve to the earlier table entry; no hits are `Neutral`.
/// English cues must be lowercase (the haystack is lowercased once).
pub fn classify_emotion(narration: &str) -> Emotion {
    let haystack = narration.to_lowercase();
    let mut best = (Emotion::Neutral, 0usize);
    for (emotion, cues) in EMOTION_CUES {
        let hits: usize = cues.iter().map(|cue| haystack.matches(cue).count()).sum();
        if hits > best.1 {
            best = (*emotion, hits);
        }
    }
    best.0
}

/// Per-entry emotion after classifying each turn's narration in order.
///
/// Mirrors [`location_timeline`]: a pure function of the history, so the same
/// fold reproduces the timeline after a save load. The opening turn is not
/// part of the history — classify `App::view.narration` separately there.
pub fn emotion_timeline(history: &[story_model::HistoryEntry]) -> Vec<Emotion> {
    history
        .iter()
        .map(|entry| classify_emotion(&entry.narration))
        .collect()
}

/// Korean label for a relationship stage (cast roster rows).
fn relationship_stage_label(stage: RelationshipStage) -> &'static str {
    match stage {
        RelationshipStage::Stranger => "낯선 사이",
        RelationshipStage::Acquaintance => "지인",
        RelationshipStage::Friend => "친구",
        RelationshipStage::CloseFriend => "절친",
        RelationshipStage::RomanticInterest => "호감",
        RelationshipStage::Partner => "연인",
        RelationshipStage::Estranged => "소원",
    }
}

/// Korean label for a dominant emotion (cast roster rows).
fn emotion_label(emotion: Emotion) -> &'static str {
    match emotion {
        Emotion::Neutral => "중립",
        Emotion::Happy => "기쁨",
        Emotion::Sad => "슬픔",
        Emotion::Angry => "분노",
        Emotion::Anxious => "불안",
        Emotion::Surprised => "놀람",
        Emotion::Affectionate => "애정",
        Emotion::Cold => "냉담",
        Emotion::Embarrassed => "당황",
        Emotion::Guilty => "죄책감",
    }
}

/// Char-wise case-insensitive prefix check. Char-based (not byte-based) so a
/// latin name matches regardless of case without any multi-byte slicing.
fn starts_with_case_insensitive(text: &str, prefix: &str) -> bool {
    text.chars().count() >= prefix.chars().count()
        && text
            .chars()
            .zip(prefix.chars())
            .all(|(a, b)| a.to_lowercase().eq(b.to_lowercase()))
}

/// Join keys for the keys-editor prefill: commas and backslashes inside a
/// key are escaped, so the only bare commas in the edited string separate
/// keys and `\,` round-trips a literal comma.
fn join_lorebook_keys(keys: &[String]) -> String {
    keys.iter()
        .map(|key| key.replace('\\', "\\\\").replace(',', "\\,"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Parse the keys-editor text into keys: split on UNESCAPED commas only and
/// unescape on submit — `\,` is a literal comma, `\\` one backslash, any
/// other `\x` stays as typed. Items are trimmed; empty ones dropped. This is
/// what lets a key like `카페, 뒷문` survive the modal.
fn parse_lorebook_keys(text: &str) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for ch in text.chars() {
        if escaped {
            escaped = false;
            match ch {
                ',' | '\\' => current.push(ch),
                _ => {
                    current.push('\\');
                    current.push(ch);
                }
            }
        } else if ch == '\\' {
            escaped = true;
        } else if ch == ',' {
            let key = current.trim().to_string();
            if !key.is_empty() {
                keys.push(key);
            }
            current.clear();
        } else {
            current.push(ch);
        }
    }
    if escaped {
        // A dangling trailing backslash is kept literally.
        current.push('\\');
    }
    let key = current.trim().to_string();
    if !key.is_empty() {
        keys.push(key);
    }
    keys
}

/// Append the recycled-turn variant badge ("변형 2/3") as one subtle line
/// closing the narration blocks. `None` renders nothing — no badge, no row.
fn push_variant_badge(blocks: &mut Vec<StoryBlock<'_>>, label: Option<&str>) {
    if let Some(label) = label {
        blocks.push(StoryBlock::Text(std::borrow::Cow::Owned(vec![Line::from(
            Span::styled(label.to_string(), Styles::subtle()),
        )])));
    }
}

/// Append the host automation notice ("이전 세션을 이어요", "✎ 이어씀", …) as
/// one subtle tail line, reusing the variant-badge surface. `None` renders
/// nothing — no row, no layout shift.
fn push_story_notice(blocks: &mut Vec<StoryBlock<'_>>, notice: Option<&str>) {
    if let Some(notice) = notice {
        blocks.push(StoryBlock::Text(std::borrow::Cow::Owned(vec![Line::from(
            Span::styled(notice.to_string(), Styles::subtle()),
        )])));
    }
}

fn save_load_copy(state: &SaveLoadState) -> (&'static str, &'static str) {
    match (state.mode, state.overwrite_confirmation) {
        (SaveLoadMode::Save, true) => (
            "저장 / 덮어쓰기 확인",
            "Enter 한 번 더: 덮어쓰기 · Esc 취소",
        ),
        (SaveLoadMode::Save, false) => (
            "수동 저장",
            "↑↓ 이동 · Enter 저장 · S/L 화면 전환 · Esc 닫기",
        ),
        (SaveLoadMode::Load, _) => (
            "저장 불러오기",
            "↑↓ 이동 · Enter 불러오기 · S/L 화면 전환 · Esc 닫기",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
    use story_model::{
        CharacterId, CharacterStatusView, ChoiceView, LoadingState, RelationshipStage, RiskLevel,
        Screen, StatusLine, TimeSlot,
    };

    fn sample_view(choices: usize) -> AppViewModel {
        AppViewModel {
            screen: Screen::Story,
            story_title: "데모".to_string(),
            chapter: "ch-1".to_string(),
            location_display: "교실".to_string(),
            time_slot: TimeSlot::Morning,
            narration: "장면.".to_string(),
            choices: (0..choices)
                .map(|i| ChoiceView {
                    id: format!("c-{i}"),
                    label: format!("선택 {i}"),
                    risk: RiskLevel::Low,
                    preview: None,
                })
                .collect(),
            status: StatusLine {
                route: None,
                affection: Some(0),
                trust: Some(0),
                tension: Some(0),
                turn: 1,
            },
            loading: None,
            error: None,
            undo_checkpoint_id: None,
            variant_label: None,
            cast: Vec::new(),
            truncation: story_model::TruncationStatus::None,
            empty_side_result: false,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn save_slot(slot_id: &str, occupied: bool) -> SlotMeta {
        SlotMeta {
            slot_id: slot_id.to_string(),
            story_id: if occupied {
                "rainy-reunion".to_string()
            } else {
                String::new()
            },
            saved_at: story_model::chrono::Utc::now(),
            chapter_label: String::new(),
            scene_label: String::new(),
            description: None,
        }
    }

    #[test]
    fn move_selection_clamps_to_bounds() {
        let mut app = App::new(sample_view(3));
        app.handle_key(key(KeyCode::Down), None);
        assert_eq!(app.selected, 1);
        app.handle_key(key(KeyCode::Down), None);
        app.handle_key(key(KeyCode::Down), None);
        assert_eq!(app.selected, 2, "clamped to last");
        app.handle_key(key(KeyCode::Up), None);
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn enter_emits_submit_for_selected_choice() {
        let mut app = App::new(sample_view(3));
        app.selected = 1;
        let cmd = app.handle_key(key(KeyCode::Enter), None).expect("submit");
        let EngineCommand::SubmitChoice { choice_id } = cmd else {
            panic!("not submit: {cmd:?}");
        };
        assert_eq!(choice_id, "c-1");
    }

    #[test]
    fn number_row_two_emits_second_choice() {
        let mut app = App::new(sample_view(3));
        let cmd = app
            .handle_key(key(KeyCode::Char('2')), None)
            .expect("submit");
        let EngineCommand::SubmitChoice { choice_id } = cmd else {
            panic!("not submit: {cmd:?}");
        };
        assert_eq!(choice_id, "c-1", "'2' selects index 1");
    }

    #[test]
    fn save_shortcut_requests_host_to_open_manual_slots() {
        let mut app = App::new(sample_view(3));
        app.handle_key(key(KeyCode::Char('s')), None);
        assert_eq!(
            app.take_save_load_request(),
            Some(SaveLoadRequest::Open(SaveLoadMode::Save))
        );
    }

    #[test]
    fn occupied_slot_requires_second_enter_before_overwrite() {
        let mut app = App::new(sample_view(3));
        app.open_save_load(SaveLoadMode::Save, vec![save_slot("slot-1", true)]);
        app.handle_key(key(KeyCode::Enter), None);
        assert_eq!(app.take_save_load_request(), None);
        app.handle_key(key(KeyCode::Enter), None);
        assert_eq!(
            app.take_save_load_request(),
            Some(SaveLoadRequest::Save {
                slot_id: "slot-1".to_string()
            })
        );
    }

    #[test]
    fn empty_manual_slot_saves_on_first_enter() {
        let mut app = App::new(sample_view(3));
        app.open_save_load(SaveLoadMode::Save, vec![save_slot("slot-1", false)]);
        app.handle_key(key(KeyCode::Enter), None);
        assert_eq!(
            app.take_save_load_request(),
            Some(SaveLoadRequest::Save {
                slot_id: "slot-1".to_string()
            })
        );
    }

    #[test]
    fn switching_save_load_mode_requests_fresh_slots() {
        let mut app = App::new(sample_view(3));
        app.open_save_load(SaveLoadMode::Load, vec![save_slot("autosave", true)]);
        app.handle_key(key(KeyCode::Char('s')), None);
        assert_eq!(
            app.take_save_load_request(),
            Some(SaveLoadRequest::Open(SaveLoadMode::Save))
        );
    }

    #[test]
    fn paste_is_inserted_only_while_free_input_is_active() {
        let mut app = App::new(sample_view(3));
        app.handle_paste("무시");
        assert_eq!(app.input.text(), "");
        app.input_mode = true;
        app.handle_paste("안녕\n유나");
        assert_eq!(app.input.text(), "안녕유나");
    }

    #[test]
    fn error_overlay_blocks_auxiliary_screens_as_well_as_story_commands() {
        let mut app = App::new(sample_view(3));
        app.view.error = Some(story_model::DomainErrorView {
            code: "test".to_string(),
            title_ko: "오류".to_string(),
            message_ko: "재시도 필요".to_string(),
            recoverable: true,
        });
        for code in [
            KeyCode::Char('o'),
            KeyCode::Char('h'),
            KeyCode::Char('m'),
            KeyCode::Home,
        ] {
            assert_eq!(app.handle_key(key(code), None), None);
        }
        assert!(!app.settings_visible);
        assert!(app.list_mode.is_none());
        assert!(!app.home_visible);
    }

    #[test]
    fn committed_later_turn_follows_the_bottom_of_story_stack() {
        let mut app = App::new(sample_view(3));
        let mut next = sample_view(3);
        next.status.turn = 2;
        app.set_view(next);
        assert_eq!(app.narration_scroll, u16::MAX);
    }

    #[test]
    fn first_upward_wheel_event_leaves_follow_end_immediately() {
        let mut app = App::new(sample_view(3));
        app.narration_scroll = u16::MAX;
        app.narration_max_scroll.set(20);

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(
            app.narration_scroll, 17,
            "the first upward wheel event must leave follow-end, not count down from u16::MAX"
        );
    }

    #[test]
    fn loading_turn_does_not_move_choice_selection() {
        let mut app = App::new(sample_view(3));
        app.view.loading = Some(story_model::LoadingState {
            generation_id: Uuid::nil(),
            message_ko: "생성 중...".to_string(),
        });
        app.handle_key(key(KeyCode::Down), None);
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn cancel_resolves_to_pending_generation_id() {
        let mut app = App::new(sample_view(0));
        app.view.loading = Some(story_model::LoadingState {
            generation_id: Uuid::nil(),
            message_ko: "생성 중...".to_string(),
        });
        let gid = Uuid::new_v4();
        let cmd = app
            .handle_key(key(KeyCode::Char('c')), Some(gid))
            .expect("cancel");
        let EngineCommand::CancelGeneration { generation_id } = cmd else {
            panic!("not cancel: {cmd:?}");
        };
        assert_eq!(generation_id, gid);
    }

    #[test]
    fn writer_chunk_streams_then_clears_on_completed_view() {
        // Issue #2: MVP-path streaming reuses the WriterChunk →
        // streaming_narration render chain. Pin the app-side behavior:
        // append with a generation-id guard, render as a partial transcript
        // entry, discard stale chunks, and release on completion.
        let id = story_model::uuid::Uuid::nil();
        let mut loading_view = sample_view(2);
        loading_view.loading = Some(story_model::LoadingState {
            generation_id: id,
            message_ko: "장면을 쓰는 중...".to_string(),
        });
        let mut app = App::new(loading_view);

        // Matching id appends and becomes visible in the transcript blocks.
        app.push_writer_chunk(id, "비가 조금씩");
        app.push_writer_chunk(id, " 그쳤다.");
        assert_eq!(
            app.streaming_narration.as_deref(),
            Some("비가 조금씩 그쳤다.")
        );
        let blocks = app.story_blocks();
        let streaming_visible = blocks.iter().any(|b| match b {
            StoryBlock::Text(lines) => lines
                .iter()
                .any(|l| l.to_string().contains("비가 조금씩 그쳤다.")),
            _ => false,
        });
        assert!(
            streaming_visible,
            "partial narration must render in the transcript"
        );

        // A late chunk from a stale generation id is discarded.
        app.push_writer_chunk(story_model::uuid::Uuid::new_v4(), "늦은 청크");
        assert_eq!(
            app.streaming_narration.as_deref(),
            Some("비가 조금씩 그쳤다.")
        );

        // Completing the turn (loading cleared) releases the partial text.
        app.set_view(sample_view(2));
        assert_eq!(app.streaming_narration, None);
    }

    #[test]
    fn set_view_clamps_selection() {
        let mut app = App::new(sample_view(3));
        app.selected = 2;
        let mut v = sample_view(2);
        app.set_view(v.clone());
        assert_eq!(app.selected, 1, "clamped to new last index");
        v.choices.clear();
        app.set_view(v);
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn quit_flag_set_on_q() {
        let mut app = App::new(sample_view(3));
        app.handle_key(key(KeyCode::Char('q')), None);
        assert!(app.should_quit);
    }

    #[test]
    fn provider_picker_click_selects_a_profile() {
        let mut app = App::new(sample_view(0));
        app.set_providers(
            &[
                (
                    "omlx-local".to_string(),
                    "http://127.0.0.1:8000".to_string(),
                ),
                (
                    "openrouter".to_string(),
                    "https://openrouter.ai/api/v1".to_string(),
                ),
            ],
            "omlx-local",
        );
        app.handle_key(key(KeyCode::Char('o')), None);
        app.handle_key(key(KeyCode::Char('e')), None);

        // Hit tests map rows through `last_render_area`, which a real frame
        // sets. Simulate a 100x40 screen at the origin: the border occupies
        // row 0, so click row 2 is the SECOND profile, row 1 the first.
        app.last_render_area.set(Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 40,
        });
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row: 2,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.take_selected_provider().as_deref(), Some("openrouter"));

        // A click back on the first row selects the base profile again.
        app.handle_key(key(KeyCode::Char('e')), None);
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.take_selected_provider().as_deref(), Some("omlx-local"));
    }

    #[test]
    fn images_toggle_hides_blocks_without_clearing_the_map() {
        let mut app = App::new(sample_view(1));
        let initial = story_model::LocationId("cafe".to_string());
        let history = vec![history_entry(1, None)];
        app.set_lists(&history, &[], &initial);

        let picker = ratatui_image::picker::Picker::halfblocks();
        app.set_image_picker(Some(picker), false);
        app.set_location_image(
            "cafe",
            Some((
                std::path::PathBuf::from("/pack/cafe.png"),
                image::DynamicImage::new_rgb8(16, 16),
            )),
        );
        assert_eq!(
            image_block_count(&mut app),
            1,
            "enabled state shows the image"
        );

        // The Settings `i` key still drives the toggle request.
        app.handle_key(key(KeyCode::Char('o')), None);
        app.handle_key(key(KeyCode::Char('i')), None);
        assert!(app.take_images_toggle_request(), "toggle requested");

        // Toggling off hides the image block but keeps the map warm, so
        // re-enabling is instant (no re-decode, no host roundtrip).
        app.set_images_enabled(false);
        assert_eq!(
            image_block_count(&mut app),
            0,
            "disabled state hides blocks"
        );
        app.set_images_enabled(true);
        assert_eq!(image_block_count(&mut app), 1, "re-enable is instant");
    }

    #[test]
    fn expression_sprite_inserts_on_emotion_change_and_dedupes() {
        let mut app = halfblocks_app(1);
        app.set_expression_image(
            Emotion::Happy,
            Some((
                std::path::PathBuf::from("/pack/expressions/happy.png"),
                image::DynamicImage::new_rgb8(16, 16),
            )),
        );
        app.set_expression_image(
            Emotion::Sad,
            Some((
                std::path::PathBuf::from("/pack/expressions/sad.png"),
                image::DynamicImage::new_rgb8(16, 16),
            )),
        );

        // Three turns: happy → sad → sad again. The repeat must not re-insert.
        let mut first = history_entry(1, None);
        first.narration = "유나는 환한 미소를 지었다.".to_string();
        let mut second = history_entry(2, None);
        second.narration = "그녀의 눈에 눈물이 맺혔다.".to_string();
        let mut third = history_entry(3, None);
        third.narration = "또다시 눈물이 맺혔다.".to_string();
        app.set_lists(&[first, second, third], &[], &default_location());

        assert_eq!(
            image_block_count(&mut app),
            2,
            "happy + sad only; the consecutive sad dedupes"
        );

        // The `f` toggle hides sprite blocks but keeps the map warm.
        app.set_expressions_enabled(false);
        assert_eq!(image_block_count(&mut app), 0, "disabled hides sprites");
        app.set_expressions_enabled(true);
        assert_eq!(image_block_count(&mut app), 2, "re-enable is instant");
    }

    #[test]
    fn expression_sprite_shares_the_scene_dedupe_key() {
        let mut app = halfblocks_app(1);
        // Portrait fallback: the scene boundary AND the happy sprite resolve
        // to the SAME file — only one may render.
        let portrait = std::path::PathBuf::from("/pack/characters/yuna.png");
        app.set_location_image(
            "cafe",
            Some((portrait.clone(), image::DynamicImage::new_rgb8(16, 16))),
        );
        app.set_expression_image(
            Emotion::Happy,
            Some((portrait, image::DynamicImage::new_rgb8(16, 16))),
        );

        let mut turn = history_entry(1, Some("cafe"));
        turn.narration = "유나는 환한 미소를 지었다.".to_string();
        app.set_lists(&[turn], &[], &default_location());

        assert_eq!(
            image_block_count(&mut app),
            1,
            "the portrait-fallback sprite must not repeat the scene image"
        );
    }

    #[test]
    fn opening_narration_drives_the_first_expression_beat() {
        let mut app = halfblocks_app(1);
        app.set_expression_image(
            Emotion::Sad,
            Some((
                std::path::PathBuf::from("/pack/expressions/sad.png"),
                image::DynamicImage::new_rgb8(16, 16),
            )),
        );
        app.view.narration = "그녀의 눈에 눈물이 맺혔다.".to_string();
        assert_eq!(
            image_block_count(&mut app),
            1,
            "the empty-stack opening classifies its narration"
        );
    }

    #[test]
    fn settings_f_key_requests_the_expression_toggle_once() {
        let mut app = halfblocks_app(1);
        app.handle_key(key(KeyCode::Char('o')), None);
        app.handle_key(key(KeyCode::Char('f')), None);
        assert!(app.take_expressions_toggle_request(), "toggle requested");
        assert!(
            !app.take_expressions_toggle_request(),
            "request is consumed"
        );

        // Korean 2-beolsik ㄹ hits the same physical key branch.
        app.handle_key(key(KeyCode::Char('ㄹ')), None);
        assert!(app.take_expressions_toggle_request());
    }

    #[test]
    fn settings_b_key_requests_the_backend_cycle_once() {
        let mut app = halfblocks_app(1);
        app.handle_key(key(KeyCode::Char('o')), None);
        app.handle_key(key(KeyCode::Char('b')), None);
        assert!(app.take_image_backend_cycle_request(), "cycle requested");
        assert!(
            !app.take_image_backend_cycle_request(),
            "request is consumed"
        );

        // Korean 2-beolsik ㅠ hits the same physical key branch.
        app.handle_key(key(KeyCode::Char('ㅠ')), None);
        assert!(app.take_image_backend_cycle_request());
    }

    #[test]
    fn settings_screen_renders_the_forced_backend_marker() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use unicode_width::UnicodeWidthStr;

        let mut app = halfblocks_app(1);
        // Player-forced protocol (Settings `b` cycle): the marker must reach
        // the screen through the real App → SettingsView projection, not a
        // hand-built view.
        app.set_image_picker(Some(ratatui_image::picker::Picker::halfblocks()), true);
        app.set_settings(
            ProviderConfigSafe {
                base_url: "http://127.0.0.1:8000".to_string(),
                model: "a".to_string(),
                director_model: None,
                choice_model: None,
                writer_model: None,
                structured_output: "auto".to_string(),
                request_timeout_ms: 1_000,
                connect_timeout_ms: 1_000,
                max_retries: 0,
                is_loopback: true,
            },
            ProviderHealth::Ready,
            0.8,
            0.9,
            40,
            1_800,
            TtsDisplay {
                enabled: true,
                backend: "say".to_string(),
                voice: None,
            },
            true,
            app.image_backend_label(),
            false,
            false,
            true,
            true,
            "자동".to_string(),
            "없음".to_string(),
            String::new(),
            String::new(),
        );
        app.settings_visible = true;

        let mut terminal = Terminal::new(TestBackend::new(100, 40)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame, frame.area()))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            let mut previous_was_wide = false;
            for x in 0..buffer.area.width {
                let symbol = buffer[(x, y)].symbol();
                if previous_was_wide && symbol == " " {
                    previous_was_wide = false;
                    continue;
                }
                previous_was_wide = symbol.width() == 2;
                out.push_str(symbol);
            }
            out.push('\n');
        }
        assert!(
            out.contains("halfblocks (수동)"),
            "forced-protocol marker missing from the settings screen:\n{out}"
        );
    }

    /// An App with a settings projection in place, so the settings key
    /// branch (and the persona modal prefill) has data to work with.
    fn settings_app() -> App {
        let mut app = halfblocks_app(1);
        app.set_settings(
            ProviderConfigSafe {
                base_url: "http://127.0.0.1:8000".to_string(),
                model: "a".to_string(),
                director_model: None,
                choice_model: None,
                writer_model: None,
                structured_output: "auto".to_string(),
                request_timeout_ms: 1_000,
                connect_timeout_ms: 1_000,
                max_retries: 0,
                is_loopback: true,
            },
            ProviderHealth::Ready,
            0.85,
            0.92,
            40,
            1_800,
            TtsDisplay {
                enabled: false,
                backend: "say".to_string(),
                voice: None,
            },
            true,
            app.image_backend_label(),
            false,
            false,
            true,
            true,
            "자동".to_string(),
            "없음".to_string(),
            "민준".to_string(),
            String::new(),
        );
        app.settings_visible = true;
        app
    }

    #[test]
    fn display_rules_rewrite_transcript_display_only() {
        let mut app = App::new(sample_view(1));
        app.set_display_rules(vec![(
            regex::Regex::new(r"\d+").expect("pattern"),
            "N".to_string(),
        )]);

        // The direct text path (opening turn / streaming) applies the rules.
        assert_eq!(app.display_text("42개의 숫자 7"), "N개의 숫자 N");
        // No rules = borrowed passthrough.
        let bare = App::new(sample_view(1));
        assert!(matches!(
            bare.display_text("그대로"),
            std::borrow::Cow::Borrowed("그대로")
        ));

        // The cached transcript path applies them too — but the stack entry
        // itself stays authoritative.
        app.replace_story_stack(vec![StoryStackEntry {
            turn: 1,
            scene_id: "scene-1".to_string(),
            model: None,
            generation_secs: None,
            player_action: Some("문을 연다".to_string()),
            narration: "온도는 21도였다.".to_string(),
            location: Some("교실".to_string()),
            emotion: Emotion::Neutral,
        }]);
        app.fill_block_cache();
        let cache = &app.stack_lines_cache.as_ref().expect("cache").1;
        let rendered: String = cache[0]
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<String>();
        assert!(
            rendered.contains("온도는 N도였다"),
            "rules applied: {rendered}"
        );
        assert!(!rendered.contains("21"));
        assert_eq!(app.story_stack[0].narration, "온도는 21도였다.");
    }

    #[test]
    fn history_items_get_display_rules_applied() {
        let mut app = App::new(sample_view(1));
        app.set_display_rules(vec![(
            regex::Regex::new("유나").expect("pattern"),
            "주인공".to_string(),
        )]);
        let mut entry = history_entry(1, None);
        entry.narration = "유나는 창가에 앉았다.".to_string();
        app.set_lists(&[entry], &[], &default_location());

        // The `h` list row goes through the same display boundary.
        assert!(
            app.history_items[0].contains("주인공는 창가에 앉았다"),
            "rules applied to the history row: {:?}",
            app.history_items[0]
        );
        assert!(!app.history_items[0].contains("유나"));
        // The transcript stack keeps the raw narration — rules are display
        // only.
        assert_eq!(app.story_stack[0].narration, "유나는 창가에 앉았다.");
    }

    #[test]
    fn lorebook_edits_wait_out_a_generation_before_reaching_the_host() {
        let mut app = settings_app();
        app.set_lorebook_data(&[lore_entry("e1", &["카페"], "오래된 피아노")], false);
        app.handle_key(key(KeyCode::Char('w')), None);
        app.handle_key(key(KeyCode::Char(' ')), None);
        app.handle_key(key(KeyCode::Esc), None);

        // While a turn generates, the engine would drop the hot-swap — the
        // host must not see the edit yet, and it must not be lost either.
        let mut view = sample_view(1);
        view.loading = Some(LoadingState {
            generation_id: Uuid::nil(),
            message_ko: "생성 중".to_string(),
        });
        app.view = view;
        assert!(
            app.take_lorebook_request().is_none(),
            "loading holds the edit"
        );

        // Settled: the same edit reaches the host exactly once.
        app.view = sample_view(1);
        let entries = app.take_lorebook_request().expect("edit survives loading");
        assert!(!entries[0].enabled);
        assert!(app.take_lorebook_request().is_none(), "consumed once");
    }

    #[test]
    fn display_rules_cover_choice_labels_and_previews() {
        let mut app = App::new(sample_view(1));
        app.set_display_rules(vec![(
            regex::Regex::new(r"\d+").expect("pattern"),
            "N".to_string(),
        )]);
        let mut view = sample_view(2);
        view.choices[1].preview = Some("3번 문으로".to_string());
        app.set_view(view);
        assert_eq!(app.view.choices[0].label, "선택 N");
        assert_eq!(app.view.choices[1].label, "선택 N");
        assert_eq!(app.view.choices[1].preview.as_deref(), Some("N번 문으로"));
    }

    #[test]
    fn prompt_viewer_scroll_clamp_counts_wrapped_rows() {
        let mut app = App::new(sample_view(1));
        // One long user line that wraps far beyond the popup width.
        let long = "긴 문장이 이어집니다 ".repeat(200);
        app.set_prompt_preview("시스템 지침", &long);
        app.open_prompt_viewer();
        assert!(app.prompt_viewer.is_some());
        // The clamp reads the last render area for the popup geometry — the
        // standard 80×24 screen here.
        app.last_render_area
            .set(ratatui::layout::Rect::new(0, 0, 80, 24));

        for _ in 0..60 {
            app.scroll_prompt_viewer(10);
        }
        let (width, height) =
            crate::screens::settings::prompt_viewer_frame(ratatui::layout::Rect::new(0, 0, 80, 24));
        let lines = crate::screens::settings::prompt_viewer_body_lines("시스템 지침", &long);
        let total_rows =
            crate::screens::story::wrapped_line_count(&lines, width.saturating_sub(2)) as i32;
        let expected_max = total_rows.saturating_sub(height.saturating_sub(2) as i32);
        assert!(
            expected_max > lines.len() as i32,
            "wrapped rows exceed logical rows in this scenario"
        );
        assert_eq!(
            app.prompt_viewer.unwrap(),
            expected_max as u16,
            "scrolling reaches the wrapped tail, not the logical-line count"
        );
        // Scrolling back up still bottoms out at zero.
        for _ in 0..60 {
            app.scroll_prompt_viewer(-30);
        }
        assert_eq!(app.prompt_viewer.unwrap(), 0);
    }

    #[test]
    fn settings_x_cycles_the_sampling_style_request() {
        let mut app = settings_app();
        app.handle_key(key(KeyCode::Char('x')), None);
        assert!(
            app.take_sampling_style_cycle_request(),
            "x requests a cycle"
        );
        assert!(
            !app.take_sampling_style_cycle_request(),
            "request is consumed"
        );

        // Korean 2-beolsik ㅌ hits the same physical key branch.
        app.handle_key(key(KeyCode::Char('ㅌ')), None);
        assert!(app.take_sampling_style_cycle_request());
    }

    #[test]
    fn settings_a_opens_the_persona_dialog_prefilled() {
        let mut app = settings_app();
        app.handle_key(key(KeyCode::Char('a')), None);
        // The modal prefill comes from the effective identity, not blanks.
        app.handle_key(key(KeyCode::Enter), None);
        assert_eq!(
            app.take_persona_submit_request(),
            Some(("민준".to_string(), String::new())),
            "submitting untouched keeps the shown identity"
        );

        app.handle_key(key(KeyCode::Char('a')), None);
        app.handle_key(key(KeyCode::Esc), None);
        assert_eq!(app.take_persona_submit_request(), None, "Esc cancels");
    }

    #[test]
    fn persona_dialog_edits_focuses_and_submits_raw_text() {
        let mut app = settings_app();
        app.handle_key(key(KeyCode::Char('a')), None);
        // Replace the prefilled name: backspace it away, then type a new one.
        app.handle_key(key(KeyCode::Backspace), None);
        app.handle_key(key(KeyCode::Backspace), None);
        app.handle_key(key(KeyCode::Char('레')), None);
        app.handle_key(key(KeyCode::Char('오')), None);
        app.handle_key(key(KeyCode::Tab), None);
        app.handle_key(key(KeyCode::Char('조')), None);
        app.handle_key(key(KeyCode::Char('용')), None);
        app.handle_key(key(KeyCode::Enter), None);
        assert_eq!(
            app.take_persona_submit_request(),
            Some(("레오".to_string(), "조용".to_string())),
        );
        assert!(
            app.persona_dialog.is_none(),
            "the modal closes after a submit"
        );
    }

    #[test]
    fn persona_dialog_absorbs_other_settings_keys_and_paste() {
        let mut app = settings_app();
        app.handle_key(key(KeyCode::Char('a')), None);
        // While the modal is open, settings shortcuts must not fire — their
        // characters land in the focused text field instead.
        app.handle_key(key(KeyCode::Char('i')), None);
        assert!(!app.take_images_toggle_request());
        app.handle_key(key(KeyCode::Char('x')), None);
        assert!(!app.take_sampling_style_cycle_request());
        // Paste lands in the focused field too.
        app.handle_paste("관찰자");
        app.handle_key(key(KeyCode::Tab), None);
        app.handle_key(key(KeyCode::Enter), None);
        assert_eq!(
            app.take_persona_submit_request(),
            Some(("민준ix관찰자".to_string(), String::new())),
        );
    }

    #[test]
    fn persona_modal_renders_over_the_settings_screen() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use unicode_width::UnicodeWidthStr;

        let mut app = settings_app();
        app.handle_key(key(KeyCode::Char('a')), None);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame, frame.area()))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            let mut previous_was_wide = false;
            for x in 0..buffer.area.width {
                let symbol = buffer[(x, y)].symbol();
                if previous_was_wide && symbol == " " {
                    previous_was_wide = false;
                    continue;
                }
                previous_was_wide = symbol.width() == 2;
                out.push_str(symbol);
            }
            out.push('\n');
        }
        assert!(
            out.contains("페르소나") && out.contains("이름"),
            "modal labels missing:\n{out}"
        );
        assert!(out.contains("▌"), "focused cursor cell missing:\n{out}");
    }

    /// Render the current app state at 80×24 into a plain string (wide-char
    /// aware), for modal render assertions.
    fn render_to_string(app: &mut App) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use unicode_width::UnicodeWidthStr;

        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame, frame.area()))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            let mut previous_was_wide = false;
            for x in 0..buffer.area.width {
                let symbol = buffer[(x, y)].symbol();
                if previous_was_wide && symbol == " " {
                    previous_was_wide = false;
                    continue;
                }
                previous_was_wide = symbol.width() == 2;
                out.push_str(symbol);
            }
            out.push('\n');
        }
        out
    }

    /// A minimal lorebook entry for dialog tests — only the edited fields
    /// (keys/content/enabled) carry meaningful values.
    fn lore_entry(id: &str, keys: &[&str], content: &str) -> story_model::LorebookEntry {
        story_model::LorebookEntry {
            id: id.to_string(),
            keys: keys.iter().map(|key| key.to_string()).collect(),
            secondary_keys: Vec::new(),
            logic: story_model::LorebookLogic::AndAny,
            content: content.to_string(),
            constant: false,
            enabled: true,
            insertion_order: 100,
            position: story_model::LorebookPosition::BeforeChar,
            case_sensitive: false,
            probability: 100,
            priority: None,
            comment: None,
            prevent_recursion: false,
            exclude_recursion: false,
            sticky: 0,
            cooldown: 0,
            delay: 0,
            vectorized: false,
        }
    }

    #[test]
    fn story_w_requests_continue_when_idle_and_is_absorbed_while_loading() {
        let mut app = App::new(sample_view(2));
        app.handle_key(key(KeyCode::Char('w')), None);
        assert!(app.take_continue_request(), "w requests a continue");
        assert!(!app.take_continue_request(), "request is consumed");
        // Korean 2-beolsik ㅈ hits the same physical key branch.
        app.handle_key(key(KeyCode::Char('ㅈ')), None);
        assert!(app.take_continue_request());

        // While a turn is in flight the key must be absorbed, not queued.
        let mut view = sample_view(2);
        view.loading = Some(LoadingState {
            generation_id: Uuid::nil(),
            message_ko: "생성 중".to_string(),
        });
        let mut app = App::new(view);
        app.handle_key(key(KeyCode::Char('w')), None);
        assert!(!app.take_continue_request(), "loading absorbs w");
    }

    #[test]
    fn story_z_and_alt_digit_request_suggest_and_quick_actions() {
        let mut app = App::new(sample_view(3));
        app.handle_key(key(KeyCode::Char('z')), None);
        assert!(app.take_suggest_action_request(), "z requests a suggest");
        // Korean 2-beolsik ㅋ hits the same physical key branch.
        app.handle_key(key(KeyCode::Char('ㅋ')), None);
        assert!(app.take_suggest_action_request());

        // Alt+1 names quick-action slot 1 for the host to fill.
        let alt_one = KeyEvent::new(KeyCode::Char('1'), KeyModifiers::ALT);
        app.handle_key(alt_one, None);
        assert_eq!(app.take_quick_action_request().as_deref(), Some("1"));
        assert_eq!(app.take_quick_action_request(), None, "consumed once");

        // A bare digit stays a direct choice confirm — no quick action.
        let command = app.handle_key(key(KeyCode::Char('1')), None);
        assert!(matches!(command, Some(EngineCommand::SubmitChoice { .. })));
        assert_eq!(app.take_quick_action_request(), None);
    }

    fn cast_member(id: &str, name: &str) -> CharacterStatusView {
        CharacterStatusView {
            id: CharacterId(id.to_string()),
            name: name.to_string(),
            relationship_stage: RelationshipStage::Acquaintance,
            current_emotion: Emotion::Neutral,
            affection: 5,
            trust: 1,
            tension: 8,
        }
    }

    #[test]
    fn story_g_requests_group_scene_when_idle_and_is_absorbed_while_loading() {
        let mut app = App::new(sample_view(2));
        app.handle_key(key(KeyCode::Char('g')), None);
        assert!(app.take_group_scene_request(), "g requests a group scene");
        assert!(!app.take_group_scene_request(), "request is consumed once");
        // Korean 2-beolsik ㅎ hits the same physical key branch.
        app.handle_key(key(KeyCode::Char('ㅎ')), None);
        assert!(app.take_group_scene_request());

        // While a turn is in flight the key must be absorbed, not queued.
        let mut view = sample_view(2);
        view.loading = Some(LoadingState {
            generation_id: Uuid::nil(),
            message_ko: "생성 중".to_string(),
        });
        let mut app = App::new(view);
        app.handle_key(key(KeyCode::Char('g')), None);
        assert!(!app.take_group_scene_request(), "loading absorbs g");
    }

    #[test]
    fn story_p_opens_the_cast_roster_overlay_and_esc_closes_it() {
        let mut app = App::new(sample_view(2));
        app.view.cast = vec![cast_member("yuna", "한유나"), cast_member("namu", "나무")];

        app.handle_key(key(KeyCode::Char('p')), None);
        assert_eq!(app.list_mode, Some(ListMode::Cast));
        let rows = app.cast_roster_rows();
        assert_eq!(
            rows[0], "한유나 · 지인 · 중립 · 호감 5 / 신뢰 1 / 긴장 8",
            "roster rows are 이름 · 관계단계 · 감정 · 호감/신뢰/긴장: {rows:?}"
        );

        app.handle_key(key(KeyCode::Esc), None);
        assert_eq!(app.list_mode, None, "Esc closes the roster");

        // Korean 2-beolsik ㅔ hits the same physical key branch.
        app.handle_key(key(KeyCode::Char('ㅔ')), None);
        assert_eq!(app.list_mode, Some(ListMode::Cast));

        // While a turn is in flight the key is absorbed, like w/z.
        let mut view = sample_view(2);
        view.loading = Some(LoadingState {
            generation_id: Uuid::nil(),
            message_ko: "생성 중".to_string(),
        });
        let mut app = App::new(view);
        app.handle_key(key(KeyCode::Char('p')), None);
        assert_eq!(app.list_mode, None, "loading absorbs p");
    }

    #[test]
    fn at_mention_free_input_pins_the_longest_cast_target_and_preserves_the_original_text() {
        let mut app = App::new(sample_view(2));
        app.view.cast = vec![
            cast_member("yuna", "한유나"),
            cast_member("short", "유나"),
            cast_member("namu", "나무"),
            cast_member("rin", "Rin"),
        ];

        // Submit `@한유나 안녕` via Enter in input mode.
        app.input_mode = true;
        app.input.clear();
        app.input.insert_str("@한유나 안녕");
        let command = app.handle_key(key(KeyCode::Enter), None);
        let Some(EngineCommand::SubmitFreeAction { action }) = command else {
            panic!("expected SubmitFreeAction, got {command:?}");
        };
        assert_eq!(
            action.target.as_ref().map(|t| t.0.as_str()),
            Some("yuna"),
            "the longest matching cast member wins (한유나 over 유나)"
        );
        assert_eq!(action.original_text, "@한유나 안녕", "verbatim input");
        assert_eq!(
            action.content, "안녕",
            "the mention is stripped from content"
        );

        // A shorter mention targets the shorter member.
        app.input_mode = true;
        app.input.clear();
        app.input.insert_str("@유나 안녕");
        let command = app.handle_key(key(KeyCode::Enter), None);
        let Some(EngineCommand::SubmitFreeAction { action }) = command else {
            panic!("expected SubmitFreeAction, got {command:?}");
        };
        assert_eq!(action.target.as_ref().map(|t| t.0.as_str()), Some("short"));

        // Latin names match case-insensitively.
        app.input_mode = true;
        app.input.clear();
        app.input.insert_str("@rin 앉아");
        let command = app.handle_key(key(KeyCode::Enter), None);
        let Some(EngineCommand::SubmitFreeAction { action }) = command else {
            panic!("expected SubmitFreeAction, got {command:?}");
        };
        assert_eq!(action.target.as_ref().map(|t| t.0.as_str()), Some("rin"));
        assert_eq!(action.original_text, "@rin 앉아");

        // No match keeps today's shape: target None, content = trimmed text.
        app.input_mode = true;
        app.input.clear();
        app.input.insert_str("@없는이 안녕");
        let command = app.handle_key(key(KeyCode::Enter), None);
        let Some(EngineCommand::SubmitFreeAction { action }) = command else {
            panic!("expected SubmitFreeAction, got {command:?}");
        };
        assert!(action.target.is_none(), "unknown @name has no target");
        assert_eq!(action.original_text, "@없는이 안녕");
        assert_eq!(action.content, "@없는이 안녕");

        // Plain free input without @ is unchanged too.
        app.input_mode = true;
        app.input.clear();
        app.input.insert_str("기다려 본다");
        let command = app.handle_key(key(KeyCode::Enter), None);
        let Some(EngineCommand::SubmitFreeAction { action }) = command else {
            panic!("expected SubmitFreeAction, got {command:?}");
        };
        assert!(action.target.is_none());
        assert_eq!(action.original_text, "기다려 본다");
        assert_eq!(action.content, "기다려 본다");
    }

    #[test]
    fn story_brackets_request_variant_cycles_only_while_idle() {
        let mut app = App::new(sample_view(2));
        app.handle_key(key(KeyCode::Char('[')), None);
        assert!(
            app.take_cycle_variant_back_request(),
            "[ requests the previous variant"
        );
        assert!(
            !app.take_cycle_variant_back_request(),
            "request is consumed"
        );
        app.handle_key(key(KeyCode::Char(']')), None);
        assert!(
            app.take_cycle_variant_forward_request(),
            "] requests the next variant"
        );
        assert!(
            !app.take_cycle_variant_forward_request(),
            "request is consumed"
        );

        // While a turn is in flight the keys are absorbed, like w/z — the
        // engine would reject a cycle mid-generation anyway.
        let mut view = sample_view(2);
        view.loading = Some(LoadingState {
            generation_id: Uuid::nil(),
            message_ko: "생성 중".to_string(),
        });
        let mut app = App::new(view);
        app.handle_key(key(KeyCode::Char('[')), None);
        app.handle_key(key(KeyCode::Char(']')), None);
        assert!(!app.take_cycle_variant_back_request(), "loading absorbs [");
        assert!(
            !app.take_cycle_variant_forward_request(),
            "loading absorbs ]"
        );
    }

    #[test]
    fn variant_badge_renders_from_the_view_label_and_vanishes_without_it() {
        let mut app = App::new(sample_view(1));
        assert!(
            !blocks_text(&app).contains("변형"),
            "no label, no badge row"
        );

        // The engine labels the recycled attempt now on display; the badge
        // must show it verbatim.
        app.view.variant_label = Some("변형 2/3".to_string());
        assert!(
            blocks_text(&app).contains("변형 2/3"),
            "badge missing with a label set: {}",
            blocks_text(&app)
        );

        // A committed canonical turn clears the label and the badge.
        app.view.variant_label = None;
        assert!(!blocks_text(&app).contains("변형"));
    }

    #[test]
    fn story_notice_renders_at_80x24_without_clipping() {
        // The full automation guide must survive the minimum size — it is
        // the only feedback the auto-resume/auto-retry paths give.
        let notices = [
            "이전 세션을 이어요 — Alt+Home 홈",
            "자동 재시도…(1/1)",
            "잘림 — w로 이어쓰기",
            "✎ 이어씀",
            "빈 결과 — 다시 시도",
        ];
        for notice in notices {
            let mut app = App::new(sample_view(1));
            app.set_story_notice(Some(notice));
            let out = render_to_string(&mut app);
            assert!(
                out.contains(notice),
                "notice '{notice}' clipped or missing at 80x24:\n{out}"
            );
            assert_eq!(app.story_notice(), Some(notice));
        }
        // Cleared notice renders nothing.
        let mut app = App::new(sample_view(1));
        app.set_story_notice(Some("이전 세션을 이어요 — Alt+Home 홈"));
        app.set_story_notice(None);
        assert!(
            !blocks_text(&app).contains("이전 세션"),
            "no notice, no row"
        );
    }

    #[test]
    fn story_notice_coexists_with_the_variant_badge_and_clears_on_new_turn() {
        let mut app = App::new(sample_view(1));
        app.view.variant_label = Some("변형 2/3".to_string());
        app.set_story_notice(Some("✎ 이어씀"));
        let text = blocks_text(&app);
        assert!(text.contains("변형 2/3"), "{text}");
        assert!(text.contains("✎ 이어씀"), "{text}");

        // A later committed turn retires the notice (set_view contract).
        let mut next = sample_view(1);
        next.status.turn = 2;
        app.set_view(next);
        assert_eq!(app.story_notice(), None);
        assert!(!blocks_text(&app).contains("✎ 이어씀"));
    }

    #[test]
    fn settings_t_opens_the_author_note_dialog_prefilled() {
        let mut app = settings_app();
        app.set_author_note("차분한 분위기를 유지한다");
        app.handle_key(key(KeyCode::Char('t')), None);
        // Enter on the untouched modal keeps the shown note.
        app.handle_key(key(KeyCode::Enter), None);
        assert_eq!(
            app.take_author_note_submit(),
            Some("차분한 분위기를 유지한다".to_string()),
            "submitting untouched keeps the shown note"
        );

        // Korean 2-beolsik ㅅ opens the same modal; typed text is submitted
        // trimmed. The prefill still mirrors the host-side note until the
        // host applies the submission.
        app.set_author_note("");
        app.handle_key(key(KeyCode::Char('ㅅ')), None);
        assert!(app.author_note_dialog.is_some(), "ㅅ opens the modal");
        app.handle_key(key(KeyCode::Char('밤')), None);
        app.handle_key(key(KeyCode::Enter), None);
        assert_eq!(app.take_author_note_submit(), Some("밤".to_string()));

        // An empty submit means "clear the note".
        app.handle_key(key(KeyCode::Char('t')), None);
        app.handle_key(key(KeyCode::Enter), None);
        assert_eq!(
            app.take_author_note_submit(),
            Some(String::new()),
            "empty submit clears the note"
        );

        // Esc cancels.
        app.handle_key(key(KeyCode::Char('t')), None);
        app.handle_key(key(KeyCode::Esc), None);
        assert_eq!(app.take_author_note_submit(), None, "Esc cancels");
    }

    #[test]
    fn settings_w_opens_the_lorebook_dialog_and_submits_dirty_edits() {
        let mut app = settings_app();
        // Before the host supplies data, `w` is a no-op.
        app.handle_key(key(KeyCode::Char('w')), None);
        assert!(app.lorebook_dialog.is_none(), "no data, no dialog");

        app.set_lorebook_data(
            &[
                lore_entry("e1", &["성별", "gender"], "플레이어는 여성이다"),
                lore_entry("e2", &["카페"], "카페에는 오래된 피아노가 있다"),
            ],
            false,
        );
        app.handle_key(key(KeyCode::Char('w')), None);
        assert!(app.lorebook_dialog.is_some());

        // Space toggles the first entry off; Enter opens the content editor
        // for the (moved) second entry and submits an edit into it.
        app.handle_key(key(KeyCode::Char(' ')), None);
        app.handle_key(key(KeyCode::Down), None);
        app.handle_key(key(KeyCode::Enter), None);
        assert!(
            app.lorebook_dialog
                .as_ref()
                .is_some_and(|d| d.edit.is_some())
        );
        app.handle_key(key(KeyCode::End), None);
        app.handle_key(key(KeyCode::Char('수')), None);
        app.handle_key(key(KeyCode::Char('정')), None);
        app.handle_key(key(KeyCode::Enter), None);
        assert!(
            app.lorebook_dialog
                .as_ref()
                .is_some_and(|d| d.edit.is_none())
        );

        // Leaving the list while dirty hands over the FULL updated list.
        app.handle_key(key(KeyCode::Esc), None);
        assert!(app.lorebook_dialog.is_none(), "Esc closes the list");
        let request = app.take_lorebook_request().expect("dirty close submits");
        assert_eq!(request.len(), 2, "the full list is returned");
        assert!(!request[0].enabled, "Space toggled enabled off");
        assert_eq!(request[0].content, "플레이어는 여성이다");
        assert_eq!(request[1].content, "카페에는 오래된 피아노가 있다수정");
        assert_eq!(app.take_lorebook_request(), None, "consumed once");

        // A clean close (no edits) submits nothing.
        app.handle_key(key(KeyCode::Char('w')), None);
        app.handle_key(key(KeyCode::Esc), None);
        assert_eq!(app.take_lorebook_request(), None, "clean close is silent");
    }

    #[test]
    fn lorebook_keys_edit_splits_on_commas_and_keeps_other_fields() {
        let mut entry = lore_entry("e1", &["성별", "gender"], "플레이어는 여성이다");
        entry.constant = true;
        entry.insertion_order = 42;
        let mut app = settings_app();
        app.set_lorebook_data(&[entry], true);
        app.handle_key(key(KeyCode::Char('w')), None);
        app.handle_key(key(KeyCode::Char('k')), None);
        // Clear the prefilled keys, then type a comma-separated list with
        // stray spaces and an empty item.
        for _ in 0.."성별, gender".chars().count() {
            app.handle_key(key(KeyCode::Backspace), None);
        }
        for ch in "이름, 소속 ,".chars() {
            app.handle_key(key(KeyCode::Char(ch)), None);
        }
        app.handle_key(key(KeyCode::Enter), None);
        app.handle_key(key(KeyCode::Esc), None);
        let request = app.take_lorebook_request().expect("keys edit is dirty");
        assert_eq!(request.len(), 1);
        assert_eq!(
            request[0].keys,
            vec!["이름".to_string(), "소속".to_string()]
        );
        // Untouched fields ride along unchanged.
        assert!(request[0].constant);
        assert_eq!(request[0].insertion_order, 42);
        assert_eq!(request[0].content, "플레이어는 여성이다");
        assert!(request[0].enabled);
    }

    #[test]
    fn lorebook_keys_edit_round_trips_literal_commas() {
        // A key CONTAINING a comma must survive open → submit untouched.
        let entry = lore_entry("e1", &["카페, 뒷문", "정원"], "숨은 통로");
        let mut app = settings_app();
        app.set_lorebook_data(&[entry], true);
        app.handle_key(key(KeyCode::Char('w')), None);
        app.handle_key(key(KeyCode::Char('k')), None);
        // The prefill escapes the embedded comma so it is not a separator.
        assert!(
            app.lorebook_dialog
                .as_ref()
                .is_some_and(|dialog| dialog.edit.as_ref().is_some_and(|edit| matches!(
                    edit,
                    LorebookEdit::Keys { buffer }
                        if buffer.text() == "카페\\, 뒷문, 정원"
                ))),
            "prefill escapes the literal comma"
        );
        app.handle_key(key(KeyCode::Enter), None);
        app.handle_key(key(KeyCode::Esc), None);
        let request = app.take_lorebook_request().expect("untouched submit");
        assert_eq!(
            request[0].keys,
            vec!["카페, 뒷문".to_string(), "정원".to_string()],
            "the comma key survives the modal"
        );

        // Typed escapes: `\,` stays inside its key, a bare comma separates.
        let mut app = settings_app();
        app.set_lorebook_data(&[lore_entry("e1", &["기존"], "교체 대상")], true);
        app.handle_key(key(KeyCode::Char('w')), None);
        app.handle_key(key(KeyCode::Char('k')), None);
        for _ in 0.."기존".chars().count() {
            app.handle_key(key(KeyCode::Backspace), None);
        }
        for ch in "뒷문\\, 지하\\, B1 , 창고".chars() {
            app.handle_key(key(KeyCode::Char(ch)), None);
        }
        app.handle_key(key(KeyCode::Enter), None);
        app.handle_key(key(KeyCode::Esc), None);
        let request = app.take_lorebook_request().expect("typed escape submits");
        assert_eq!(
            request[0].keys,
            vec!["뒷문, 지하, B1".to_string(), "창고".to_string()]
        );
    }

    #[test]
    fn lorebook_c_toggles_constant_and_p_cycles_probability_as_dirty_edits() {
        let mut app = settings_app();
        app.set_lorebook_data(&[lore_entry("e1", &["카페"], "오래된 피아노")], true);
        app.handle_key(key(KeyCode::Char('w')), None);

        // `p` walks the ladder 100 → 75, `c` pins the entry; the Korean
        // 2-beolsik aliases hit the same arms.
        app.handle_key(key(KeyCode::Char('p')), None);
        app.handle_key(key(KeyCode::Char('c')), None);
        app.handle_key(key(KeyCode::Char('ㅔ')), None);
        app.handle_key(key(KeyCode::Char('ㅊ')), None);
        app.handle_key(key(KeyCode::Esc), None);
        let request = app.take_lorebook_request().expect("c/p edits are dirty");
        assert_eq!(request[0].probability, 50, "100 → 75 → 50");
        assert!(!request[0].constant, "c toggled the constant pin back off");
        assert_eq!(app.take_lorebook_request(), None, "consumed once");

        // The ladder wraps at its tail and an off-ladder value joins at 75.
        let starts = [25, 100, 75, 50, 25, 100, 60, 75];
        let expected = [100, 75, 50, 25, 100, 75, 75, 50];
        for (probability, want) in starts.iter().zip(expected.iter()) {
            let mut app = settings_app();
            let mut entry = lore_entry("e1", &["카페"], "오래된 피아노");
            entry.probability = *probability;
            app.set_lorebook_data(&[entry], true);
            app.handle_key(key(KeyCode::Char('w')), None);
            app.handle_key(key(KeyCode::Char('p')), None);
            app.handle_key(key(KeyCode::Esc), None);
            let request = app.take_lorebook_request().expect("p edit is dirty");
            assert_eq!(request[0].probability, *want, "cycle from {probability}");
        }
    }

    #[test]
    fn lorebook_modal_renders_entries_notices_and_empty_line() {
        let mut app = settings_app();
        app.set_lorebook_data(
            &[
                lore_entry("e1", &["성별", "gender"], "플레이어는 여성이다"),
                lore_entry("e2", &["카페"], "카페에는 오래된 피아노가 있다"),
            ],
            false,
        );
        app.handle_key(key(KeyCode::Char('w')), None);
        let out = render_to_string(&mut app);
        assert!(out.contains("로어북"), "modal title missing:\n{out}");
        assert!(
            out.contains("세션 전용"),
            "session-only notice missing:\n{out}"
        );
        assert!(out.contains("성별|gender"), "keys column missing:\n{out}");
        assert!(out.contains("✔"), "enabled marker missing:\n{out}");
        assert!(
            out.contains("플레이어는 여성이다"),
            "content missing:\n{out}"
        );

        // Empty data still opens, with the explicit "no entries" line.
        let mut app = settings_app();
        app.set_lorebook_data(&[], true);
        app.handle_key(key(KeyCode::Char('w')), None);
        let out = render_to_string(&mut app);
        assert!(out.contains("엔트리 없음"), "empty line missing:\n{out}");
    }

    #[test]
    fn settings_r_opens_the_prompt_viewer_only_after_capture() {
        let mut app = settings_app();
        app.handle_key(key(KeyCode::Char('r')), None);
        assert!(app.prompt_viewer.is_none(), "no capture, no viewer");

        app.set_prompt_preview("너는 감독이다.", "플레이어: 문을 두드린다");
        app.handle_key(key(KeyCode::Char('r')), None);
        assert!(app.prompt_viewer.is_some(), "capture opens the viewer");
        // Esc closes; Korean 2-beolsik ㄱ reopens the same viewer.
        app.handle_key(key(KeyCode::Esc), None);
        assert!(app.prompt_viewer.is_none());
        app.handle_key(key(KeyCode::Char('ㄱ')), None);
        assert!(app.prompt_viewer.is_some());
    }

    #[test]
    fn set_input_text_fills_the_input_and_opens_input_mode() {
        let mut app = App::new(sample_view(2));
        assert!(!app.input_mode);
        app.set_input_text("문을 두드린다");
        assert!(app.input_mode);
        assert_eq!(app.input.text(), "문을 두드린다");
        // The player sends it themselves — Enter goes through the normal
        // free-action path, never an automatic submit.
        let command = app.handle_key(key(KeyCode::Enter), None);
        assert!(matches!(
            command,
            Some(EngineCommand::SubmitFreeAction { .. })
        ));
    }

    /// Count Image blocks the story pane would render for the current state.
    fn image_block_count(app: &mut App) -> usize {
        app.fill_block_cache();
        app.story_blocks()
            .iter()
            .filter(|block| matches!(block, StoryBlock::Image(_)))
            .count()
    }

    /// Concatenate the text of every story block (images contribute
    /// nothing) — the narration pane's visible text, for assertions.
    fn blocks_text(app: &App) -> String {
        app.story_blocks()
            .iter()
            .map(|block| match block {
                StoryBlock::Text(lines) => lines
                    .iter()
                    .map(|line| {
                        line.spans
                            .iter()
                            .map(|span| span.content.as_ref())
                            .collect::<String>()
                    })
                    .collect::<String>(),
                StoryBlock::Image(_) => String::new(),
            })
            .collect()
    }

    fn default_location() -> story_model::LocationId {
        story_model::LocationId("station".to_string())
    }

    /// A history entry whose state patch optionally moves to `next_location`.
    /// The fold attaches the POST-patch location to the entry — the turn's
    /// narration narrates arriving there, and the engine's current location
    /// (which drove the old side-image column) is post-patch too.
    fn history_entry(turn: u32, next_location: Option<&str>) -> story_model::HistoryEntry {
        story_model::HistoryEntry {
            id: Uuid::new_v4(),
            turn,
            scene_id: format!("scene-{turn}"),
            created_at: chrono::Utc::now(),
            narration: format!("턴 {turn} 내레이션"),
            player_action: None,
            choices: Vec::new(),
            applied_patch: story_model::StatePatch {
                next_location: next_location.map(|loc| story_model::LocationId(loc.to_string())),
                ..Default::default()
            },
            generation_id: Uuid::new_v4(),
        }
    }

    fn halfblocks_app(view_choices: usize) -> App {
        let mut app = App::new(sample_view(view_choices));
        app.set_image_picker(Some(ratatui_image::picker::Picker::halfblocks()), false);
        app
    }

    #[test]
    fn location_timeline_folds_patches_in_order() {
        let initial = default_location();
        let history = vec![
            history_entry(1, Some("cafe")),
            history_entry(2, None),
            history_entry(3, Some("docks")),
        ];
        let timeline = location_timeline(&history, &initial);
        assert_eq!(
            timeline,
            vec![
                Some("cafe".to_string()),
                Some("cafe".to_string()),
                Some("docks".to_string())
            ]
        );
        // Empty history → empty timeline; the opening comes from the seed.
        assert!(location_timeline(&[], &initial).is_empty());
    }

    #[test]
    fn classify_emotion_maps_narration_keywords_to_emotions() {
        let cases: &[(&str, Emotion)] = &[
            ("유나는 환한 미소를 지으며 커피를 내밀었다.", Emotion::Happy),
            ("그녀의 눈에 눈물이 맺혔다.", Emotion::Sad),
            ("그는 화를 내며 책상을 쳤다.", Emotion::Angry),
            ("불안한 마음을 감추려 앞만 보았다.", Emotion::Anxious),
            ("깜짝 놀라 한 걸음 물러섰다.", Emotion::Surprised),
            ("다정한 목소리로 이름을 불러주었다.", Emotion::Affectionate),
            ("차가운 시선이 스쳐 지나갔다.", Emotion::Cold),
            (
                "볼이 붉어지고 부끄러워 고개를 돌렸다.",
                Emotion::Embarrassed,
            ),
            ("죄책감에 미안하다는 말이 목에 걸렸다.", Emotion::Guilty),
            // English cues match against the lowercased narration.
            ("She smiles at the joke.", Emotion::Happy),
            ("He hugs her tightly.", Emotion::Affectionate),
            ("그녀는 조용히 창밖을 바라보았다.", Emotion::Neutral),
        ];
        for (text, want) in cases {
            assert_eq!(classify_emotion(text), *want, "narration: {text}");
        }
    }

    #[test]
    fn classify_emotion_counts_hits_and_breaks_ties_by_table_order() {
        // Sad scores 2 against Happy's 1 — majority wins.
        assert_eq!(
            classify_emotion("슬픈 눈물을 닦으며 억지 웃음을 지었다."),
            Emotion::Sad
        );
        // Equal scores resolve to the earlier table entry (Happy precedes Sad).
        assert_eq!(classify_emotion("웃음과 눈물"), Emotion::Happy);
    }

    #[test]
    fn emotion_timeline_classifies_each_turn_narration() {
        let mut first = history_entry(1, None);
        first.narration = "그녀의 눈에 눈물이 맺혔다.".to_string();
        let mut second = history_entry(2, None);
        second.narration = "유나는 환한 미소를 지었다.".to_string();
        let history = vec![first, second];
        assert_eq!(
            emotion_timeline(&history),
            vec![Emotion::Sad, Emotion::Happy]
        );
        assert!(emotion_timeline(&[]).is_empty());
    }

    #[test]
    fn set_lists_folds_patch_locations_over_history() {
        let mut app = App::new(sample_view(1));
        let history = vec![
            history_entry(1, Some("cafe")),
            history_entry(2, None),
            history_entry(3, Some("docks")),
        ];
        app.set_lists(&history, &[], &default_location());
        assert_eq!(app.story_stack[0].location.as_deref(), Some("cafe"));
        assert_eq!(app.story_stack[1].location.as_deref(), Some("cafe"));
        assert_eq!(app.story_stack[2].location.as_deref(), Some("docks"));
    }

    #[test]
    fn opening_turn_shows_the_initial_location_image() {
        let mut app = halfblocks_app(0);
        app.set_lists(&[], &[], &default_location());
        app.set_location_image(
            "station",
            Some((
                std::path::PathBuf::from("/pack/station.png"),
                image::DynamicImage::new_rgb8(8, 8),
            )),
        );
        assert_eq!(
            image_block_count(&mut app),
            1,
            "opening shows the initial location's image"
        );
        // A location resolved to no art contributes no image block.
        app.set_location_image("station", None);
        assert_eq!(image_block_count(&mut app), 0);
    }

    #[test]
    fn same_portrait_fallback_not_repeated_across_locations() {
        let mut app = halfblocks_app(1);
        let history = vec![
            history_entry(1, Some("cafe")),
            history_entry(2, Some("docks")),
        ];
        app.set_lists(&history, &[], &default_location());
        // Every location resolves to the SAME portrait file (character
        // fallback): the portrait shows once, not at every scene boundary.
        let portrait = std::path::PathBuf::from("/pack/characters/mira.png");
        for location in ["station", "cafe", "docks"] {
            app.set_location_image(
                location,
                Some((portrait.clone(), image::DynamicImage::new_rgb8(8, 8))),
            );
        }
        assert_eq!(
            image_block_count(&mut app),
            1,
            "same source file must not repeat at every boundary"
        );
    }

    #[test]
    fn revisited_location_shows_the_image_again() {
        let mut app = halfblocks_app(1);
        let history = vec![
            history_entry(1, Some("cafe")),
            history_entry(2, Some("docks")),
            history_entry(3, Some("cafe")),
        ];
        app.set_lists(&history, &[], &default_location());
        for (location, name) in [("station", "station"), ("cafe", "cafe"), ("docks", "docks")] {
            app.set_location_image(
                location,
                Some((
                    std::path::PathBuf::from(format!("/pack/{name}.png")),
                    image::DynamicImage::new_rgb8(8, 8),
                )),
            );
        }
        // Three distinct scene boundaries → three image blocks, including
        // the return to cafe.
        assert_eq!(image_block_count(&mut app), 3);
    }

    #[test]
    fn location_image_without_a_picker_is_authoritatively_absent() {
        // No set_image_picker: a resolution cannot become renderable. The
        // absence is still recorded (no per-frame retry) and rendering stays
        // text-only.
        let mut app = App::new(sample_view(1));
        let history = vec![history_entry(1, None)];
        app.set_lists(&history, &[], &default_location());
        app.set_location_image(
            "station",
            Some((
                std::path::PathBuf::from("/pack/station.png"),
                image::DynamicImage::new_rgb8(8, 8),
            )),
        );
        let stored = app
            .location_images
            .get("station")
            .expect("resolution recorded");
        assert!(stored.is_none(), "no picker → authoritative absence");
        assert_eq!(image_block_count(&mut app), 0);
    }

    #[test]
    fn stack_cache_invalidates_when_the_stack_changes() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use unicode_width::UnicodeWidthStr;

        fn entry(turn: u32, narration: &str) -> story_model::HistoryEntry {
            story_model::HistoryEntry {
                id: Uuid::new_v4(),
                turn,
                scene_id: format!("scene-{turn}"),
                created_at: chrono::Utc::now(),
                narration: narration.to_string(),
                player_action: None,
                choices: Vec::new(),
                applied_patch: story_model::StatePatch::default(),
                generation_id: Uuid::new_v4(),
            }
        }

        fn draw(app: &mut App) -> String {
            let mut terminal = Terminal::new(TestBackend::new(100, 40)).expect("terminal");
            terminal
                .draw(|frame| app.render(frame, frame.area()))
                .expect("draw");
            let buffer = terminal.backend().buffer();
            let mut out = String::new();
            for y in 0..buffer.area.height {
                // A double-width cell is followed by a literal " " spacer;
                // skip only that spacer so Hangul sequences stay contiguous.
                let mut previous_was_wide = false;
                for x in 0..buffer.area.width {
                    let symbol = buffer[(x, y)].symbol();
                    if previous_was_wide && symbol == " " {
                        previous_was_wide = false;
                        continue;
                    }
                    previous_was_wide = symbol.width() == 2;
                    out.push_str(symbol);
                }
            }
            out
        }

        let mut app = App::new(sample_view(1));
        app.set_lists(&[entry(1, "첫 번째 장면")], &[], &default_location());
        let first = draw(&mut app);
        assert!(first.contains("첫 번째 장면"));

        // The cache MUST be invalidated by the set_lists version bump: the
        // second draw renders the NEW transcript, not the frozen first one.
        app.set_lists(
            &[
                entry(1, "첫 번째 장면"),
                entry(2, "두 번째 장면의 내레이션"),
            ],
            &[],
            &default_location(),
        );
        let second = draw(&mut app);
        assert!(
            second.contains("두 번째 장면의 내레이션"),
            "stale cache rendered the old transcript across a stack change"
        );
        assert_ne!(first, second);
    }

    #[test]
    fn set_lists_builds_chronological_story_stack() {
        let mut app = App::new(sample_view(1));
        let entries = vec![
            story_model::HistoryEntry {
                id: Uuid::new_v4(),
                turn: 1,
                scene_id: "station".to_string(),
                created_at: chrono::Utc::now(),
                narration: "첫 장면".to_string(),
                player_action: None,
                choices: Vec::new(),
                applied_patch: story_model::StatePatch::default(),
                generation_id: Uuid::new_v4(),
            },
            story_model::HistoryEntry {
                id: Uuid::new_v4(),
                turn: 2,
                scene_id: "cafe".to_string(),
                created_at: chrono::Utc::now(),
                narration: "다음 장면".to_string(),
                player_action: None,
                choices: Vec::new(),
                applied_patch: story_model::StatePatch::default(),
                generation_id: Uuid::new_v4(),
            },
        ];

        app.set_lists(&entries[..1], &[], &default_location());
        assert_eq!(app.narration_scroll, 0, "first scene opens at the top");
        app.set_lists(&entries, &[], &default_location());

        assert_eq!(app.story_stack.len(), 2);
        assert_eq!(app.story_stack[0].narration, "첫 장면");
        assert_eq!(app.story_stack[1].narration, "다음 장면");
        assert_eq!(
            app.narration_scroll,
            u16::MAX,
            "new scene follows the bottom"
        );
    }

    #[test]
    fn set_lists_attaches_generation_duration_per_turn() {
        let mut app = App::new(sample_view(1));
        let g1 = Uuid::new_v4();
        let g2 = Uuid::new_v4();
        let entries = vec![
            story_model::HistoryEntry {
                id: Uuid::new_v4(),
                turn: 1,
                scene_id: "station".to_string(),
                created_at: chrono::Utc::now(),
                narration: "첫 장면".to_string(),
                player_action: None,
                choices: Vec::new(),
                applied_patch: story_model::StatePatch::default(),
                generation_id: g1,
            },
            story_model::HistoryEntry {
                id: Uuid::new_v4(),
                turn: 2,
                scene_id: "cafe".to_string(),
                created_at: chrono::Utc::now(),
                narration: "다음 장면".to_string(),
                player_action: None,
                choices: Vec::new(),
                applied_patch: story_model::StatePatch::default(),
                generation_id: g2,
            },
        ];

        // No durations recorded yet → entries carry None.
        app.set_lists(&entries, &[], &default_location());
        assert_eq!(app.story_stack[0].generation_secs, None);
        assert_eq!(app.story_stack[1].generation_secs, None);

        // Record durations keyed by generation_id (as the host does on the
        // completion path, before the committing dispatch).
        app.generation_durations.insert(g1, 47);
        app.generation_durations.insert(g2, 133);
        app.set_lists(&entries, &[], &default_location());
        assert_eq!(app.story_stack[0].generation_secs, Some(47));
        assert_eq!(app.story_stack[1].generation_secs, Some(133));
    }

    #[test]
    fn home_menu_defaults_to_continue_and_keeps_new_game_explicit() {
        let mut app = App::new(sample_view(0));
        app.view.status.turn = 0;
        app.set_can_continue(true);
        app.show_home();

        // AC-2: with an autosave the default cursor is the 이어하기 row.
        assert!(app.can_continue);
        app.handle_key(key(KeyCode::Enter), None);
        assert_eq!(app.take_home_request(), Some(HomeRequest::Continue));

        // 새 게임 stays an explicit selection: Up to row 0, then Enter.
        app.show_home();
        app.handle_key(key(KeyCode::Up), None);
        app.handle_key(key(KeyCode::Enter), None);
        assert_eq!(app.take_home_request(), Some(HomeRequest::StartNew));
    }

    #[test]
    fn home_without_a_resume_keeps_the_new_game_default_cursor() {
        let mut app = App::new(sample_view(0));
        app.view.status.turn = 0;
        app.set_can_continue(false);
        app.show_home();

        app.handle_key(key(KeyCode::Enter), None);
        assert_eq!(app.take_home_request(), Some(HomeRequest::StartNew));
    }

    #[test]
    fn losing_the_resume_demotes_the_cursor_off_the_continue_row() {
        let mut app = App::new(sample_view(0));
        app.view.status.turn = 0;
        app.set_can_continue(true);
        assert!(app.can_continue);
        // A pack switch (host-side set_can_continue(false)) must not leave
        // the cursor on a row that no longer exists.
        app.set_can_continue(false);
        app.show_home();
        app.handle_key(key(KeyCode::Enter), None);
        assert_eq!(app.take_home_request(), Some(HomeRequest::StartNew));
    }

    #[test]
    fn home_key_opens_launcher_and_continue_returns_to_current_game() {
        let mut app = App::new(sample_view(1));
        app.handle_key(key(KeyCode::Home), None);
        assert!(app.home_visible);
        app.handle_key(key(KeyCode::Enter), None);
        assert_eq!(app.take_home_request(), Some(HomeRequest::ReturnToGame));
    }

    #[test]
    fn settings_entry_requests_model_refresh_and_accepts_choice() {
        let mut app = App::new(sample_view(0));
        app.set_settings(
            ProviderConfigSafe {
                base_url: "http://127.0.0.1:8000".to_string(),
                model: "a".to_string(),
                director_model: None,
                choice_model: None,
                writer_model: None,
                structured_output: "auto".to_string(),
                request_timeout_ms: 1_000,
                connect_timeout_ms: 1_000,
                max_retries: 0,
                is_loopback: true,
            },
            ProviderHealth::Ready,
            0.7,
            0.9,
            40,
            1_200,
            TtsDisplay {
                enabled: false,
                backend: "say".to_string(),
                voice: None,
            },
            true,
            Some("halfblocks".to_string()),
            false,
            false,
            false,
            true,
            "자동".to_string(),
            "없음".to_string(),
            String::new(),
            String::new(),
        );
        app.show_home();
        app.handle_key(key(KeyCode::Down), None);
        app.handle_key(key(KeyCode::Enter), None);
        assert!(app.take_settings_opened());

        app.set_models(vec!["a".to_string(), "b".to_string()], "a");
        app.handle_key(key(KeyCode::Char('p')), None);
        app.handle_key(key(KeyCode::Down), None);
        app.handle_key(key(KeyCode::Char('r')), None);
        assert!(app.take_model_reload_requested());
        app.refresh_models(vec!["a".to_string(), "b".to_string(), "c".to_string()], "a");
        app.handle_key(key(KeyCode::Enter), None);
        assert_eq!(app.take_selected_model().as_deref(), Some("b"));

        app.handle_key(key(KeyCode::Char('p')), None);
        app.handle_key(key(KeyCode::Char('c')), None);
        assert_eq!(app.take_selected_choice_model().as_deref(), Some("b"));
    }

    #[test]
    fn provider_picker_selects_a_profile() {
        let mut app = App::new(sample_view(0));
        app.set_providers(
            &[
                (
                    "omlx-local".to_string(),
                    "http://127.0.0.1:8000".to_string(),
                ),
                (
                    "openrouter".to_string(),
                    "https://openrouter.ai/api/v1".to_string(),
                ),
            ],
            "omlx-local",
        );
        assert_eq!(app.provider_selected, 0, "active profile highlighted");

        app.handle_key(key(KeyCode::Char('o')), None);
        app.handle_key(key(KeyCode::Char('e')), None);
        assert_eq!(app.list_mode, Some(ListMode::ProviderPicker));

        app.handle_key(key(KeyCode::Down), None);
        app.handle_key(key(KeyCode::Enter), None);
        assert_eq!(app.take_selected_provider().as_deref(), Some("openrouter"));
        assert_eq!(app.take_selected_provider(), None, "request is consumed");
    }

    #[test]
    fn leaving_and_reentering_settings_keeps_model_picker_available() {
        let mut app = App::new(sample_view(0));
        app.set_settings(
            ProviderConfigSafe {
                base_url: "http://127.0.0.1:8000".to_string(),
                model: "a".to_string(),
                director_model: None,
                choice_model: None,
                writer_model: None,
                structured_output: "auto".to_string(),
                request_timeout_ms: 1_000,
                connect_timeout_ms: 1_000,
                max_retries: 0,
                is_loopback: true,
            },
            ProviderHealth::Ready,
            0.7,
            0.9,
            40,
            1_200,
            TtsDisplay {
                enabled: false,
                backend: "say".to_string(),
                voice: None,
            },
            true,
            Some("halfblocks".to_string()),
            false,
            false,
            false,
            true,
            "자동".to_string(),
            "없음".to_string(),
            String::new(),
            String::new(),
        );
        app.show_home();
        app.handle_key(key(KeyCode::Down), None);
        app.handle_key(key(KeyCode::Enter), None);
        app.handle_key(key(KeyCode::Esc), None);
        app.handle_key(key(KeyCode::Enter), None);
        assert!(app.take_settings_opened());

        app.set_models(vec!["a".to_string(), "b".to_string()], "a");
        app.handle_key(key(KeyCode::Char('p')), None);
        app.handle_key(key(KeyCode::Down), None);
        app.handle_key(key(KeyCode::Enter), None);
        assert_eq!(app.take_selected_model().as_deref(), Some("b"));
    }

    #[test]
    fn u_key_resolves_the_view_undo_checkpoint() {
        let mut app = App::new(sample_view(3));
        assert_eq!(
            app.handle_key(key(KeyCode::Char('u')), None),
            None,
            "no undo target in the view: the key is a no-op"
        );
        let checkpoint_id = Uuid::new_v4();
        app.view.undo_checkpoint_id = Some(checkpoint_id);
        assert_eq!(
            app.handle_key(key(KeyCode::Char('u')), None),
            Some(EngineCommand::RestoreCheckpoint { checkpoint_id })
        );
        assert_eq!(
            app.handle_key(key(KeyCode::Char('ㅕ')), None),
            Some(EngineCommand::RestoreCheckpoint { checkpoint_id }),
            "korean layout resolves the same checkpoint"
        );
    }

    #[test]
    fn history_enter_edits_selected_narration_with_ctrl_s() {
        let mut app = App::new(sample_view(1));
        let first = history_entry(1, None);
        let second = history_entry(2, None);
        let second_id = second.id;
        app.set_lists(&[first, second], &[], &default_location());

        app.handle_key(key(KeyCode::Char('h')), None);
        assert_eq!(app.list_mode, Some(ListMode::History));
        assert_eq!(app.history_selected, 0, "newest row is selected first");
        app.handle_key(key(KeyCode::Enter), None);
        assert!(app.history_edit.is_some());

        let dialog = app.history_edit.as_mut().unwrap();
        dialog.buffer.clear();
        dialog.buffer.insert_str("고친 문장");
        let submit = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(
            app.handle_key(submit, None),
            Some(EngineCommand::EditHistoryNarration {
                entry_id: second_id,
                narration: "고친 문장".to_string(),
            })
        );
        assert!(app.history_edit.is_none());
    }

    #[test]
    fn history_delete_confirm_truncates_from_selected_turn() {
        let mut app = App::new(sample_view(1));
        let first = history_entry(1, None);
        let second = history_entry(2, None);
        let second_id = second.id;
        // Host reports a checkpoint for turn 1 → turn 2 is deletable.
        app.set_lists_with_checkpoints(&[first, second], &[], &default_location(), &[1]);

        app.handle_key(key(KeyCode::Char('h')), None);
        app.handle_key(key(KeyCode::Char('d')), None);
        assert!(app.history_delete_confirm);
        assert_eq!(
            app.handle_key(key(KeyCode::Enter), None),
            Some(EngineCommand::DeleteHistoryFrom {
                entry_id: second_id
            })
        );
        assert!(!app.history_delete_confirm);
    }

    #[test]
    fn history_delete_refuses_the_opening_turn() {
        let mut app = App::new(sample_view(1));
        let opening = history_entry(1, None);
        app.set_lists_with_checkpoints(&[opening], &[], &default_location(), &[1]);
        app.handle_key(key(KeyCode::Char('h')), None);
        app.handle_key(key(KeyCode::Char('d')), None);
        assert!(
            !app.history_delete_confirm,
            "turn 1 cannot open the delete confirm"
        );
        assert_eq!(
            app.story_notice(),
            Some("이 턴은 삭제할 수 없습니다 (오프닝 또는 되돌리기 링 밖)")
        );
    }

    #[test]
    fn history_delete_refuses_when_checkpoint_is_outside_the_ring() {
        let mut app = App::new(sample_view(1));
        let first = history_entry(1, None);
        let second = history_entry(2, None);
        // No turn-1 checkpoint reported → delete is refused with a notice.
        app.set_lists_with_checkpoints(&[first, second], &[], &default_location(), &[]);
        app.handle_key(key(KeyCode::Char('h')), None);
        app.handle_key(key(KeyCode::Char('d')), None);
        assert!(!app.history_delete_confirm);
        assert!(app.story_notice().is_some());
    }

    #[test]
    fn history_edit_keeps_dialog_on_empty_submit() {
        let mut app = App::new(sample_view(1));
        let entry = history_entry(1, None);
        app.set_lists(&[entry], &[], &default_location());
        app.handle_key(key(KeyCode::Char('h')), None);
        app.handle_key(key(KeyCode::Enter), None);
        app.history_edit.as_mut().unwrap().buffer.clear();
        let submit = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL);
        assert_eq!(app.handle_key(submit, None), None);
        assert!(
            app.history_edit.is_some(),
            "empty submit must keep the dialog"
        );
        assert_eq!(
            app.story_notice(),
            Some("내레이션이 비어 있어 적용하지 않았습니다")
        );
    }

    #[test]
    fn loading_and_error_block_the_undo_key() {
        let mut app = App::new(sample_view(3));
        app.view.undo_checkpoint_id = Some(Uuid::new_v4());
        app.view.loading = Some(story_model::LoadingState {
            generation_id: Uuid::nil(),
            message_ko: "생성 중...".to_string(),
        });
        assert_eq!(app.handle_key(key(KeyCode::Char('u')), None), None);
        app.view.loading = None;
        app.view.error = Some(story_model::DomainErrorView {
            code: "test".to_string(),
            title_ko: "오류".to_string(),
            message_ko: "재시도 필요".to_string(),
            recoverable: true,
        });
        assert_eq!(app.handle_key(key(KeyCode::Char('u')), None), None);
    }

    #[test]
    fn u_key_survives_a_resolved_ending_with_no_choices() {
        // D1 regression: after an ending resolves, the engine clears the
        // choices but the undo target (the pre-ending turn) stays offered.
        // The keyboard path must match the footer/[U] click path here.
        let mut app = App::new(sample_view(0));
        let checkpoint_id = Uuid::new_v4();
        app.view.choices = Vec::new();
        app.view.undo_checkpoint_id = Some(checkpoint_id);
        assert_eq!(
            app.handle_key(key(KeyCode::Char('u')), None),
            Some(EngineCommand::RestoreCheckpoint { checkpoint_id })
        );
        // No target in the view (fresh game, save restore): the nil sentinel
        // is dropped instead of reaching the engine.
        app.view.undo_checkpoint_id = None;
        assert_eq!(app.handle_key(key(KeyCode::Char('u')), None), None);
    }

    #[test]
    fn n_key_asks_before_resetting_progress() {
        let mut app = App::new(sample_view(3));
        assert_eq!(app.view.status.turn, 1);
        assert_eq!(app.handle_key(key(KeyCode::Char('n')), None), None);
        assert!(app.new_game_confirm, "the confirm modal intercepts n");
        assert_eq!(
            app.handle_key(key(KeyCode::Enter), None),
            Some(EngineCommand::StartNewGame),
            "only an explicit Enter resets"
        );
        assert!(!app.new_game_confirm);

        // Esc (or a repeated n) backs out without touching anything.
        app.handle_key(key(KeyCode::Char('ㅜ')), None);
        assert!(app.new_game_confirm);
        assert_eq!(app.handle_key(key(KeyCode::Esc), None), None);
        assert!(!app.new_game_confirm);
        assert!(!app.should_quit);
    }

    #[test]
    fn n_key_at_turn_zero_starts_immediately() {
        let mut app = App::new(sample_view(3));
        app.view.status.turn = 0;
        assert_eq!(
            app.handle_key(key(KeyCode::Char('n')), None),
            Some(EngineCommand::StartNewGame),
            "nothing to lose before the opening: no modal"
        );
        assert!(!app.new_game_confirm);
    }

    #[test]
    fn r_key_rerolls_the_in_flight_turn_through_the_app_gate() {
        // D1 regression: the loading key gate must pass `r`/`ㄱ` through to
        // map_key, or the advertised [R] 다시 만들기 is a dead key.
        let mut app = App::new(sample_view(3));
        app.view.loading = Some(story_model::LoadingState {
            generation_id: Uuid::nil(),
            message_ko: "생성 중...".to_string(),
        });
        assert!(matches!(
            app.handle_key(key(KeyCode::Char('r')), Some(Uuid::nil())),
            Some(EngineCommand::RegenerateCurrentTurn)
        ));
        assert!(matches!(
            app.handle_key(key(KeyCode::Char('ㄱ')), Some(Uuid::nil())),
            Some(EngineCommand::RegenerateCurrentTurn)
        ));
        // The gate still absorbs unrelated keys while loading.
        assert_eq!(app.handle_key(key(KeyCode::Char('h')), None), None);
        assert!(app.list_mode.is_none());
    }

    #[test]
    fn modal_blocks_mouse_behind_the_new_game_confirm() {
        // D2 regression: clicks must not drive the story from behind the
        // confirm modal (footer [U]/[Home]/... live underneath it).
        let mut app = App::new(sample_view(3));
        app.handle_key(key(KeyCode::Char('n')), None);
        assert!(app.new_game_confirm);
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 1,
            modifiers: KeyModifiers::NONE,
        });
        assert!(app.new_game_confirm, "the modal stays up");
        assert!(
            app.take_mouse_command().is_none(),
            "no engine command leaks from a click behind the modal"
        );
        assert!(!app.should_quit);
    }

    #[test]
    fn home_new_game_asks_when_progress_exists() {
        let mut app = App::new(sample_view(3));
        assert_eq!(app.view.status.turn, 1);
        app.home_visible = true;
        app.home_selected = 0;
        // Stray Enter on the destructive default row opens the confirm.
        assert_eq!(app.handle_key(key(KeyCode::Enter), None), None);
        assert!(app.new_game_confirm);
        assert!(app.new_game_confirm_from_home);
        assert!(app.home_request.is_none(), "no reset before confirmation");
        // Esc closes only the modal; the home menu stays for the original
        // purpose (pack picking, import, ...).
        assert_eq!(app.handle_key(key(KeyCode::Esc), None), None);
        assert!(!app.new_game_confirm);
        assert!(app.home_visible);
        assert!(app.home_request.is_none());

        // Confirm replays the home arm (no greetings → home_request), the
        // same flow a deliberate selection would have taken.
        app.handle_key(key(KeyCode::Enter), None);
        assert!(app.new_game_confirm);
        assert_eq!(app.handle_key(key(KeyCode::Enter), None), None);
        assert_eq!(app.home_request, Some(HomeRequest::StartNew));
        assert!(!app.new_game_confirm);
        assert!(!app.home_visible);
    }

    #[test]
    fn home_new_game_at_turn_zero_starts_immediately() {
        let mut app = App::new(sample_view(3));
        app.view.status.turn = 0;
        app.home_visible = true;
        app.home_selected = 0;
        assert_eq!(app.handle_key(key(KeyCode::Enter), None), None);
        assert!(!app.new_game_confirm, "nothing to lose before the opening");
        assert_eq!(app.home_request, Some(HomeRequest::StartNew));
    }

    #[test]
    fn home_new_game_confirm_replays_the_greeting_picker() {
        // With progress and multiple greetings, confirming the home guard
        // must replay the original arm: picker opens (not a silent start),
        // the pick gates StartNew, and a cancelled pick re-asks the guard.
        let mut app = App::new(sample_view(3));
        app.home_visible = true;
        app.set_greetings(vec![
            "기본 오프닝".to_string(),
            "비 오는 날의 인사".to_string(),
        ]);
        app.handle_key(key(KeyCode::Enter), None);
        assert!(app.new_game_confirm, "progress exists: confirm first");
        assert!(app.greeting_picker.is_none(), "picker waits for confirm");

        assert_eq!(app.handle_key(key(KeyCode::Enter), None), None);
        assert!(
            app.greeting_picker.is_some(),
            "confirm replays the greeting picker arm"
        );
        assert!(app.take_home_request().is_none(), "picker gates the start");

        // Esc cancels the pick; choosing 새 게임 again asks again.
        app.handle_key(key(KeyCode::Esc), None);
        assert!(app.greeting_picker.is_none());
        assert!(app.home_visible);
        assert_eq!(app.handle_key(key(KeyCode::Enter), None), None);
        assert!(
            app.new_game_confirm,
            "the guard re-arms after a cancelled pick"
        );
    }

    #[test]
    fn settings_g_key_requests_the_generation_toggle_once() {
        let mut app = App::new(sample_view(0));
        app.set_settings(
            ProviderConfigSafe {
                base_url: "http://127.0.0.1:8000".to_string(),
                model: "a".to_string(),
                director_model: None,
                choice_model: None,
                writer_model: None,
                structured_output: "auto".to_string(),
                request_timeout_ms: 1_000,
                connect_timeout_ms: 1_000,
                max_retries: 0,
                is_loopback: true,
            },
            ProviderHealth::Ready,
            0.7,
            0.9,
            40,
            1_200,
            TtsDisplay {
                enabled: false,
                backend: "say".to_string(),
                voice: None,
            },
            true,
            Some("halfblocks".to_string()),
            false,
            false,
            false,
            true,
            "자동".to_string(),
            "없음".to_string(),
            String::new(),
            String::new(),
        );
        assert!(!app.use_director_writer(), "mirrors the seeded value");

        // Open Settings (`o`), then `g` (2-beolsik ㅎ) requests the toggle;
        // the Korean variant hits the same branch.
        app.handle_key(key(KeyCode::Char('o')), None);
        app.handle_key(key(KeyCode::Char('g')), None);
        assert!(app.take_generation_toggle_request());
        assert!(!app.take_generation_toggle_request(), "request is consumed");

        app.handle_key(key(KeyCode::Char('ㅎ')), None);
        assert!(app.take_generation_toggle_request());

        // The host flips the session value, then patches the display.
        app.update_settings_generation(true);
        assert!(app.use_director_writer());
    }

    #[test]
    fn settings_s_key_requests_the_stream_toggle_once() {
        let mut app = App::new(sample_view(0));
        app.set_settings(
            ProviderConfigSafe {
                base_url: "http://127.0.0.1:8000".to_string(),
                model: "a".to_string(),
                director_model: None,
                choice_model: None,
                writer_model: None,
                structured_output: "auto".to_string(),
                request_timeout_ms: 1_000,
                connect_timeout_ms: 1_000,
                max_retries: 0,
                is_loopback: true,
            },
            ProviderHealth::Ready,
            0.7,
            0.9,
            40,
            1_200,
            TtsDisplay {
                enabled: false,
                backend: "say".to_string(),
                voice: None,
            },
            true,
            Some("halfblocks".to_string()),
            false,
            false,
            false,
            true,
            "자동".to_string(),
            "없음".to_string(),
            String::new(),
            String::new(),
        );
        assert!(!app.stream_turns(), "mirrors the seeded value");

        // Open Settings (`o`), then `s` (2-beolsik ㄴ) requests the toggle;
        // the Korean variant hits the same branch. The story screen's `s`
        // (save dialog) is a different screen scope and stays unaffected —
        // pressing it here must not open the save/load dialog either.
        app.handle_key(key(KeyCode::Char('o')), None);
        app.handle_key(key(KeyCode::Char('s')), None);
        assert!(app.take_stream_toggle_request());
        assert!(!app.take_stream_toggle_request(), "request is consumed");
        assert!(
            app.take_save_load_request().is_none(),
            "settings `s` must not leak into the story save dialog"
        );

        app.handle_key(key(KeyCode::Char('ㄴ')), None);
        assert!(app.take_stream_toggle_request());
        assert!(app.take_save_load_request().is_none());

        // The host flips the session value, then patches the display.
        app.update_settings_stream(true);
        assert!(app.stream_turns());
    }

    #[test]
    fn mouse_click_selects_a_model_row() {
        let mut app = App::new(sample_view(0));
        app.set_models(vec!["a".to_string(), "b".to_string()], "a");
        app.list_mode = Some(ListMode::ModelPicker);
        app.last_render_area.set(Rect::new(0, 0, 80, 24));

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 8,
            row: 2,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(app.take_selected_model().as_deref(), Some("b"));
        assert_eq!(app.list_mode, None);
    }

    #[test]
    fn mouse_click_submits_the_story_choice_under_the_cursor() {
        let mut app = App::new(sample_view(3));
        app.last_render_area.set(Rect::new(0, 0, 80, 24));

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 8,
            row: 17,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(
            app.take_mouse_command(),
            Some(EngineCommand::SubmitChoice {
                choice_id: "c-0".to_string()
            })
        );
    }

    #[test]
    fn mouse_click_on_visible_home_shortcut_opens_launcher() {
        let app = App::new(sample_view(1));
        let mut clicked_home = false;
        for column in 0..80 {
            let mut candidate = app.clone();
            candidate.last_render_area.set(Rect::new(0, 0, 80, 24));
            candidate.handle_mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column,
                row: 23,
                modifiers: KeyModifiers::NONE,
            });
            if candidate.home_visible {
                clicked_home = true;
                break;
            }
        }
        assert!(
            clicked_home,
            "the rendered [Home] shortcut must be clickable"
        );
    }

    #[test]
    fn mouse_click_on_footer_shortcuts_triggers_actions() {
        let app = App::new(sample_view(1));
        let mut clicked_save = false;
        let mut clicked_input = false;
        let mut clicked_settings = false;

        for column in 0..80 {
            let mut candidate = app.clone();
            candidate.last_render_area.set(Rect::new(0, 0, 80, 24));
            candidate.handle_mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column,
                row: 23,
                modifiers: KeyModifiers::NONE,
            });
            if candidate.save_load_request == Some(SaveLoadRequest::Open(SaveLoadMode::Save)) {
                clicked_save = true;
            }
            if candidate.input_mode {
                clicked_input = true;
            }
            if candidate.settings_visible {
                clicked_settings = true;
            }
        }
        assert!(clicked_save, "rendered [S] shortcut must be clickable");
        assert!(clicked_input, "rendered [I] shortcut must be clickable");
        assert!(clicked_settings, "rendered [O] shortcut must be clickable");
    }

    #[test]
    fn mouse_click_on_save_load_slot_selects_and_executes() {
        let mut app = App::new(sample_view(3));
        app.open_save_load(SaveLoadMode::Load, vec![save_slot("slot-1", true)]);
        app.last_render_area.set(Rect::new(0, 0, 80, 24));

        // Click row 3 (first slot inside list_area: title occupies rows 0-1, list border at 2, slots at 3..)
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 10,
            row: 3,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(
            app.take_save_load_request(),
            Some(SaveLoadRequest::Load {
                slot_id: "slot-1".to_string()
            })
        );
    }

    #[test]
    fn follow_story_end_uses_the_layout_resolved_bottom_sentinel() {
        let mut app = App::new(sample_view(1));
        app.follow_story_end();
        assert_eq!(app.narration_scroll, u16::MAX);
    }

    #[test]
    fn downward_scroll_restores_follow_end_sentinel_at_known_bottom() {
        let mut app = App::new(sample_view(1));
        app.narration_scroll = 17;
        app.narration_max_scroll.set(20);

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(app.narration_scroll, u16::MAX);
    }

    #[test]
    fn home_menu_opens_import_dialog_and_submits_path() {
        let mut app = App::new(sample_view(0));
        app.show_home();
        app.set_can_continue(false);
        // 0 → (이어하기 disabled) → 2 설정 → 3 카드 가져오기
        app.handle_key(key(KeyCode::Down), None);
        app.handle_key(key(KeyCode::Down), None);
        assert_eq!(app.home_selected, 3);
        app.handle_key(key(KeyCode::Enter), None);
        assert!(app.import_dialog.is_some(), "dialog opens");

        // Typing goes into the dialog, not the story input.
        for character in "sto/c".chars() {
            app.handle_key(key(KeyCode::Char(character)), None);
        }
        app.handle_key(key(KeyCode::Enter), None);
        assert_eq!(app.take_import_request().as_deref(), Some("sto/c"));
        assert!(app.take_import_request().is_none(), "request consumed once");
        app.take_import_request(); // stay None
        let _ = app.take_import_request();

        // Paste lands in the path buffer too — the reopened dialog starts
        // from the last import directory ("sto/"), so the paste completes
        // the file name instead of retyping the folder.
        app.open_import_dialog();
        app.handle_paste("card.png");
        assert_eq!(
            app.take_import_request(),
            None,
            "paste alone does not submit"
        );
        app.handle_key(key(KeyCode::Enter), None);
        assert_eq!(app.take_import_request().as_deref(), Some("sto/card.png"));
    }

    #[test]
    fn import_report_enter_requests_pack_swap_and_esc_closes() {
        let mut app = App::new(sample_view(0));
        app.show_home();
        app.open_import_dialog();
        app.handle_key(key(KeyCode::Char('x')), None);
        app.handle_key(key(KeyCode::Enter), None);
        assert!(app.take_import_request().is_some());

        app.show_import_report(
            "stories/x",
            vec!["로어북 엔트리 1개".to_string()],
            Some("stories/x".to_string()),
        );
        app.handle_key(key(KeyCode::Enter), None);
        assert_eq!(app.take_pack_swap_request().as_deref(), Some("stories/x"));
        assert!(
            app.import_dialog.is_none(),
            "report closes after the swap request"
        );

        // Without a ready pack, Enter does nothing and Esc closes.
        app.open_import_dialog();
        app.handle_key(key(KeyCode::Char('y')), None);
        app.handle_key(key(KeyCode::Enter), None);
        app.show_import_report(
            "stories/x",
            vec!["임포트 실패: 파일 없음".to_string()],
            None,
        );
        app.handle_key(key(KeyCode::Enter), None);
        assert!(app.take_pack_swap_request().is_none(), "no pack, no swap");
        app.handle_key(key(KeyCode::Esc), None);
        assert!(app.import_dialog.is_none());
    }

    #[test]
    fn import_dialog_esc_cancels_and_running_ignores_keys() {
        let mut app = App::new(sample_view(0));
        app.show_home();
        app.open_import_dialog();
        app.handle_key(key(KeyCode::Esc), None);
        assert!(app.import_dialog.is_none());
        assert!(app.take_import_request().is_none());

        // Running state swallows keys (including Esc) until the host reports.
        app.open_import_dialog();
        app.handle_key(key(KeyCode::Char('z')), None);
        app.handle_key(key(KeyCode::Enter), None);
        app.show_import_report("p", vec![], None); // host path: goes straight to report
        app.import_dialog = Some(ImportDialog::Running {
            path: "p".to_string(),
        });
        app.handle_key(key(KeyCode::Esc), None);
        assert!(app.import_dialog.is_some(), "running state ignores Esc");
    }

    #[test]
    fn pack_picker_row_opens_picker_and_enter_emits_swap_request() {
        let mut app = App::new(sample_view(0));
        app.show_home();
        app.set_current_pack_dir("stories/rainy-reunion");
        // Home row 4 ("팩 선택") requests the host to scan + open the picker;
        // row 5 is the exit, kept last.
        app.choose_home(4);
        assert_eq!(
            app.take_home_request(),
            Some(HomeRequest::PickPack),
            "row 4 asks the host for a pack scan"
        );
        app.choose_home(5);
        assert!(app.should_quit, "row 5 is 종료");
        // Host path: scan results come back through open_pack_picker.
        app.open_pack_picker(
            vec![
                "stories/rainy-reunion".to_string(),
                "stories/mira-ko".to_string(),
            ],
            vec![
                "비가 그친 뒤 · ko-KR".to_string(),
                "Mira · ko-KR".to_string(),
            ],
            "stories/rainy-reunion",
        );
        assert!(app.pack_picker.is_some(), "picker is open");
        assert!(app.home_visible, "home stays visible underneath");
        app.handle_key(key(KeyCode::Down), None); // → index 1 (mira-ko)
        app.handle_key(key(KeyCode::Enter), None);
        assert!(app.pack_picker.is_none(), "picker closes on Enter");
        assert_eq!(
            app.take_pack_swap_request().as_deref(),
            Some("stories/mira-ko")
        );
    }

    #[test]
    fn pack_picker_esc_cancels_without_swap() {
        let mut app = App::new(sample_view(0));
        app.show_home();
        app.open_pack_picker(
            vec!["stories/rainy-reunion".to_string()],
            vec!["비가 그친 뒤 · ko-KR".to_string()],
            "stories/rainy-reunion",
        );
        app.handle_key(key(KeyCode::Esc), None);
        assert!(app.pack_picker.is_none());
        assert!(app.take_pack_swap_request().is_none(), "no pack, no swap");
    }

    #[test]
    fn pack_picker_marks_the_current_pack_and_tolerates_an_empty_scan() {
        let mut app = App::new(sample_view(0));
        app.open_pack_picker(
            vec!["stories/x".to_string()],
            vec!["어떤 팩 · ko-KR".to_string()],
            "stories/x",
        );
        assert_eq!(app.pack_picker, Some(0), "current pack is highlighted");

        let mut empty = App::new(sample_view(0));
        empty.open_pack_picker(vec![], vec![], "stories/none");
        assert!(
            empty.pack_picker.is_some(),
            "empty scan still opens the picker"
        );
        empty.handle_key(key(KeyCode::Enter), None);
        assert!(
            empty.take_pack_swap_request().is_none(),
            "an empty list must not emit a swap"
        );
    }

    #[test]
    fn render_draws_the_import_dialog_over_home() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use unicode_width::UnicodeWidthStr;

        let mut app = App::new(sample_view(0));
        app.show_home();
        app.open_import_dialog();
        app.handle_paste("stories/card.png");
        let mut terminal = Terminal::new(TestBackend::new(90, 30)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame, frame.area()))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            // Skip the spacer cell that follows a double-width symbol so
            // Hangul sequences stay contiguous.
            let mut previous_was_wide = false;
            for x in 0..buffer.area.width {
                let symbol = buffer[(x, y)].symbol();
                if previous_was_wide && symbol == " " {
                    previous_was_wide = false;
                    continue;
                }
                previous_was_wide = symbol.width() == 2;
                out.push_str(symbol);
            }
            out.push('\n');
        }
        assert!(out.contains("카드 가져오기"), "modal title renders: {out}");
        assert!(out.contains("> stories/card.png"), "path renders: {out}");
        assert!(out.contains("시작 메뉴"), "home still visible underneath");
    }

    #[test]
    fn greetings_gate_new_game_and_picker_requests_start() {
        let mut app = App::new(sample_view(0));
        app.view.status.turn = 0;
        app.show_home();
        app.set_greetings(vec![
            "기본 오프닝".to_string(),
            "비 오는 날의 인사".to_string(),
            "3년 전의 재회".to_string(),
        ]);

        // Enter on 새 게임 opens the picker instead of starting.
        app.handle_key(key(KeyCode::Enter), None);
        assert!(app.greeting_picker.is_some(), "picker opens");
        assert!(app.take_home_request().is_none(), "start is gated");

        // Down twice → index 2, Enter requests StartNew with the pick.
        app.handle_key(key(KeyCode::Down), None);
        app.handle_key(key(KeyCode::Down), None);
        app.handle_key(key(KeyCode::Enter), None);
        assert_eq!(app.take_greeting_selection(), Some(2));
        assert!(matches!(
            app.take_home_request(),
            Some(HomeRequest::StartNew)
        ));
        assert!(
            app.greeting_picker.is_none(),
            "picker closes after the pick"
        );
    }

    #[test]
    fn greeting_picker_esc_cancels_the_start() {
        let mut app = App::new(sample_view(0));
        app.view.status.turn = 0;
        app.show_home();
        app.set_greetings(vec!["기본 오프닝".to_string(), "다른 인사".to_string()]);
        app.handle_key(key(KeyCode::Enter), None);
        app.handle_key(key(KeyCode::Esc), None);
        assert!(app.greeting_picker.is_none());
        assert!(app.take_greeting_selection().is_none());
        assert!(
            app.take_home_request().is_none(),
            "canceled pick never starts"
        );
        // Home is still visible; a plain Enter now starts with the default.
        app.handle_key(key(KeyCode::Enter), None);
        assert!(app.greeting_picker.is_some(), "새 게임 re-opens the picker");
    }

    #[test]
    fn no_greetings_starts_immediately() {
        let mut app = App::new(sample_view(0));
        app.view.status.turn = 0;
        app.show_home();
        app.handle_key(key(KeyCode::Enter), None);
        assert!(app.greeting_picker.is_none(), "no picker without greetings");
        assert!(matches!(
            app.take_home_request(),
            Some(HomeRequest::StartNew)
        ));
    }

    #[test]
    fn tab_opens_browser_and_esc_restores_typed_buffer() {
        let mut app = App::new(sample_view(0));
        app.open_import_dialog();
        app.handle_paste("/tmp/my-card");
        app.handle_key(key(KeyCode::Tab), None);

        assert_eq!(app.take_browse_request().as_deref(), Some(""));
        assert!(app.take_browse_request().is_none(), "consumed once");

        // The host answers the listing; Esc goes back to path entry with
        // the typed text intact.
        app.open_file_browser(
            "/tmp",
            vec![("dir".to_string(), true), ("card.charx".to_string(), false)],
        );
        app.handle_key(key(KeyCode::Esc), None);
        if let Some(ImportDialog::Input { buffer }) = &app.import_dialog {
            assert_eq!(buffer.text(), "/tmp/my-card");
        } else {
            panic!("expected Input mode after Esc");
        }
    }

    #[test]
    fn browser_enter_on_dir_requests_listing_and_on_file_submits_import() {
        let mut app = App::new(sample_view(0));
        app.open_import_dialog();
        app.handle_key(key(KeyCode::Tab), None);
        app.open_file_browser(
            "/tmp/cards",
            vec![("sub".to_string(), true), ("a.charx".to_string(), false)],
        );

        app.handle_key(key(KeyCode::Enter), None);
        assert_eq!(app.take_browse_request().as_deref(), Some("/tmp/cards/sub"));

        // Descend and come back, then pick the card file.
        app.open_file_browser(
            "/tmp/cards",
            vec![("sub".to_string(), true), ("a.charx".to_string(), false)],
        );
        app.handle_key(key(KeyCode::Down), None);
        app.handle_key(key(KeyCode::Enter), None);
        assert!(app.take_browse_request().is_none(), "no listing request");
        assert_eq!(
            app.take_import_request().as_deref(),
            Some("/tmp/cards/a.charx")
        );
        assert!(matches!(
            app.import_dialog,
            Some(ImportDialog::Running { .. })
        ));
    }

    #[test]
    fn browser_backspace_requests_parent_directory() {
        let mut app = App::new(sample_view(0));
        app.open_import_dialog();
        app.handle_key(key(KeyCode::Tab), None);
        app.open_file_browser("/tmp/cards", Vec::new());

        app.handle_key(key(KeyCode::Backspace), None);
        assert_eq!(app.take_browse_request().as_deref(), Some("/tmp"));
    }

    #[test]
    fn picker_delete_requests_confirm_only_for_non_active_packs() {
        let mut app = App::new(sample_view(0));
        app.set_current_pack_dir("stories/alpha");
        app.open_pack_picker(
            vec!["stories/alpha".to_string(), "stories/beta".to_string()],
            vec!["알파 · ko-KR".to_string(), "베타 · ko-KR".to_string()],
            "stories/alpha",
        );
        // Cursor starts on the active pack (alpha): d is a no-op.
        app.handle_key(key(KeyCode::Char('d')), None);
        assert!(app.pack_delete_confirm.is_none(), "active pack protected");

        app.handle_key(key(KeyCode::Down), None);
        app.handle_key(key(KeyCode::Char('d')), None);
        assert_eq!(app.pack_delete_confirm, Some(1), "confirm opens for beta");
    }

    #[test]
    fn delete_confirm_enter_requests_deletion_and_esc_cancels() {
        let mut app = App::new(sample_view(0));
        app.set_current_pack_dir("stories/alpha");
        app.open_pack_picker(
            vec!["stories/alpha".to_string(), "stories/beta".to_string()],
            vec!["알파".to_string(), "베타".to_string()],
            "stories/alpha",
        );
        app.handle_key(key(KeyCode::Down), None);
        app.handle_key(key(KeyCode::Char('d')), None);

        app.handle_key(key(KeyCode::Esc), None);
        assert!(app.pack_delete_confirm.is_none(), "esc cancels");
        assert!(app.take_pack_delete_request().is_none());

        app.handle_key(key(KeyCode::Char('d')), None);
        app.handle_key(key(KeyCode::Enter), None);
        assert_eq!(
            app.take_pack_delete_request().as_deref(),
            Some("stories/beta")
        );
        assert!(app.pack_picker.is_none(), "picker closed while host works");
    }

    #[test]
    fn tab_browser_starts_in_the_last_import_directory() {
        let mut app = App::new(sample_view(0));
        app.open_import_dialog();
        // import_last_dir already holds the last import's folder.
        app.import_last_dir = Some("/tmp/cards".to_string());

        app.handle_key(key(KeyCode::Tab), None);
        assert_eq!(
            app.take_browse_request().as_deref(),
            Some("/tmp/cards"),
            "browser continues where the last import came from"
        );

        // Without history the host picks the start directory ("" sentinel).
        let mut app = App::new(sample_view(0));
        app.open_import_dialog();
        app.handle_key(key(KeyCode::Tab), None);
        assert_eq!(app.take_browse_request().as_deref(), Some(""));
    }

    #[test]
    fn picker_notice_is_set_after_open_and_cleared_by_next_open() {
        let mut app = App::new(sample_view(0));
        app.open_pack_picker(
            vec!["stories/alpha".to_string()],
            vec!["알파 · ko-KR".to_string()],
            "stories/alpha",
        );
        app.set_pack_picker_notice(Some("삭제 실패".to_string()));
        assert_eq!(app.pack_notice.as_deref(), Some("삭제 실패"));

        // A fresh scan (e.g. another deletion round) clears the stale notice.
        app.open_pack_picker(
            vec!["stories/alpha".to_string()],
            vec!["알파 · ko-KR".to_string()],
            "stories/alpha",
        );
        assert_eq!(app.pack_notice, None);
    }
}
