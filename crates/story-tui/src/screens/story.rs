//! Story screen renderer (ST-021).
//!
//! Renders an [`AppViewModel`] into ratatui widgets inside a given `Rect`:
//! a title rule, narration (Korean-aware wrap), a selectable choice list with
//! risk indicators, a status line, and loading / error / min-size overlays.
//!
//! All Korean text is rendered via [`ratatui::text::Line`] / `Paragraph` with
//! `Wrap { trim: false }`; ratatui 0.30 measures display width through
//! `unicode-width`, so double-width Hangul aligns correctly.

use crate::input::InputBuffer;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};
use ratatui_image::sliced::{SignedPosition, SlicedImage};
use story_model::{AppViewModel, RiskLevel, TimeSlot};
use unicode_width::UnicodeWidthStr;

use crate::app::SceneImage;

use super::keymap::keymap_lines_two_columns;
use super::model_display_label;
use super::popup::{POPUP_WIDTH_WIDE, centered_rect, render_min_size_warning};

/// Minimum supported terminal size for the Story screen.
pub const MIN_WIDTH: u16 = 60;
pub const MIN_HEIGHT: u16 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListMode {
    History,
    Memory,
    ModelPicker,
    ProviderPicker,
}

/// One immutable block in the chronological story transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoryStackEntry {
    pub turn: u32,
    pub scene_id: String,
    /// Model used for this response when known in the current runtime.
    pub model: Option<String>,
    /// Wall-clock seconds this turn took to generate, when known.
    pub generation_secs: Option<u64>,
    pub player_action: Option<String>,
    pub narration: String,
    /// Location this turn happened at (folded from the history's state
    /// patches by `location_timeline`). `None` only for hand-built test
    /// entries; drives inline scene images.
    pub location: Option<String>,
}

/// One renderable unit of the transcript, in chronological order. The
/// narration pane lays blocks out vertically: a scene image inserted before
/// the turn that entered a new location, then that turn's text.
#[derive(Clone)]
pub enum StoryBlock<'a> {
    /// A turn's text (header, meta, action, narration) or the opening text.
    Text(std::borrow::Cow<'a, [Line<'static>]>),
    /// A location image at a scene boundary.
    Image(&'a SceneImage),
}

/// Blank rows rendered below an inline image so it never touches the `────`
/// rule of the following turn block.
pub const IMAGE_BLOCK_SPACER_ROWS: u16 = 1;

pub fn render_list(frame: &mut Frame, area: Rect, mode: ListMode, items: &[String], scroll: u16) {
    let title = match mode {
        ListMode::History => "이전 생성 기록",
        ListMode::Memory => "기억 목록",
        ListMode::ModelPicker => "모델 선택",
        ListMode::ProviderPicker => "프로바이더 선택",
    };
    let rows: Vec<Line> = if items.is_empty() {
        vec![Line::from("아직 기록이 없습니다.")]
    } else {
        items.iter().map(|item| Line::from(item.clone())).collect()
    };
    let paragraph = Paragraph::new(rows)
        .scroll((scroll, 0))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(crate::theme::BORDER_TYPE)
                .border_style(crate::theme::Styles::border_primary())
                .title(Span::styled(
                    format!(" {} · PgUp/PgDn 스크롤 · Esc 닫기 ", title),
                    crate::theme::Styles::title(),
                )),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(Clear, area);
    frame.render_widget(paragraph, area);
}

/// Which model currently holds each generation role, for picker badges.
///
/// The strings are raw model ids kept ONLY for equality comparison — the
/// picker still prints the opaque [`model_display_label`], never the id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelRoleHints {
    pub base: Option<String>,
    pub director: Option<String>,
    pub choice: Option<String>,
    pub writer: Option<String>,
}

impl ModelRoleHints {
    /// The ` · 기본 · 본문` badge suffix for `model`, or an empty string.
    fn badges_for(&self, model: &str) -> String {
        let mut roles = Vec::new();
        if self.base.as_deref() == Some(model) {
            roles.push("기본");
        }
        if self.director.as_deref() == Some(model) {
            roles.push("기획");
        }
        if self.choice.as_deref() == Some(model) {
            roles.push("선택지");
        }
        if self.writer.as_deref() == Some(model) {
            roles.push("본문");
        }
        if roles.is_empty() {
            String::new()
        } else {
            format!("  · {}", roles.join("·"))
        }
    }
}

pub fn render_model_picker(
    frame: &mut Frame,
    area: Rect,
    items: &[String],
    selected: usize,
    roles: &ModelRoleHints,
) {
    let rows: Vec<Line> = if items.is_empty() {
        vec![Line::from("모델 목록을 불러오지 못했습니다.")]
    } else {
        items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let active = index == selected.min(items.len().saturating_sub(1));
                Line::styled(
                    format!(
                        "{} {}{}",
                        if active { "▶" } else { " " },
                        model_display_label(item),
                        roles.badges_for(item)
                    ),
                    if active {
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                            .add_modifier(Modifier::REVERSED)
                    } else {
                        Style::default().fg(Color::Gray)
                    },
                )
            })
            .collect()
    };
    let paragraph = Paragraph::new(rows)
        .scroll((model_picker_scroll(area, selected), 0))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::LightCyan))
                .title("모델 선택 · ↑↓ 이동 · Enter 기본 · D 기획 · C 선택지 · W 본문 (연속 지정) · R 새로고침 · Esc 닫기"),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(Clear, area);
    frame.render_widget(paragraph, area);
}

/// Render the provider picker: the configured profiles by name, with the
/// base_url hint so a name alone never forces a trip to the TOML file.
pub fn render_provider_picker(
    frame: &mut Frame,
    area: Rect,
    items: &[(String, String)],
    selected: usize,
) {
    let rows: Vec<Line> = if items.is_empty() {
        vec![Line::from("구성된 프로바이더가 없습니다.")]
    } else {
        items
            .iter()
            .enumerate()
            .map(|(index, (name, base_url))| {
                let active = index == selected.min(items.len().saturating_sub(1));
                Line::styled(
                    format!(
                        "{indicator} {name}  ·  {base_url}",
                        indicator = if active { "▶" } else { " " },
                    ),
                    if active {
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                            .add_modifier(Modifier::REVERSED)
                    } else {
                        Style::default().fg(Color::Gray)
                    },
                )
            })
            .collect()
    };
    let paragraph = Paragraph::new(rows)
        .scroll((model_picker_scroll(area, selected), 0))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::LightCyan))
                .title("프로바이더 선택 · ↑↓ 이동 · Enter 전환 · Esc 닫기"),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(Clear, area);
    frame.render_widget(paragraph, area);
}

pub fn model_picker_scroll(area: Rect, selected: usize) -> u16 {
    let visible_rows = area.height.saturating_sub(2) as usize;
    selected.saturating_sub(visible_rows.saturating_sub(1)) as u16
}

