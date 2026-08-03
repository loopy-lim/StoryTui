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
use ratatui::Frame;
use ratatui::layout::Rect;
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
    ListMode, StoryStackEntry, choice_index_at, home_shortcut_at, render_list, render_model_picker,
    render_story,
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

#[derive(Clone, Debug)]
struct SettingsState {
    provider: ProviderConfigSafe,
    health: ProviderHealth,
    temperature: f32,
    top_p: f32,
    top_k: u32,
    max_tokens: u32,
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
    model_reload_requested: bool,
    mouse_command: Option<EngineCommand>,
    /// Chronological narration stack shown in the main story pane.
    pub story_stack: Vec<StoryStackEntry>,
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
}

impl App {
    /// Build an App from the first published view.
    pub fn new(view: AppViewModel) -> Self {
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
            model_reload_requested: false,
            mouse_command: None,
            story_stack: Vec::new(),
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
        }
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

    /// Consume a model-picker request to refresh the server list.
    pub fn take_model_reload_requested(&mut self) -> bool {
        std::mem::take(&mut self.model_reload_requested)
    }

    /// Consume an engine command requested through a mouse click.
    pub fn take_mouse_command(&mut self) -> Option<EngineCommand> {
        self.mouse_command.take()
    }

    /// Supply the redacted configuration and health details for the Settings
    /// screen. Secrets never enter this UI state.
    pub fn set_settings(
        &mut self,
        provider: ProviderConfigSafe,
        health: ProviderHealth,
        temperature: f32,
        top_p: f32,
        top_k: u32,
        max_tokens: u32,
    ) {
        self.settings = Some(SettingsState {
            provider,
            health,
            temperature,
            top_p,
            top_k,
            max_tokens,
        });
    }

    pub fn set_can_continue(&mut self, can_continue: bool) {
        self.can_continue = can_continue;
        if !can_continue && self.home_selected == 1 {
            self.home_selected = 0;
        }
    }

    /// Show the launcher. Kept safe so headless Story-screen consumers
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
        if self.view.loading.is_none() {
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
        self.story_stack = history
            .iter()
            .map(|entry| StoryStackEntry {
                turn: entry.turn,
                scene_id: entry.scene_id.clone(),
                model: self.generation_models.get(&entry.generation_id).cloned(),
                player_action: entry
                    .player_action
                    .as_ref()
                    .map(|action| action.original_text.trim().to_string()),
                narration: entry.narration.clone(),
            })
            .collect();
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
        if self.home_visible {
            self.handle_home_key(key);
            return None;
        }
        if self.save_load.is_some() {
            self.handle_save_load_key(key);
            return None;
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
                // Same physical keys under Korean 2-beolsik: s=ㄴ, l=ㅣ.
                KeyCode::Char('s' | 'ㄴ') => {
                    self.save_load_request = Some(SaveLoadRequest::Open(SaveLoadMode::Save));
                    return None;
                }
                KeyCode::Char('l' | 'ㅣ') => {
                    self.save_load_request = Some(SaveLoadRequest::Open(SaveLoadMode::Load));
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

    /// Render the Story screen for the current view.
    pub fn render(&self, frame: &mut Frame, area: Rect) {
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
            let items = match mode {
                ListMode::History => &self.history_items,
                ListMode::Memory => &self.memory_items,
                ListMode::ModelPicker => unreachable!("rendered above"),
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
                    theme_name: "default",
                },
            );
            return;
        }
        let max_scroll = render_story(
            frame,
            area,
            &self.view,
            self.selected,
            self.narration_scroll,
            self.input_mode,
            &self.input,
            &self.story_stack,
            self.streaming_narration.as_deref(),
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
                let index = mouse.row.saturating_sub(area.y.saturating_add(1)) as usize;
                if let Some(model) = self.model_items.get(index).cloned() {
                    self.model_selected = index;
                    self.selected_model = Some(model);
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
            KeyCode::Char('s' | 'ㄴ') => {
                state.mode = SaveLoadMode::Save;
                state.overwrite_confirmation = false;
            }
            KeyCode::Char('l' | 'ㅣ') => {
                state.mode = SaveLoadMode::Load;
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
                self.home_selected = (self.home_selected + 1).min(3);
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
            3 => self.should_quit = true,
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
    }

    #[test]
    fn leaving_and_reentering_settings_keeps_model_picker_available() {
        let mut app = App::new(sample_view(0));
        app.set_settings(
            ProviderConfigSafe {
                base_url: "http://127.0.0.1:8000".to_string(),
                model: "a".to_string(),
                director_model: None,
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
            row: 18,
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
}
