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
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};
use story_model::{AppViewModel, RiskLevel, TimeSlot};
use unicode_width::UnicodeWidthStr;

/// Minimum supported terminal size for the Story screen.
pub const MIN_WIDTH: u16 = 60;
pub const MIN_HEIGHT: u16 = 16;

const CHOICE_REFRESH_SHORTCUT: &str = "[R] 선택지 다시 만들기  ";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListMode {
    History,
    Memory,
    ModelPicker,
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
}

pub fn render_list(frame: &mut Frame, area: Rect, mode: ListMode, items: &[String], scroll: u16) {
    let title = match mode {
        ListMode::History => "이전 생성 기록",
        ListMode::Memory => "기억 목록",
        ListMode::ModelPicker => "모델 선택",
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
                .border_style(Style::default().fg(Color::LightCyan))
                .title(Span::styled(
                    format!("{} · PgUp/PgDn 스크롤 · Esc 닫기", title),
                    Style::default().add_modifier(Modifier::BOLD),
                )),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(Clear, area);
    frame.render_widget(paragraph, area);
}

pub fn render_model_picker(frame: &mut Frame, area: Rect, items: &[String], selected: usize) {
    let rows: Vec<Line> = if items.is_empty() {
        vec![Line::from("모델 목록을 불러오지 못했습니다.")]
    } else {
        items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let active = index == selected.min(items.len().saturating_sub(1));
                Line::styled(
                    format!("{} {}", if active { "▶" } else { " " }, item),
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
                .title("모델 선택 · ↑↓ 이동 · Enter 기본모델 · D 기획(Director) · W 서사(Writer) · R 새로고침 · Esc 닫기"),
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
#[allow(clippy::too_many_arguments)] // rendering inputs stay safe at the UI boundary
pub fn render_story(
    frame: &mut Frame,
    area: Rect,
    view: &AppViewModel,
    selected: usize,
    narration_scroll: u16,
    input_mode: bool,
    input: &InputBuffer,
    story_stack: &[StoryStackEntry],
    streaming_narration: Option<&str>,
    generation_elapsed_secs: Option<u64>,
) -> u16 {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        render_min_size_warning(frame, area);
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
    let narration_text = if let Some(streaming) = streaming_narration {
        let mut entries = story_stack.to_vec();
        entries.push(StoryStackEntry {
            turn: view.status.turn.saturating_add(1),
            scene_id: "작성 중".to_string(),
            model: None,
            generation_secs: None,
            player_action: None,
            narration: streaming.to_string(),
        });
        story_stack_text(&entries)
    } else if story_stack.is_empty() {
        Text::from(markdown_lines(&view.narration))
    } else {
        story_stack_text(story_stack)
    };
    let needs_scrollbar =
        wrapped_line_count(&narration_text, chunks[0].width) > chunks[0].height as usize;
    let narration_chunks = if needs_scrollbar {
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(1)]).split(chunks[0])
    } else {
        [chunks[0], Rect::default()].into()
    };
    let narration_area = narration_chunks[0];
    let content_lines = wrapped_line_count(&narration_text, narration_area.width);
    let max_scroll = content_lines.saturating_sub(narration_area.height as usize) as u16;
    let scroll_position = narration_scroll.min(max_scroll);
    let narration = Paragraph::new(narration_text)
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(Color::Gray));
    // Mouse wheels and PageDown can generate an offset beyond the wrapped
    // content. Clamp it here so the pane never becomes mysteriously blank.
    let narration = narration.scroll((scroll_position, 0));
    frame.render_widget(narration, narration_area);
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
        render_status_panel(frame, chunks[3], view);
    } else {
        render_status_panel(frame, chunks[2], view);
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

/// Whether a click lands on the visible `[Home] 홈` shortcut in the status
/// panel. Keeping the hit test beside the renderer prevents the label from
/// becoming a keyboard-only affordance.
pub fn home_shortcut_at(
    area: Rect,
    view: &AppViewModel,
    input_mode: bool,
    column: u16,
    row: u16,
) -> bool {
    if area.width < MIN_WIDTH
        || area.height < MIN_HEIGHT
        || view.status.turn == 0
        || view.loading.is_some()
        || view.error.is_some()
    {
        return false;
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
    let shortcut = "[Home] 홈";
    let start = area
        .x
        .saturating_add(CHOICE_REFRESH_SHORTCUT.width() as u16);
    row == status_area.y.saturating_add(2)
        && column >= start
        && column < start.saturating_add(shortcut.width() as u16)
}

fn wrapped_line_count(text: &Text<'_>, width: u16) -> usize {
    let width = width.max(1) as usize;
    text.lines
        .iter()
        .map(|line| line.width().max(1).div_ceil(width))
        .sum()
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

fn story_stack_text(entries: &[StoryStackEntry]) -> Text<'static> {
    let mut lines = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        if index > 0 {
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
            meta.push(format!("모델 · {model}"));
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
    }
    Text::from(lines)
}

fn markdown_lines(input: &str) -> Vec<Line<'static>> {
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

/// Recognize only the safe dialogue contract used in generated narration.
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
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            title_text,
            Style::default().add_modifier(Modifier::BOLD),
        ));
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
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                        .add_modifier(Modifier::REVERSED);
                }
                lines.push(line);
            }
        }
        lines
    };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_input_panel(frame: &mut Frame, area: Rect, input: &InputBuffer) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::LightCyan))
        .title("직접 입력 · Enter 전송 · Esc 취소");
    let (before, after) = input.split_at_cursor();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(before),
            Span::styled("▌", Style::default().fg(Color::Yellow)),
            Span::raw(after),
        ]))
        .style(Style::default().fg(Color::White))
        .block(block),
        area,
    );
}

