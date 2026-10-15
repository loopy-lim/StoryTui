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
use std::collections::BTreeMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use image::DynamicImage;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Text};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use story_core::engine_cmd::EngineCommand;
use story_model::uuid::Uuid;
use story_model::{ActionType, AppViewModel, ParsedAction};
use story_model::{ProviderConfigSafe, ProviderHealth};
use story_persistence::SlotMeta;

use crate::event::{KeyAction, SELECT_SENTINEL, map_key, parse_direct_select_index};
use crate::input::InputBuffer;
use crate::screens::home::{menu_selection_at, render_home};
use crate::screens::save_load::render_save_load;
use crate::screens::settings::{SettingsView, render_settings};
use crate::screens::story::{
    ListMode, StoryStackEntry, choice_index_at, home_shortcut_at, markdown_lines,
    model_picker_scroll, render_list, render_model_picker, render_provider_picker, render_story,
    story_stack_text, streaming_entry_lines,
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
}

/// The TTS slice of the Settings projection (plain display data).
#[derive(Clone, Debug)]
pub struct TtsDisplay {
    pub enabled: bool,
    pub backend: String,
    pub voice: Option<String>,
}

/// Card import dialog state: path input → running → report.
#[derive(Clone, Debug)]
enum ImportDialog {
    /// Typing the card file path.
    Input { buffer: InputBuffer },
    /// The host is running the import (keys ignored).
    Running { path: String },
    /// Finished. `ready` carries the pack dir the player can switch to.
    Report {
        path: String,
        lines: Vec<String>,
        ready: Option<String>,
    },
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
}