/// Render the Story screen for `view`, highlighting `selected` (clamped to the
/// choice count). `selected` is an index into `view.choices`.
#[allow(clippy::too_many_arguments)] // rendering inputs stay explicit at the UI boundary
pub fn render_story(
    frame: &mut Frame,
    area: Rect,
    view: &AppViewModel,
    selected: usize,
    narration_scroll: u16,
    input_mode: bool,
    input: &InputBuffer,
    blocks: &[StoryBlock<'_>],
    streaming_narration: Option<&str>,
    generation_elapsed_secs: Option<u64>,
) -> u16 {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        render_min_size_warning(frame, area, MIN_WIDTH, MIN_HEIGHT);
        return 0;
    }

    // Title rule.
    let title_line = Line::from(vec![
        Span::styled(
            view.story_title.clone(),
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(Color::DarkGray)),
        Span::styled(view.chapter.clone(), Style::default().fg(Color::Gray)),
        Span::styled(" · ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            view.location_display.clone(),
            Style::default().fg(Color::Gray),
        ),
        Span::styled(" · ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            time_slot_label(view.time_slot),
            Style::default().fg(Color::LightCyan),
        ),
    ]);
    let frame_chunks = Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).split(area);
    let title_block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(Color::DarkGray));
    let title_para = Paragraph::new(title_line)
        .block(title_block)
        .wrap(Wrap { trim: false });
    frame.render_widget(title_para, frame_chunks[0]);

    // The transcript scrolls; the current controls stay docked at the bottom.
    let body = frame_chunks[1];
    let choice_height = choice_panel_height(view, body.width);
    let chunks = if input_mode {
        Layout::vertical([
            Constraint::Fill(3),
            Constraint::Length(choice_height),
            Constraint::Length(3),
            Constraint::Length(3),
        ])
        .split(body)
    } else {
        Layout::vertical([
            Constraint::Fill(3),
            Constraint::Length(choice_height),
            Constraint::Length(3),
        ])
        .split(body)
    };
    // The transcript owns the full narration region: scene images are rows
    // IN the transcript (inserted at scene boundaries), not a column beside
    // it, so they scroll together with the prose.
    let narration_region = chunks[0];
    // Decide the scrollbar against the width the text occupies *with* the
    // scrollbar present, so the one-column reservation cannot flip the
    // decision it feeds. When it fits at the narrower width it also fits at
    // the wider one, so the choice stays stable frame to frame.
    let decide_width = narration_region.width.saturating_sub(1).max(1);
    // Pass 1: each block's visual height. Text heights wrap at `decide_width`
    // (see the text render below for why that is exact when it matters);
    // image heights are exact cell counts.
    let mut spans: Vec<(usize, u16)> = Vec::with_capacity(blocks.len());
    let mut total_rows: usize = 0;
    for block in blocks {
        let height = match block {
            StoryBlock::Text(lines) => wrapped_line_count(lines, decide_width) as u16,
            StoryBlock::Image(image) => image.rows.saturating_add(IMAGE_BLOCK_SPACER_ROWS),
        };
        spans.push((total_rows, height));
        total_rows += height as usize;
    }
    let needs_scrollbar = total_rows > narration_region.height as usize;
    let narration_chunks = if needs_scrollbar {
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(1)]).split(narration_region)
    } else {
        [narration_region, Rect::default()].into()
    };
    let narration_area = narration_chunks[0];
    // Mouse wheels and PageDown can generate an offset beyond the wrapped
    // content. Clamp it here so the pane never becomes mysteriously blank.
    let max_scroll = total_rows.saturating_sub(narration_area.height as usize) as u16;
    let scroll_position = narration_scroll.min(max_scroll);
    // Pass 2: render only the blocks intersecting the viewport. Text heights
    // were computed at `decide_width`; with a scrollbar the text renders at
    // exactly that width, so the counts are exact. Without a scrollbar the
    // scroll clamps to zero — every block renders whole from its start — so a
    // height computed at the narrower width is only an unused upper bound.
    let viewport_top = scroll_position as usize;
    let viewport_bottom = viewport_top + narration_area.height as usize;
    for (block, (start, height)) in blocks.iter().zip(&spans) {
        let end = start + *height as usize;
        if end <= viewport_top || *start >= viewport_bottom {
            continue;
        }
        match block {
            StoryBlock::Text(lines) => {
                // Wrap is per logical line, so a per-block paragraph produces
                // the same rows the old whole-transcript paragraph did.
                let paragraph = Paragraph::new(Text::from(lines.as_ref()))
                    .wrap(Wrap { trim: false })
                    .style(Style::default().fg(Color::Gray))
                    .scroll((viewport_top.saturating_sub(*start) as u16, 0));
                let visible_top = (*start).max(viewport_top);
                let visible_bottom = end.min(viewport_bottom);
                let rect = Rect {
                    x: narration_area.x,
                    y: narration_area.y + (visible_top - viewport_top) as u16,
                    width: narration_area.width,
                    height: (visible_bottom - visible_top) as u16,
                };
                frame.render_widget(paragraph, rect);
            }
            StoryBlock::Image(image) => {
                // Position is relative to the viewport and negative when the
                // image starts above it. The intersection guard bounds `y` to
                // (-rows, viewport_height) — comfortably inside i16.
                // SlicedImage maps skip/drop so a partially scrolled image
                // shows the correct slice; plain `Image` would always pin the
                // image's TOP rows to the visible rect.
                let y = *start as i32 - scroll_position as i32;
                let widget = SlicedImage::new(&image.protocol, SignedPosition::from((0, y as i16)));
                frame.render_widget(widget, narration_area);
            }
        }
    }
    if needs_scrollbar {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .thumb_symbol("┃")
            .thumb_style(Style::default().fg(Color::LightCyan))
            .track_symbol(Some("│"))
            .track_style(Style::default().fg(Color::DarkGray))
            .begin_symbol(None)
            .end_symbol(None);
        // `ScrollbarState::content_length` is the number of reachable scroll
        // positions for a wrapped paragraph, not its total visual line count.
        // Feeding it `content_lines` leaves the thumb stranded in the middle
        // even at the final line; `max_scroll + 1` makes the last offset map
        // to the bottom of the track.
        let mut scrollbar_state = ScrollbarState::new(max_scroll as usize + 1)
            .position(scroll_position as usize)
            .viewport_content_length(narration_area.height as usize);
        frame.render_stateful_widget(scrollbar, narration_chunks[1], &mut scrollbar_state);
    }
    render_choice_panel(
        frame,
        chunks[1],
        view,
        selected,
        generation_elapsed_secs,
        streaming_narration,
    );
    if input_mode {
        render_input_panel(frame, chunks[2], input);
        render_status_panel(frame, chunks[3], view, true);
    } else {
        render_status_panel(frame, chunks[2], view, false);
    }

    // Overlays on top.
    if let Some(ref err) = view.error {
        render_error_overlay(frame, area, err);
    }
    max_scroll
}

/// Return the choice under `row` using the same wrapped-line layout as the
/// renderer. The host uses this to turn a left click into a choice command.
pub fn choice_index_at(
    area: Rect,
    view: &AppViewModel,
    input_mode: bool,
    row: u16,
) -> Option<usize> {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT || view.loading.is_some() {
        return None;
    }
    let frame_chunks = Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).split(area);
    let body = frame_chunks[1];
    let choice_height = choice_panel_height(view, body.width);
    let chunks = if input_mode {
        Layout::vertical([
            Constraint::Fill(3),
            Constraint::Length(choice_height),
            Constraint::Length(3),
            Constraint::Length(3),
        ])
        .split(body)
    } else {
        Layout::vertical([
            Constraint::Fill(3),
            Constraint::Length(choice_height),
            Constraint::Length(3),
        ])
        .split(body)
    };
    let choice_area = chunks[1];
    let content_start = choice_area.y.saturating_add(1);
    let mut cursor = content_start;
    let width = choice_area.width.saturating_sub(4).max(1) as usize;
    for (index, choice) in view.choices.iter().enumerate() {
        let line_count = choice_lines(
            index,
            &choice.label,
            choice.preview.as_deref(),
            choice.risk,
            width,
        )
        .len() as u16;
        if row >= cursor && row < cursor.saturating_add(line_count) {
            return Some(index);
        }
        cursor = cursor.saturating_add(line_count);
    }
    None
}