fn render_status_panel(frame: &mut Frame, area: Rect, view: &AppViewModel) {
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::DarkGray));
    frame.render_widget(Paragraph::new(status_lines(view)).block(block), area);
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

fn status_lines(view: &AppViewModel) -> Vec<Line<'static>> {
    let values = Line::from(status_values_prefix(view));
    if view.status.turn == 0 && view.choices.is_empty() && view.loading.is_none() {
        vec![
            values,
            Line::from(Span::styled(
                "[N] 게임 시작",
                Style::default()
                    .fg(Color::LightCyan)
                    .add_modifier(Modifier::BOLD),
            )),
        ]
    } else if view.loading.is_none() && view.error.is_none() {
        vec![
            values,
            Line::from(Span::styled(
                format!("{CHOICE_REFRESH_SHORTCUT}[Home] 홈  [O] 설정  [S] 저장  [L] 불러오기"),
                Style::default().fg(Color::DarkGray),
            )),
        ]
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
    let h = (wrapped_line_count(&body, inner_width) as u16 + 2)
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

fn render_min_size_warning(frame: &mut Frame, area: Rect) {
    let msg = format!(
        "터미널이 너무 작습니다 (최소 {MIN_WIDTH}x{MIN_HEIGHT}, 현재 {}x{})",
        area.width, area.height
    );
    let para = Paragraph::new(Line::from(msg))
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::Yellow));
    frame.render_widget(para, area);
}

/// Center a `w`×`h` rect inside `area`.
fn centered_rect(area: Rect, w: u16, h: u16) -> Rect {
    let pop = Layout::vertical([
        Constraint::Length((area.height.saturating_sub(h)) / 2),
        Constraint::Length(h),
        Constraint::Min(0),
    ])
    .split(area);
    let mid = pop[1];
    let horiz = Layout::horizontal([
        Constraint::Length((area.width.saturating_sub(w)) / 2),
        Constraint::Length(w),
        Constraint::Min(0),
    ])
    .split(mid);
    horiz[1]
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
    use super::{choice_lines, fmt_duration, speaker_dialogue_line};
    use story_model::RiskLevel;

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