/// A scene image pre-encoded for the detected terminal graphics protocol.
/// Encoding happens once on `set_scene_image`; rendering is cheap per frame.
#[derive(Clone)]
pub struct SceneImage {
    pub protocol: Protocol,
    /// Encoded size in terminal cells (aspect-fit into the scene column).
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

/// Upper bound for the scene column so narration keeps the majority width.
const SCENE_IMAGE_MAX_COLS: u16 = 26;
const SCENE_IMAGE_MAX_ROWS: u16 = 14;
/// Narrowest narration column we accept before giving up on the image.
const SCENE_IMAGE_MIN_TEXT_COLS: u16 = 40;

impl SceneImage {
    /// Encode `image` fitted into the scene column bounds. `None` when the
    /// protocol encode fails; the player then simply sees text only.
    pub fn encode(picker: &Picker, image: DynamicImage) -> Option<Self> {
        let protocol = picker
            .new_protocol(
                image,
                ratatui::layout::Size::new(SCENE_IMAGE_MAX_COLS, SCENE_IMAGE_MAX_ROWS),
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

    /// Layout guard mirroring the renderer: does this image fit beside a
    /// readable narration column in `width`?
    pub fn fits_beside_narration(&self, width: u16) -> bool {
        width.saturating_sub(self.cols) >= SCENE_IMAGE_MIN_TEXT_COLS
    }
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
    /// Current pre-encoded scene image, if the pack ships one.
    scene_image: Option<SceneImage>,
    mouse_command: Option<EngineCommand>,
    /// Card import dialog (home menu "카드 가져오기"); `Some` = modal active.
    import_dialog: Option<ImportDialog>,
    /// Set when the player submits a card path; the host runs the import.
    import_request: Option<String>,
    /// Set when the player confirms the report; the host swaps the pack in.
    pack_swap_request: Option<String>,
    /// Chronological narration stack shown in the main story pane. Writes
    /// MUST go through [`Self::replace_story_stack`] (or `set_lists`) so the
    /// render cache is invalidated — the version field keys that cache.
    pub(crate) story_stack: Vec<StoryStackEntry>,
    /// Bumped on every `story_stack` replacement; keys the render cache.
    story_stack_version: u64,
    /// Composed stack lines for `story_stack_version` — building them means
    /// markdown-parsing the whole transcript, which must not happen per frame.
    stack_lines_cache: Option<(u64, Vec<Line<'static>>)>,

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
            scene_image: None,
            mouse_command: None,
            import_dialog: None,
            import_request: None,
            pack_swap_request: None,
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

    /// Open the card import dialog from the home menu.
    pub fn open_import_dialog(&mut self) {
        self.import_dialog = Some(ImportDialog::Input {
            buffer: InputBuffer::new(500),
        });
    }

    /// Consume a submitted card path; the host runs the import.
    pub fn take_import_request(&mut self) -> Option<String> {
        self.import_request.take()
    }

    /// Consume a confirmed pack directory; the host swaps it in.
    pub fn take_pack_swap_request(&mut self) -> Option<String> {
        self.pack_swap_request.take()
    }

    /// Present a finished import (or its failure) in the dialog.
    pub fn show_import_report(&mut self, path: &str, lines: Vec<String>, ready: Option<String>) {
        self.import_dialog = Some(ImportDialog::Report {
            path: path.to_string(),
            lines,
            ready,
        });
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
                    self.import_request = Some(path.clone());
                    *dialog = ImportDialog::Running { path };
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
        });
    }

    /// Record the terminal graphics capability. Call once at startup before
    /// the first `set_scene_image`.
    pub fn set_image_picker(&mut self, picker: Option<Picker>) {
        self.image_backend = picker.as_ref().map(|picker| match picker.protocol_type() {
            ratatui_image::picker::ProtocolType::Kitty => "kitty",
            ratatui_image::picker::ProtocolType::Sixel => "sixel",
            ratatui_image::picker::ProtocolType::Iterm2 => "iterm2",
            ratatui_image::picker::ProtocolType::Halfblocks => "halfblocks",
        });
        self.image_picker = picker;
    }

    /// Settings-screen image toggle. The cached scene image is kept, so
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

    /// Replace the scene image. Decoding is host-owned; encoding happens here
    /// once so per-frame rendering stays cheap. `None` clears the column.
    pub fn set_scene_image(&mut self, image: Option<DynamicImage>) {
        self.scene_image = match (image.as_ref(), self.image_picker.as_ref()) {
            (Some(image), Some(picker)) => SceneImage::encode(picker, image.clone()),
            _ => None,
        };
    }

    pub fn scene_image(&self) -> Option<&SceneImage> {
        if self.images_enabled {
            self.scene_image.as_ref()
        } else {
            None
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
    pub fn set_lists(
        &mut self,
        history: &[story_model::HistoryEntry],
        memories: &[story_model::MemoryEntry],
    ) {
        let previous_story_count = self.story_stack.len();
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
            .map(|entry| StoryStackEntry {
                turn: entry.turn,
                scene_id: entry.scene_id.clone(),
                model: self.generation_models.get(&entry.generation_id).cloned(),
                generation_secs: self.generation_durations.get(&entry.generation_id).copied(),
                player_action: entry
                    .player_action
                    .as_ref()
                    .map(|action| action.original_text.trim().to_string()),
                narration: entry.narration.clone(),
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
                KeyCode::Char('d' | 'ㅇ') if self.list_mode == Some(ListMode::ModelPicker) => {
                    self.selected_director_model =
                        self.model_items.get(self.model_selected).cloned();
                    self.list_mode = None;
                }
                KeyCode::Char('c' | 'ㅊ') if self.list_mode == Some(ListMode::ModelPicker) => {
                    self.selected_choice_model = self.model_items.get(self.model_selected).cloned();
                    self.list_mode = None;
                }
                KeyCode::Char('w' | 'ㅈ') if self.list_mode == Some(ListMode::ModelPicker) => {
                    self.selected_writer_model = self.model_items.get(self.model_selected).cloned();
                    self.list_mode = None;
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

    /// Render the Story screen for the current view.
    /// Compose the narration pane's text, reusing the cached stack lines
    /// unless the stack changed. Only the live "작성 중" entry is rebuilt
    /// per frame while a Writer stream is arriving.
    fn narration_text(&mut self) -> Text<'static> {
        if self.story_stack.is_empty() {
            return Text::from(markdown_lines(&self.view.narration));
        }
        // The stored version IS the cache key: a set_lists bump invalidates
        // the cache on the very next frame.
        let cache_matches = self
            .stack_lines_cache
            .as_ref()
            .is_some_and(|(version, _)| *version == self.story_stack_version);
        if !cache_matches {
            let lines = story_stack_text(&self.story_stack);
            self.stack_lines_cache = Some((self.story_stack_version, lines));
        }
        let mut lines = self
            .stack_lines_cache
            .as_ref()
            .map(|(_, lines)| lines.clone())
            .unwrap_or_default();
        if let Some(streaming) = self.streaming_narration.as_deref() {
            lines.extend(streaming_entry_lines(self.view.status.turn, streaming));
        }
        Text::from(lines)
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
    }

    fn render_base(&mut self, frame: &mut Frame, area: Rect) {
        self.last_render_area.set(area);
        if self.home_visible {
            render_home(frame, area, self.home_selected, self.can_continue);
            return;
        }
        if let Some(state) = &self.save_load {
            let (title, footer) = save_load_copy(state);
            render_save_load(frame, area, title, &state.slots, state.selected, footer);
            return;
        }
        if let Some(mode) = self.list_mode {
            if mode == ListMode::ModelPicker {
                render_model_picker(frame, area, &self.model_items, self.model_selected);
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
                    theme_name: "default",
                },
            );
            return;
        }
        let narration_text = self.narration_text();
        let max_scroll = render_story(
            frame,
            area,
            &self.view,
            self.selected,
            self.narration_scroll,
            self.input_mode,
            &self.input,
            narration_text,
            self.streaming_narration.as_deref(),
            self.generation_elapsed_seconds(),
            self.scene_image.as_ref(),
        );
        self.narration_max_scroll.set(max_scroll);
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

    /// Handle mouse-wheel scrolling without changing the selected choice.
    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        // The import dialog is a keyboard-only modal: clicks must not reach
        // the home menu or story screen underneath it.
        if self.import_dialog.is_some() {
            return;
        }
        if self.save_load.is_some() {
            return;
        }
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            let area = self.last_render_area.get();
            if self.home_visible {
                if let Some(selection) = menu_selection_at(area, mouse.row) {
                    self.home_selected = selection;
                    self.choose_home(selection);
                }
                return;
            }
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
            if self.settings_visible && mouse.row == area.y.saturating_add(4) {
                // The model row is the second line in the provider block.
                self.list_mode = Some(ListMode::ModelPicker);
                return;
            }
            if home_shortcut_at(area, &self.view, self.input_mode, mouse.column, mouse.row) {
                self.open_home_from_game();
                return;
            }
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
                self.home_selected = (self.home_selected + 1).min(4);
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
            4 => self.should_quit = true,
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

fn save_load_copy(state: &SaveLoadState) -> (&'static str, &'static str) {
    match (state.mode, state.overwrite_confirmation) {
        (SaveLoadMode::Save, true) => (
            "저장 / 덮어쓰기 확인",
            "Enter 한 번 더: 덮어쓰기 · Esc 취소 · S 저장 · L 불러오기",
        ),
        (SaveLoadMode::Save, false) => (
            "수동 저장",
            "↑↓ 이동 · Enter 저장 · S 저장 · L 불러오기 · Esc 닫기",
        ),
        (SaveLoadMode::Load, _) => (
            "저장 불러오기",
            "↑↓ 이동 · Enter 불러오기 · S 저장 · L 불러오기 · Esc 닫기",
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
    fn images_toggle_via_settings_key_flips_state() {
        let mut app = App::new(sample_view(0));
        assert!(app.images_enabled(), "images default to on");
        app.handle_key(key(KeyCode::Char('o')), None);
        app.handle_key(key(KeyCode::Char('i')), None);
        assert!(app.take_images_toggle_request(), "toggle requested");
        app.set_images_enabled(false);
        assert!(!app.images_enabled());

        // With images disabled the renderer sees no image, but the cached
        // scene survives: re-enabling is instant (no re-decode).
        let picker = ratatui_image::picker::Picker::halfblocks();
        app.set_image_picker(Some(picker));
        app.set_scene_image(Some(image::DynamicImage::new_rgb8(16, 16)));
        assert!(app.scene_image().is_none(), "disabled toggle hides image");
        app.set_images_enabled(true);
        assert!(app.scene_image().is_some(), "re-enable is instant");
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
        app.set_lists(&[entry(1, "첫 번째 장면")], &[]);
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

        app.set_lists(&entries[..1], &[]);
        assert_eq!(app.narration_scroll, 0, "first scene opens at the top");
        app.set_lists(&entries, &[]);

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
        app.set_lists(&entries, &[]);
        assert_eq!(app.story_stack[0].generation_secs, None);
        assert_eq!(app.story_stack[1].generation_secs, None);

        // Record durations keyed by generation_id (as the host does on the
        // completion path, before the committing dispatch).
        app.generation_durations.insert(g1, 47);
        app.generation_durations.insert(g2, 133);
        app.set_lists(&entries, &[]);
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

        // Paste lands in the path buffer too.
        app.open_import_dialog();
        app.handle_paste("/tmp/card.png");
        assert_eq!(
            app.take_import_request(),
            None,
            "paste alone does not submit"
        );
        app.handle_key(key(KeyCode::Enter), None);
        assert_eq!(app.take_import_request().as_deref(), Some("/tmp/card.png"));
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
}