/// Actions triggered from mouse clicks on the story footer shortcuts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoryFooterAction {
    Home,
    NewGame,
    Input,
    Save,
    Load,
    Settings,
    History,
    Memory,
    Regenerate,
    Help,
    InputSubmit,
    InputCancel,
}

/// Resolve a click on the shortcut footer to a specific action.
pub fn footer_action_at(
    area: Rect,
    view: &AppViewModel,
    input_mode: bool,
    column: u16,
    row: u16,
) -> Option<StoryFooterAction> {
    if area.width < MIN_WIDTH
        || area.height < MIN_HEIGHT
        || view.loading.is_some()
        || view.error.is_some()
    {
        return None;
    }
    let frame_chunks = Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).split(area);
    let body = frame_chunks[1];
    let choice_height = choice_panel_height(view, body.width);
    let status_area = if input_mode {
        Layout::vertical([
            Constraint::Fill(3),
            Constraint::Length(choice_height),
            Constraint::Length(3),
            Constraint::Length(3),
        ])
        .split(body)[3]
    } else {
        Layout::vertical([
            Constraint::Fill(3),
            Constraint::Length(choice_height),
            Constraint::Length(3),
        ])
        .split(body)[2]
    };
    if row != status_area.y.saturating_add(2) {
        return None;
    }
    let mut cursor = area.x;
    for (key, label) in active_footer_segments(view, input_mode, area.width) {
        let seg_width = (key.width() + label.width()) as u16;
        if column >= cursor && column < cursor.saturating_add(seg_width) {
            return match key {
                "[Home]" => Some(StoryFooterAction::Home),
                "[N]" => Some(StoryFooterAction::NewGame),
                "[I]" => Some(StoryFooterAction::Input),
                "[S]" => Some(StoryFooterAction::Save),
                "[L]" => Some(StoryFooterAction::Load),
                "[O]" => Some(StoryFooterAction::Settings),
                "[H]" => Some(StoryFooterAction::History),
                "[M]" => Some(StoryFooterAction::Memory),
                "[R]" => Some(StoryFooterAction::Regenerate),
                "[?]" => Some(StoryFooterAction::Help),
                "[Enter]" => Some(StoryFooterAction::InputSubmit),
                "[Esc]" => Some(StoryFooterAction::InputCancel),
                _ => None,
            };
        }
        cursor = cursor.saturating_add(seg_width + FOOTER_SEPARATOR.width() as u16);
    }
    None
}

/// Whether a click lands on the visible `[Home] 홈` segment of the shortcut
/// footer. Keeping the hit test beside the renderer — both resolve the same
/// [`active_footer_segments`] layout — prevents the label from becoming a
/// keyboard-only affordance.
pub fn home_shortcut_at(
    area: Rect,
    view: &AppViewModel,
    input_mode: bool,
    column: u16,
    row: u16,
) -> bool {
    footer_action_at(area, view, input_mode, column, row) == Some(StoryFooterAction::Home)
}

/// Count the visual lines `text` occupies when word-wrapped at `width`
/// columns.
///
/// ratatui's `WordWrapper` moves a word that does not fit onto the next
/// visual line, hard-breaking only when a single word is wider than the
/// area. A per-line `width().div_ceil()` undercounts wrapped prose — which
/// used to leave both the scrollbar geometry and the auto-scroll bottom short
/// of the real content end. The greedy simulation below matches the wrapper
/// for space-separated Korean prose; only a trailing-whitespace edge can
/// differ by one line, which the scroll clamp absorbs.
fn wrapped_line_count(lines: &[Line<'_>], width: u16) -> usize {
    let width = width.max(1) as usize;
    lines
        .iter()
        .map(|line| {
            // Lines that already fit need no simulation — skip the allocation.
            if line.width() <= width {
                1
            } else {
                greedy_wrap_count(&line.to_string(), width)
            }
        })
        .sum()
}

/// Visual line count for one logical line under greedy word wrap at `width`.
fn greedy_wrap_count(line: &str, width: usize) -> usize {
    if line.width() <= width {
        return 1;
    }
    let mut lines = 0usize;
    let mut current = 0usize; // display width of the visual line being filled
    let mut current_counted = false; // whether `current`'s line is in `lines`
    for word in line.split(' ') {
        let word_width = word.width();
        if word_width == 0 {
            continue;
        }
        if word_width > width {
            // One run wider than the area: hard-break char by char — 2-wide
            // Hangul rarely divides the width evenly — exactly as the
            // wrapper does.
            if current > 0 && !current_counted {
                lines += 1;
            }
            let mut fill = 0usize;
            let mut word_lines = 1usize;
            for ch in word.chars() {
                let char_width = ch.to_string().width();
                if fill + char_width > width {
                    word_lines += 1;
                    fill = 0;
                }
                fill += char_width;
            }
            lines += word_lines;
            current = fill;
            current_counted = true;
            continue;
        }
        let joined = if current == 0 {
            word_width
        } else {
            current + 1 + word_width
        };
        if joined <= width {
            current = joined;
            continue;
        }
        if current > 0 && !current_counted {
            lines += 1;
        }
        current = word_width;
        current_counted = false;
    }
    if current > 0 && !current_counted {
        lines += 1;
    }
    lines.max(1)
}

/// Render a wall-clock duration (seconds) as a compact Korean label:
/// `47초`, `2분 13초`, `1시간 5분`.
fn fmt_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}초")
    } else if secs < 3600 {
        format!("{}분 {}초", secs / 60, secs % 60)
    } else {
        format!("{}시간 {}분", secs / 3600, (secs % 3600) / 60)
    }
}

/// Lines for the live "작성 중" entry appended while a Writer stream is
/// arriving. Mirrors the header/body shape of a committed stack entry.
pub(crate) fn streaming_entry_lines(turn: u32, streaming: &str) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "────────────────────────────────────────",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("턴 {} · {}", turn.saturating_add(1), "작성 중"),
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        )),
    ];
    lines.extend(markdown_lines(streaming));
    lines
}

