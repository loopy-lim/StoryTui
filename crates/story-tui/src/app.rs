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
use ratatui::text::Line;
use ratatui_image::picker::Picker;
use ratatui_image::sliced::SlicedProtocol;
use story_core::engine_cmd::EngineCommand;
use story_model::uuid::Uuid;
use story_model::{ActionType, AppViewModel, Emotion, LocationId, ParsedAction};
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
    footer_action_at, markdown_lines, model_picker_scroll, render_help_overlay, render_list,
    render_model_picker, render_provider_picker, render_story, story_entry_lines,
    streaming_entry_lines,
};

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
    /// Two-stage Director → Writer generation path (streaming narration).
    use_director_writer: bool,
    /// Whether expression sprites are currently shown.
    expressions_enabled: bool,
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
    /// Player toggle from the Settings screen — rendering skips when off.
    images_enabled: bool,
    /// Set when the player toggled images in Settings (`i`).
    images_toggle_requested: bool,
    /// Set when the player toggled the generation path in Settings (`g`).
    generation_toggle_requested: bool,
    /// Player toggle for expression sprites from the Settings screen (`f`).
    expressions_enabled: bool,
    /// Set when the player toggled expressions in Settings (`f`).
    expressions_toggle_requested: bool,
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
            images_enabled: true,
            images_toggle_requested: false,
            generation_toggle_requested: false,
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
            pack_delete_request: None,
            pack_swap_request: None,
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
        expressions_enabled: bool,
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
            use_director_writer,
            expressions_enabled,
        });
    }

    /// Record the terminal graphics capability. Call once at startup before
    /// the first `set_location_image`.
    pub fn set_image_picker(&mut self, picker: Option<Picker>) {
        self.image_backend = picker.as_ref().map(|picker| match picker.protocol_type() {
            ratatui_image::picker::ProtocolType::Kitty => "kitty",
            ratatui_image::picker::ProtocolType::Sixel => "sixel",
            ratatui_image::picker::ProtocolType::Iterm2 => "iterm2",
            ratatui_image::picker::ProtocolType::Halfblocks => "halfblocks",
        });
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

    /// Consume an image toggle request from the Settings screen.
    pub fn take_images_toggle_request(&mut self) -> bool {
        std::mem::take(&mut self.images_toggle_requested)
    }

    /// Consume a generation-path toggle request from the Settings screen.
    pub fn take_generation_toggle_request(&mut self) -> bool {
        std::mem::take(&mut self.generation_toggle_requested)
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

    /// Patch only the image section of the Settings state (the `i` toggle).
    pub fn update_settings_images(&mut self, images_enabled: bool, image_backend: Option<String>) {
        if let Some(settings) = self.settings.as_mut() {
            settings.images_enabled = images_enabled;
            settings.image_backend = image_backend;
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

    pub fn set_can_continue(&mut self, can_continue: bool) {
        self.can_continue = can_continue;
        if !can_continue && self.home_selected == 1 {
            self.home_selected = 0;
        }
    }

    /// Show the launcher. Kept explicit so headless Story-screen consumers
    /// can still construct an `App` directly without a launcher transition.
    pub fn show_home(&mut self) {
        self.home_visible = true;
        self.home_continue_returns_to_game = false;
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
    pub fn set_view(&mut self, view: AppViewModel) {
        let previous_turn = self.view.status.turn;
        let count = view.choices.len();
        if count == 0 {
            self.selected = 0;
        } else if self.selected >= count {
            self.selected = count - 1;
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
        let previous_story_count = self.story_stack.len();
        self.initial_location = Some(initial_location.0.clone());
        let timeline = location_timeline(history, initial_location);
        let emotions = emotion_timeline(history);
        self.history_items = history
            .iter()
            .rev()
            .map(|entry| {
                let preview = entry.narration.lines().next().unwrap_or_default();
                format!("턴 {} · {} — {}", entry.turn, entry.scene_id, preview)
            })
            .collect();
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
                    return None;
                }
                KeyCode::Char('h' | 'ㅗ') => {
                    self.list_mode = Some(ListMode::History);
                    self.list_scroll = 0;
                    return None;
                }
                KeyCode::Char('m' | 'ㅡ') => {
                    self.list_mode = Some(ListMode::Memory);
                    self.list_scroll = 0;
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
                // Same physical key under Korean 2-beolsik: f=ㄹ.
                KeyCode::Char('f' | 'ㄹ') => {
                    self.expressions_toggle_requested = true;
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
        // logically fixed. Only cancellation and quitting remain available.
        if self.view.loading.is_some()
            && !matches!(key.code, KeyCode::Char('c' | 'ㅊ' | 'q' | 'ㅂ'))
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
                .map(|(index, entry)| story_entry_lines(entry, index == 0))
                .collect();
            self.stack_lines_cache = Some((self.story_stack_version, lines));
        }
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
                &self.view.narration,
            ))));
            if let Some(streaming) = self.streaming_narration.as_deref() {
                blocks.push(StoryBlock::Text(std::borrow::Cow::Owned(
                    streaming_entry_lines(self.view.status.turn, streaming),
                )));
            }
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
            blocks.push(StoryBlock::Text(std::borrow::Cow::Owned(
                streaming_entry_lines(self.view.status.turn, streaming),
            )));
        }
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
            let items = match mode {
                ListMode::History => &self.history_items,
                ListMode::Memory => &self.memory_items,
                ListMode::ModelPicker | ListMode::ProviderPicker => unreachable!("rendered above"),
            };
            render_list(frame, area, mode, items, self.list_scroll);
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
                    use_director_writer: settings.use_director_writer,
                    expressions_enabled: settings.expressions_enabled,
                    theme_name: "default",
                    returns_home: self.settings_returns_home,
                },
            );
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
                                    action: Box::new(ParsedAction {
                                        action_type: ActionType::GeneralAction,
                                        target: None,
                                        intent: None,
                                        tone: None,
                                        content: text.trim().to_string(),
                                        original_text: text,
                                        confidence: Some(0.0),
                                    }),
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
                    action: Box::new(ParsedAction {
                        action_type: ActionType::GeneralAction,
                        target: None,
                        intent: None,
                        tone: None,
                        content: text.trim().to_string(),
                        original_text: text,
                        confidence: Some(0.0),
                    }),
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
    use story_model::{ChoiceView, RiskLevel, Screen, StatusLine, TimeSlot};

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
        app.set_image_picker(Some(picker));
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

    /// Count Image blocks the story pane would render for the current state.
    fn image_block_count(app: &mut App) -> usize {
        app.fill_block_cache();
        app.story_blocks()
            .iter()
            .filter(|block| matches!(block, StoryBlock::Image(_)))
            .count()
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
        app.set_image_picker(Some(ratatui_image::picker::Picker::halfblocks()));
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
    fn home_menu_emits_start_and_continue_requests() {
        let mut app = App::new(sample_view(0));
        app.set_can_continue(true);
        app.show_home();

        app.handle_key(key(KeyCode::Enter), None);
        assert_eq!(app.take_home_request(), Some(HomeRequest::StartNew));

        app.show_home();
        app.handle_key(key(KeyCode::Down), None);
        app.handle_key(key(KeyCode::Enter), None);
        assert_eq!(app.take_home_request(), Some(HomeRequest::Continue));
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