/// Compose one committed stack entry into renderable lines. `first` suppresses
/// the separating blank/rule/blank chrome (the transcript's very first entry
/// opens the pane directly). Exported so the App can cache the result per
/// entry and interleave image blocks between them.
pub(crate) fn story_entry_lines(entry: &StoryStackEntry, first: bool) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if !first {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "────────────────────────────────────────",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(""));
    }
    lines.push(Line::from(Span::styled(
        format!("턴 {} · {}", entry.turn, entry.scene_id),
        Style::default()
            .fg(Color::LightCyan)
            .add_modifier(Modifier::BOLD),
    )));
    // Per-turn metadata: the model and how long the generation took,
    // joined by " · " on one subtle line.
    let mut meta: Vec<String> = Vec::new();
    if let Some(model) = &entry.model {
        meta.push(format!("모델 · {}", model_display_label(model)));
    }
    if let Some(secs) = entry.generation_secs {
        meta.push(format!("생성 {}", fmt_duration(secs)));
    }
    if !meta.is_empty() {
        lines.push(Line::from(Span::styled(
            meta.join(" · "),
            Style::default().fg(Color::DarkGray),
        )));
    }
    if let Some(action) = &entry.player_action {
        lines.push(Line::from(Span::styled(
            "당신의 선택",
            Style::default()
                .fg(Color::LightYellow)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            action.clone(),
            Style::default().fg(Color::Yellow),
        )));
        lines.push(Line::from(""));
    }
    lines.extend(markdown_lines(&entry.narration));
    lines
}

pub(crate) fn markdown_lines(input: &str) -> Vec<Line<'static>> {
    input
        .lines()
        .map(|raw| {
            if let Some((speaker, dialogue)) = speaker_dialogue_line(raw) {
                let speaker_style = if matches!(speaker, "당신" | "플레이어") {
                    Style::default()
                        .fg(Color::LightCyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(Color::LightMagenta)
                        .add_modifier(Modifier::BOLD)
                };
                let dialogue_style = Style::default().fg(Color::LightYellow);
                let mut spans = vec![Span::styled(format!("{speaker}: "), speaker_style)];
                spans.extend(inline_markdown(dialogue, dialogue_style));
                return Line::from(spans);
            }
            let dialogue = raw
                .trim_start()
                .chars()
                .next()
                .is_some_and(|c| matches!(c, '"' | '“' | '「' | '『'));
            let (prefix, body, style) = if let Some(rest) = raw.strip_prefix("### ") {
                (
                    "",
                    rest,
                    Style::default()
                        .fg(Color::LightCyan)
                        .add_modifier(Modifier::BOLD),
                )
            } else if let Some(rest) = raw.strip_prefix("## ") {
                (
                    "",
                    rest,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )
            } else if let Some(rest) = raw.strip_prefix("# ") {
                (
                    "",
                    rest,
                    Style::default()
                        .fg(Color::LightCyan)
                        .add_modifier(Modifier::BOLD),
                )
            } else if let Some(rest) = raw.strip_prefix("- ").or_else(|| raw.strip_prefix("* ")) {
                ("• ", rest, Style::default().fg(Color::Gray))
            } else if let Some(rest) = raw.strip_prefix("> ") {
                ("│ ", rest, Style::default().fg(Color::LightYellow))
            } else {
                (
                    "",
                    raw,
                    if dialogue {
                        Style::default().fg(Color::LightYellow)
                    } else {
                        Style::default().fg(Color::Reset)
                    },
                )
            };
            let mut spans = vec![Span::styled(prefix.to_string(), style)];
            spans.extend(inline_markdown(body, style));
            Line::from(spans)
        })
        .collect()
}

/// Recognize only the explicit dialogue contract used in generated narration.
///
/// Keeping this presentation-only and conservative avoids treating ordinary
/// prose such as URLs or explanations with a colon as a speaker label.
fn speaker_dialogue_line(raw: &str) -> Option<(&str, &str)> {
    let trimmed = raw.trim();
    let separator = trimmed.find([':', '：'])?;
    let (speaker, rest) = trimmed.split_at(separator);
    let speaker = speaker.trim();
    let separator_len = rest.chars().next()?.len_utf8();
    let dialogue = rest[separator_len..].trim_start();
    let begins_with_quote = dialogue
        .chars()
        .next()
        .is_some_and(|c| matches!(c, '"' | '“' | '「' | '『'));

    (begins_with_quote
        && !speaker.is_empty()
        && speaker.chars().count() <= 24
        && !speaker.chars().any(char::is_whitespace))
    .then_some((speaker, dialogue))
}

fn inline_markdown(input: &str, base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut rest = input;
    while let Some((start, marker)) = next_markdown_marker(rest) {
        if start > 0 {
            spans.push(Span::styled(rest[..start].to_string(), base));
        }
        let after = &rest[start + marker.len()..];
        if marker == "[" {
            let Some(label_end) = after.find("](") else {
                spans.push(Span::styled(rest[start..].to_string(), base));
                return spans;
            };
            let Some(url_end) = after[label_end + 2..].find(')') else {
                spans.push(Span::styled(rest[start..].to_string(), base));
                return spans;
            };
            spans.push(Span::styled(
                after[..label_end].to_string(),
                base.fg(Color::LightBlue).add_modifier(Modifier::UNDERLINED),
            ));
            rest = &after[label_end + 2 + url_end + 1..];
            continue;
        }
        let Some(end) = after.find(marker) else {
            spans.push(Span::styled(rest[start..].to_string(), base));
            return spans;
        };
        let style = match marker {
            "~~" => base.add_modifier(Modifier::CROSSED_OUT),
            "`" => base.fg(Color::LightMagenta).add_modifier(Modifier::BOLD),
            "*" | "_" => base.add_modifier(Modifier::ITALIC),
            _ => base.add_modifier(Modifier::BOLD),
        };
        spans.push(Span::styled(after[..end].to_string(), style));
        rest = &after[end + marker.len()..];
    }
    if !rest.is_empty() {
        spans.push(Span::styled(rest.to_string(), base));
    }
    spans
}

fn next_markdown_marker(input: &str) -> Option<(usize, &'static str)> {
    ["**", "__", "~~", "`", "*", "_", "["]
        .into_iter()
        .filter_map(|marker| input.find(marker).map(|index| (index, marker)))
        .min_by_key(|(index, marker)| (*index, std::cmp::Reverse(marker.len())))
}

fn choice_panel_height(view: &AppViewModel, width: u16) -> u16 {
    let rows = if view.loading.is_some() {
        2
    } else {
        view.choices
            .iter()
            .enumerate()
            .map(|(index, choice)| {
                choice_lines(
                    index,
                    &choice.label,
                    choice.preview.as_deref(),
                    choice.risk,
                    width.saturating_sub(4).max(1) as usize,
                )
                .len()
            })
            .sum::<usize>()
            .max(1)
    };
    (rows as u16).saturating_add(2)
}

fn render_choice_panel(
    frame: &mut Frame,
    area: Rect,
    view: &AppViewModel,
    selected: usize,
    elapsed_secs: Option<u64>,
    streaming_narration: Option<&str>,
) {
    let turn = view.status.turn;
    let elapsed_mins = turn * 30;
    let hours = elapsed_mins / 60;
    let mins = elapsed_mins % 60;
    let elapsed_label = if turn == 0 {
        "시작 (0분)".to_string()
    } else if hours > 0 && mins > 0 {
        format!("{hours}시간 {mins}분 경과")
    } else if hours > 0 {
        format!("{hours}시간 경과")
    } else {
        format!("{mins}분 경과")
    };

    let avail_width = area.width.saturating_sub(4) as usize;
    let full_title = format!(
        "선택지 · 시간대: {} · 누적: {} (턴 {})",
        time_slot_label(view.time_slot),
        elapsed_label,
        turn
    );
    let med_title = format!(
        "선택지 · 시간대: {} (턴 {})",
        time_slot_label(view.time_slot),
        turn
    );
    let short_title = format!("선택지 (턴 {})", turn);

    let title_text = if full_title.width() <= avail_width {
        full_title
    } else if med_title.width() <= avail_width {
        med_title
    } else if short_title.width() <= avail_width {
        short_title
    } else {
        "선택지".to_string()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(crate::theme::BORDER_TYPE)
        .border_style(crate::theme::Styles::border_muted())
        .title(Span::styled(title_text, crate::theme::Styles::title()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let lines = if view.loading.is_some() {
        // Infer the active stage from the Writer stream: once prose chunks
        // arrive we are in the writing stage, otherwise the Director is still
        // planning (or a single-pass generation is running).
        let stage_label = if streaming_narration.is_some() {
            "본문 작성 중..."
        } else {
            "장면 기획 중..."
        };
        let timer_subtext = match elapsed_secs {
            Some(secs) => {
                let m = secs / 60;
                let s = secs % 60;
                if m > 0 {
                    format!("생성 진행 중... ({m}분 {s}초 소요 중) [C] 취소")
                } else {
                    format!("생성 진행 중... ({s}초 소요 중) [C] 취소")
                }
            }
            None => "선택지 생성 대기 중... [C] 취소".to_string(),
        };
        vec![
            Line::from(Span::styled(
                stage_label.to_string(),
                Style::default().fg(Color::LightYellow),
            )),
            Line::from(Span::styled(
                timer_subtext,
                Style::default().fg(Color::DarkGray),
            )),
        ]
    } else {
        let mut lines = Vec::new();
        let width = inner.width.saturating_sub(2).max(1) as usize;
        for (index, choice) in view.choices.iter().enumerate() {
            let highlight = index == selected.min(view.choices.len().saturating_sub(1));
            for (line_index, mut line) in choice_lines(
                index,
                &choice.label,
                choice.preview.as_deref(),
                choice.risk,
                width,
            )
            .into_iter()
            .enumerate()
            {
                line.spans.insert(
                    0,
                    Span::raw(if line_index == 0 && highlight {
                        "▶ "
                    } else {
                        "  "
                    }),
                );
                if highlight {
                    line.style = Style::default()
                        .fg(crate::theme::Palette::SELECTED)
                        .add_modifier(Modifier::BOLD);
                }
                lines.push(line);
            }
        }
        lines
    };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// Separator between footer segments.
const FOOTER_SEPARATOR: &str = "  ";

/// One shortcut footer segment: the bracketed key and its Korean label
/// (leading space included).
type FooterSegment = (&'static str, &'static str);

/// The shortcut footer segments that render at `width`.
///
/// The footer advertises the keys the story screen actually binds — not just
/// the five it used to show. Segments are dropped by tier as the terminal
/// narrows so the line never overflows: tier 2 (`[R]`/`[H]`/`[M]` detail)
/// first, then `[O]`; the core (`[I]`/`[S]`/`[L]`/`[Home]`/`[?]`) survives
/// at the minimum width. While typing, the footer swaps to the input
/// panel's keys.
fn active_footer_segments(view: &AppViewModel, input_mode: bool, width: u16) -> Vec<FooterSegment> {
    if input_mode {
        return vec![("[Enter]", " 전송"), ("[Esc]", " 취소")];
    }
    if view.status.turn == 0 && view.choices.is_empty() && view.loading.is_none() {
        return vec![("[N]", " 게임 시작"), ("[?]", " 도움말")];
    }
    let has_choices = !view.choices.is_empty();
    // (visible, key, label, tier) — tier 2 shows only at the widest step.
    let candidates: [(bool, &'static str, &'static str, u8); 9] = [
        (has_choices, "[R]", " 선택지 다시 만들기", 2),
        (has_choices, "[I]", " 직접 입력", 0),
        (true, "[H]", " 기록", 2),
        (true, "[M]", " 기억", 2),
        (true, "[S]", " 저장", 0),
        (true, "[L]", " 불러오기", 0),
        (view.status.turn > 0, "[Home]", " 홈", 0),
        (true, "[O]", " 설정", 1),
        (true, "[?]", " 도움말", 0),
    ];
    for tier in [2u8, 1, 0] {
        let active: Vec<FooterSegment> = candidates
            .iter()
            .filter(|(visible, _, _, segment_tier)| *visible && *segment_tier <= tier)
            .map(|&(_, key, label, _)| (key, label))
            .collect();
        let total: usize = active
            .iter()
            .map(|(key, label)| key.width() + label.width())
            .sum::<usize>()
            + FOOTER_SEPARATOR.width() * active.len().saturating_sub(1);
        if total <= width as usize {
            return active;
        }
    }
    // Even the core set does not fit: help is the one affordance that
    // survives at any width.
    vec![("[?]", " 도움말")]
}

/// Build the footer line as styled spans: the bracketed key in cyan, the
/// label in dim gray.
fn footer_spans(view: &AppViewModel, input_mode: bool, width: u16) -> Vec<Span<'static>> {
    let key_style = crate::theme::Styles::keycap();
    let label_style = crate::theme::Styles::button_label();
    let mut spans = Vec::new();
    for (index, (key, label)) in active_footer_segments(view, input_mode, width)
        .into_iter()
        .enumerate()
    {
        if index > 0 {
            spans.push(Span::styled(FOOTER_SEPARATOR, label_style));
        }
        spans.push(Span::styled(key.to_string(), key_style));
        spans.push(Span::styled(label.to_string(), label_style));
    }
    spans
}

fn render_input_panel(frame: &mut Frame, area: Rect, input: &InputBuffer) {
    // The grapheme counter stays visible so long pastes never surprise the
    // player with silent truncation.
    let title = format!(
        " 직접 입력 ({}/{}) · Enter 전송 · Esc 취소 ",
        input.char_count(),
        input.max_chars()
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(crate::theme::BORDER_TYPE)
        .border_style(crate::theme::Styles::border_primary())
        .title(Span::styled(title, crate::theme::Styles::title()));
    let (before, after) = input.split_at_cursor();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(before),
            Span::styled("▌", Style::default().fg(crate::theme::Palette::SELECTED)),
            Span::raw(after),
        ]))
        .style(Style::default().fg(crate::theme::Palette::PROSE))
        .block(block),
        area,
    );
}

fn render_status_panel(frame: &mut Frame, area: Rect, view: &AppViewModel, input_mode: bool) {
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::DarkGray));
    frame.render_widget(
        Paragraph::new(status_lines(view, area.width, input_mode)).block(block),
        area,
    );
}

fn choice_lines(
    index: usize,
    label: &str,
    preview: Option<&str>,
    risk: RiskLevel,
    width: usize,
) -> Vec<Line<'static>> {
    let marker = format!("{}. ", index + 1);
    let continuation = " ".repeat(marker.width());
    let mut lines = wrap_choice_label(label, &marker, &continuation, width);
    let risk_text = format!("  ({})", risk_label(risk));
    let last_width = lines.last().map_or(0, Line::width);
    if last_width + risk_text.width() <= width {
        lines
            .last_mut()
            .expect("choice labels always have one line")
            .spans
            .push(Span::styled(
                risk_text,
                Style::default().fg(risk_color(risk)),
            ));
    } else {
        lines.push(Line::from(vec![
            Span::raw(continuation),
            Span::styled(risk_text, Style::default().fg(risk_color(risk))),
        ]));
    }
    if let Some(preview) = preview.map(str::trim).filter(|preview| !preview.is_empty()) {
        let preview_lines = wrap_choice_label(preview, "   ↳ ", "     ", width);
        lines.extend(preview_lines.into_iter().map(|mut line| {
            line.style = Style::default().fg(Color::DarkGray);
            line
        }));
    }
    lines
}

fn wrap_choice_label(
    label: &str,
    first_prefix: &str,
    continuation: &str,
    width: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut prefix = first_prefix.to_string();
    let mut body = String::new();
    for word in label.split_whitespace() {
        let separator = if body.is_empty() { "" } else { " " };
        if prefix.width() + body.width() + separator.width() + word.width() <= width {
            body.push_str(separator);
            body.push_str(word);
            continue;
        }
        if !body.is_empty() {
            lines.push(Line::from(format!("{prefix}{body}")));
            prefix = continuation.to_string();
            body.clear();
        }
        append_wrapped_word(
            &mut lines,
            &mut prefix,
            &mut body,
            word,
            continuation,
            width,
        );
    }
    if body.is_empty() && lines.is_empty() {
        lines.push(Line::from(first_prefix.to_string()));
    } else if !body.is_empty() {
        lines.push(Line::from(format!("{prefix}{body}")));
    }
    lines
}

fn append_wrapped_word(
    lines: &mut Vec<Line<'static>>,
    prefix: &mut String,
    body: &mut String,
    word: &str,
    continuation: &str,
    width: usize,
) {
    for ch in word.chars() {
        let ch_width = ch.to_string().width();
        if prefix.width() + body.width() + ch_width > width && !body.is_empty() {
            lines.push(Line::from(format!("{prefix}{body}")));
            *prefix = continuation.to_string();
            body.clear();
        }
        body.push(ch);
    }
}

fn status_lines(view: &AppViewModel, width: u16, input_mode: bool) -> Vec<Line<'static>> {
    let values = Line::from(status_values_prefix(view));
    if input_mode {
        return vec![values, Line::from(footer_spans(view, true, width))];
    }
    if view.status.turn == 0 && view.choices.is_empty() && view.loading.is_none() {
        // First screen: the one action that starts the story, plus where to
        // find the rest of the keys.
        vec![
            values,
            Line::from(vec![
                Span::styled(
                    "[N]",
                    Style::default()
                        .fg(Color::LightCyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(" 게임 시작", Style::default().fg(Color::DarkGray)),
                Span::styled(FOOTER_SEPARATOR, Style::default().fg(Color::DarkGray)),
                Span::styled("[?]", Style::default().fg(Color::LightCyan)),
                Span::styled(" 도움말", Style::default().fg(Color::DarkGray)),
            ]),
        ]
    } else if view.loading.is_none() && view.error.is_none() {
        vec![values, Line::from(footer_spans(view, false, width))]
    } else {
        vec![values]
    }
}

fn status_values_prefix(view: &AppViewModel) -> String {
    let stat = |label: &str, v: Option<i16>| match v {
        Some(n) => format!("{label}:{n}"),
        None => format!("{label}:—"),
    };
    format!(
        "{}  {}  {}  턴: {}",
        stat("애정", view.status.affection),
        stat("신뢰", view.status.trust),
        stat("긴장", view.status.tension),
        view.status.turn
    )
}

fn render_error_overlay(frame: &mut Frame, area: Rect, err: &story_model::DomainErrorView) {
    let title = format!("오류: {}", err.title_ko);
    let body = Text::from(vec![
        Line::from(err.message_ko.clone()),
        Line::from(""),
        Line::from(Span::styled(
            "[R] 다시 시도   [Esc] 닫기".to_string(),
            Style::default().add_modifier(Modifier::DIM),
        )),
    ]);
    let w = (err.message_ko.width() as u16 + 4)
        .max(title.width() as u16 + 4)
        .min(area.width);
    // Server/protocol details can be much wider than a terminal. Paragraph
    // does not wrap by default, so calculate the wrapped body height before
    // centering the overlay; the message then remains inside its border.
    let inner_width = w.saturating_sub(2).max(1);
    let h = (wrapped_line_count(&body.lines, inner_width) as u16 + 2)
        .max(5)
        .min(area.height);
    let centered = centered_rect(area, w, h);
    frame.render_widget(Clear, centered);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red))
        .title(Span::styled(
            title,
            Style::default().add_modifier(Modifier::BOLD),
        ));
    let para = Paragraph::new(body).block(block).wrap(Wrap { trim: false });
    frame.render_widget(para, centered);
}

/// The `?` help overlay: the full keymap, two columns, centered over the
/// story screen. The same table the Settings screen renders.
pub fn render_help_overlay(frame: &mut Frame, area: Rect) {
    let (left, right) = keymap_lines_two_columns();
    let popup = centered_rect(area, POPUP_WIDTH_WIDE, 20);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::LightCyan))
        .title("키 안내 · ? 또는 Esc로 닫기");
    let body = block.inner(popup);
    let columns =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(body);
    frame.render_widget(block, popup);
    frame.render_widget(Paragraph::new(left).wrap(Wrap { trim: false }), columns[0]);
    frame.render_widget(Paragraph::new(right).wrap(Wrap { trim: false }), columns[1]);
}

fn time_slot_label(t: TimeSlot) -> &'static str {
    match t {
        TimeSlot::Dawn => "새벽",
        TimeSlot::Morning => "오전",
        TimeSlot::Afternoon => "오후",
        TimeSlot::Evening => "저녁",
        TimeSlot::Night => "밤",
    }
}

fn risk_label(r: RiskLevel) -> &'static str {
    match r {
        RiskLevel::Safe => "안전",
        RiskLevel::Low => "낮음",
        RiskLevel::Medium => "보통",
        RiskLevel::High => "높음",
        RiskLevel::Severe => "심각",
    }
}

fn risk_color(r: RiskLevel) -> Color {
    match r {
        RiskLevel::Safe => Color::Green,
        RiskLevel::Low => Color::Cyan,
        RiskLevel::Medium => Color::Yellow,
        RiskLevel::High => Color::LightRed,
        RiskLevel::Severe => Color::Red,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SceneImage, StoryBlock, choice_lines, fmt_duration, markdown_lines, render_story,
        speaker_dialogue_line,
    };
    use crate::input::InputBuffer;
    use image::DynamicImage;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui_image::picker::Picker;
    use story_model::{AppViewModel, ChoiceView, RiskLevel, StatusLine, TimeSlot};
    use unicode_width::UnicodeWidthStr;

    fn sample_view() -> AppViewModel {
        AppViewModel {
            screen: story_model::Screen::Story,
            story_title: "테스트".to_string(),
            chapter: "chapter-1".to_string(),
            location_display: "카페".to_string(),
            time_slot: TimeSlot::Evening,
            narration: "비가 그쳤다.\n\n그녀는 창밖을 보았다.".to_string(),
            choices: vec![ChoiceView {
                id: "c1".to_string(),
                label: "사과한다".to_string(),
                risk: RiskLevel::Low,
                preview: None,
            }],
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

    /// Transcript blocks for the sample narration, optionally opening with a
    /// scene image (as a scene boundary would).
    fn sample_blocks<'a>(scene: Option<&'a SceneImage>) -> Vec<StoryBlock<'a>> {
        let mut blocks: Vec<StoryBlock<'a>> = Vec::new();
        if let Some(scene) = scene {
            blocks.push(StoryBlock::Image(scene));
        }
        blocks.push(StoryBlock::Text(std::borrow::Cow::Owned(markdown_lines(
            &sample_view().narration,
        ))));
        blocks
    }

    fn draw_terminal(
        blocks: &[StoryBlock<'_>],
        scroll: u16,
        width: u16,
        height: u16,
    ) -> Terminal<TestBackend> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                render_story(
                    frame,
                    frame.area(),
                    &sample_view(),
                    0,
                    scroll,
                    false,
                    &InputBuffer::new(200),
                    blocks,
                    None,
                    None,
                );
            })
            .expect("draw");
        terminal
    }

    fn draw_with(scene: Option<&SceneImage>, width: u16, height: u16) -> String {
        let blocks = sample_blocks(scene);
        let terminal = draw_terminal(&blocks, 0, width, height);
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            // A double-width cell is followed by a literal " " spacer; skip
            // it so Hangul sequences stay contiguous.
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

    /// A checkerboard forces distinct halfblock glyphs/colors; a uniform
    /// image would encode as background-colored spaces.
    fn checkerboard(width: u32, height: u32) -> DynamicImage {
        let mut raw = image::Rgb32FImage::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let value = if (x + y) % 2 == 0 { 1.0 } else { 0.0 };
                raw.put_pixel(x, y, image::Rgb([value, value, value]));
            }
        }
        DynamicImage::from(raw)
    }

    #[test]
    fn inline_scene_image_precedes_its_turn_block() {
        let scene = SceneImage::encode(&Picker::halfblocks(), checkerboard(64, 64))
            .expect("halfblocks encode");
        let out = draw_with(Some(&scene), 100, 30);
        assert!(out.contains("비가 그쳤다"), "narration must remain: {out}");
        assert!(out.contains("사과한다"), "choices must remain: {out}");

        // The image occupies the transcript's first rows (left-aligned, below
        // the title rule): painted cells, not blank ones.
        let terminal = draw_terminal(&sample_blocks(Some(&scene)), 0, 100, 30);
        let buffer = terminal.backend().buffer();
        let image_top = 2; // below the title rule
        let image_bottom = (image_top + scene.rows).min(buffer.area.height);
        let mut painted = 0usize;
        for y in image_top..image_bottom {
            for x in 0..scene.cols {
                let cell = &buffer[(x, y)];
                if cell.symbol() != " "
                    || cell.fg != ratatui::style::Color::Reset
                    || cell.bg != ratatui::style::Color::Reset
                {
                    painted += 1;
                }
            }
        }
        assert!(
            painted >= scene.cols as usize,
            "inline image rows must be painted (painted={painted}, cols={})",
            scene.cols
        );
    }

    #[test]
    fn no_scene_image_keeps_plain_layout() {
        let out = draw_with(None, 100, 30);
        assert!(
            out.contains("비가 그쳤다"),
            "plain layout lost narration: {out}"
        );
        assert!(out.contains("사과한다"), "plain layout lost choices: {out}");
    }

    #[test]
    fn inline_image_renders_at_min_width() {
        let scene = SceneImage::encode(&Picker::halfblocks(), checkerboard(64, 64))
            .expect("halfblocks encode");
        // MIN_WIDTH is 60: a ≤26-col image fits on its own rows with the
        // transcript full-width — no column, no fallback needed.
        let out = draw_with(Some(&scene), 60, 30);
        assert!(out.contains("비가 그쳤다"), "text follows the image: {out}");
    }

    #[test]
    fn scrollbar_track_is_contiguous_for_the_whole_transcript() {
        // Regression: an undercounting wrap estimate used to make the track
        // geometry disagree with the text, and the scrollbar column showed
        // gaps mid-transcript. The track must render on every narration row
        // while the scrollbar is visible — including across inline image
        // rows, which now live inside the transcript.
        let view = sample_view();
        let mut long = String::new();
        for index in 0..40 {
            long.push_str(&format!(
                "{index}번째 줄입니다. 비가 그친 카페의 창밖을 봅니다.\n\n"
            ));
        }
        let text_only = vec![StoryBlock::Text(std::borrow::Cow::Owned(markdown_lines(
            &long,
        )))];
        let mut terminal = Terminal::new(TestBackend::new(100, 34)).expect("terminal");
        let mut max_scroll = 0u16;
        let blocks = text_only.clone();
        terminal
            .draw(|frame| {
                max_scroll = render_story(
                    frame,
                    frame.area(),
                    &view,
                    0,
                    5,
                    false,
                    &InputBuffer::new(200),
                    &blocks,
                    None,
                    None,
                );
            })
            .expect("draw");
        assert!(max_scroll > 0, "long transcript must be scrollable");
        let buffer = terminal.backend().buffer();
        // The scrollbar owns the last column of the narration area, which
        // spans from below the title rule to above the choices panel.
        let column = buffer.area.width - 1;
        let missing: Vec<u16> = (3..22)
            .filter(|y| {
                let symbol = buffer[(column, *y)].symbol();
                symbol != "│" && symbol != "┃"
            })
            .collect();
        assert!(
            missing.is_empty(),
            "track missing at rows {missing:?} in column {column}"
        );

        // With an inline image the same contiguity must hold: the track is
        // still the last column, and image rows must not steal track cells.
        let scene = SceneImage::encode(&Picker::halfblocks(), checkerboard(64, 64))
            .expect("halfblocks encode");
        let mut blocks: Vec<StoryBlock<'_>> = vec![StoryBlock::Image(&scene)];
        blocks.extend(text_only);
        let mut terminal = Terminal::new(TestBackend::new(100, 34)).expect("terminal");
        terminal
            .draw(|frame| {
                render_story(
                    frame,
                    frame.area(),
                    &view,
                    0,
                    5,
                    false,
                    &InputBuffer::new(200),
                    &blocks,
                    None,
                    None,
                );
            })
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let missing: Vec<u16> = (3..22)
            .filter(|y| {
                let symbol = buffer[(column, *y)].symbol();
                symbol != "│" && symbol != "┃"
            })
            .collect();
        assert!(
            missing.is_empty(),
            "track missing across image rows at {missing:?} in column {column}"
        );
    }

    #[test]
    fn scrolling_past_an_image_reveals_the_lower_slice() {
        // Row-encoded image: pixel row p paints red ≈ p/27, so every halfblock
        // cell row carries a distinct color signature. This is the tripwire
        // for the slice regression: a top-pinned render (plain `Image`
        // clipping) would repeat the image's FIRST cell row at the viewport
        // top after scrolling instead of the correct lower slice.
        let mut raw = image::Rgb32FImage::new(26, 28);
        for y in 0..28u32 {
            for x in 0..26u32 {
                let value = 0.05 + 0.9 * (y as f32 / 27.0);
                raw.put_pixel(x, y, image::Rgb([value, 0.0, 0.0]));
            }
        }
        let scene =
            SceneImage::encode(&Picker::halfblocks(), DynamicImage::from(raw)).expect("encode");
        let mut long = String::new();
        for index in 0..40 {
            long.push_str(&format!(
                "{index}번째 줄입니다. 스크롤 여유용 본문입니다.\n\n"
            ));
        }
        let colors_at = |scroll: u16| {
            let blocks = vec![
                StoryBlock::Image(&scene),
                StoryBlock::Text(std::borrow::Cow::Owned(markdown_lines(&long))),
            ];
            let terminal = draw_terminal(&blocks, scroll, 100, 34);
            let buffer = terminal.backend().buffer();
            let top = &buffer[(0, 2)]; // first visible transcript row
            let next = &buffer[(0, 3)];
            ((top.fg, top.bg), (next.fg, next.bg))
        };
        let (top_at_zero, second_at_zero) = colors_at(0);
        let (top_at_one, _) = colors_at(1);
        assert_ne!(
            top_at_zero, top_at_one,
            "scrolling by one row must advance the visible image slice"
        );
        assert_eq!(
            top_at_one, second_at_zero,
            "the slice shown at scroll 1 must be the image's SECOND cell row, not a re-pinned top"
        );
    }

    #[test]
    fn greedy_wrap_matches_the_word_wrapper() {
        use super::greedy_wrap_count;
        // Fits: one visual line.
        assert_eq!(greedy_wrap_count("비가 그쳤다", 30), 1);
        // Words moved whole to the next line.
        assert_eq!(greedy_wrap_count("안녕 하세요 반갑습니다", 12), 2);
        // "안녕" / "하세요" / hard-broken "반갑습니"+"다".
        assert_eq!(greedy_wrap_count("안녕 하세요 반갑습니다", 7), 4);
        // A run wider than the area hard-breaks on 2-wide boundaries.
        assert_eq!(greedy_wrap_count("가나다라마바사", 4), 4);
        // Width 3 fits one 2-wide syllable per line: seven of them.
        assert_eq!(greedy_wrap_count("가나다라마바사", 3), 7);
        // Mixed: a normal word then an over-long run.
        assert_eq!(greedy_wrap_count("안녕 가나다라마바사", 4), 5);
    }

    #[test]
    fn wrap_count_feeds_a_reachable_scroll_bottom() {
        // Regression: div_ceil per logical line undercounted wrapped prose,
        // so max_scroll landed above the real last line. The word-aware count
        // must report every visual line the paragraph will render.
        use super::{markdown_lines, wrapped_line_count};
        let lines = markdown_lines(
            "짙은 남색 코트는 옆 의자에 반듯하게 접혀 있고 오른손의 은색 시계가 빛난다",
        );
        let count = wrapped_line_count(&lines, 30);
        // The single 43-wide logical line cannot fit in fewer than 2 lines.
        assert!(count >= 2, "wrapped prose undercounted: {count}");
    }

    #[test]
    fn footer_advertises_every_bound_key_when_wide() {
        use super::active_footer_segments;
        let view = sample_view();
        let segments = active_footer_segments(&view, false, 200);
        let keys: Vec<_> = segments.iter().map(|(key, _)| *key).collect();
        for expected in [
            "[R]", "[I]", "[H]", "[M]", "[S]", "[L]", "[Home]", "[O]", "[?]",
        ] {
            assert!(
                keys.contains(&expected),
                "wide footer is missing {expected}"
            );
        }
    }

    #[test]
    fn footer_narrows_without_losing_the_core() {
        use super::active_footer_segments;
        let view = sample_view();
        // At the minimum supported width the whole core still fits.
        let min_width = active_footer_segments(&view, false, 60);
        let keys: Vec<_> = min_width.iter().map(|(key, _)| *key).collect();
        assert!(keys.contains(&"[?]"), "help survives the minimum width");
        assert!(!keys.contains(&"[R]"), "tier-2 detail drops first");
        assert!(keys.contains(&"[Home]"), "home survives the minimum width");
        // Below that, only the help affordance remains.
        let tiny = active_footer_segments(&view, false, 30);
        assert_eq!(tiny, vec![("[?]", " 도움말")]);
    }

    #[test]
    fn input_mode_footer_swaps_to_editor_keys() {
        use super::active_footer_segments;
        let view = sample_view();
        let segments = active_footer_segments(&view, true, 80);
        assert_eq!(segments, vec![("[Enter]", " 전송"), ("[Esc]", " 취소")]);
    }

    #[test]
    fn input_panel_shows_the_grapheme_counter() {
        let mut input = InputBuffer::new(200);
        input.insert_str("안녕");
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
        terminal
            .draw(|frame| {
                use super::render_input_panel;
                render_input_panel(frame, Rect::new(0, 0, 80, 3), &input);
            })
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
        }
        // The counter itself is ASCII, so double-width spacers cannot split it.
        assert!(out.contains("(2/200)"), "{out}");
    }

    #[test]
    fn help_overlay_lists_the_full_keymap() {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("terminal");
        terminal
            .draw(|frame| super::render_help_overlay(frame, frame.area()))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buffer.area.height {
            // Skip the spacer cell after double-width symbols so Hangul stays
            // contiguous.
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
        assert!(out.contains("키 안내"), "{out}");
        // Rows that used to be undocumented.
        assert!(out.contains("직접 입력"), "{out}");
        assert!(out.contains("이전 생성 기록"), "{out}");
        assert!(out.contains("도움말 오버레이"), "{out}");
    }

    #[test]
    fn fmt_duration_renders_compact_korean_labels() {
        assert_eq!(fmt_duration(0), "0초");
        assert_eq!(fmt_duration(47), "47초");
        assert_eq!(fmt_duration(60), "1분 0초");
        assert_eq!(fmt_duration(133), "2분 13초");
        assert_eq!(fmt_duration(3600), "1시간 0분");
        assert_eq!(fmt_duration(3900), "1시간 5분");
    }

    #[test]
    fn choice_preview_is_a_visible_second_line() {
        let lines = choice_lines(
            0,
            "시계가 멈춘 이유를 직접 묻는다",
            Some("회피하던 시간의 의미를 시험한다"),
            RiskLevel::Medium,
            80,
        );
        assert_eq!(lines.len(), 2);
        assert!(lines[0].to_string().contains("시계가 멈춘 이유"));
        assert!(lines[1].to_string().contains("↳"));
        assert!(lines[1].to_string().contains("시간의 의미를 시험한다"));
    }

    #[test]
    fn recognizes_explicit_npc_speaker_line() {
        assert_eq!(
            speaker_dialogue_line("유나: “정말 오랜만이네요.”"),
            Some(("유나", "“정말 오랜만이네요.”"))
        );
        assert_eq!(
            speaker_dialogue_line("유나： \"기다렸어요.\""),
            Some(("유나", "\"기다렸어요.\""))
        );
    }

    #[test]
    fn does_not_misclassify_regular_colon_prose_as_dialogue() {
        assert_eq!(speaker_dialogue_line("주소: https://example.test"), None);
        assert_eq!(
            speaker_dialogue_line("\"누구지?\" 하고 당신은 생각했다."),
            None
        );
        assert_eq!(
            speaker_dialogue_line("긴 설명 문장: \"따옴표가 있어도 화자명은 아니다\""),
            None
        );
    }
}
