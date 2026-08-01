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
    Block, Borders, Clear, HighlightSpacing, List, ListItem, ListState, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Wrap,
};
use story_model::{AppViewModel, RiskLevel, TimeSlot};
use unicode_width::UnicodeWidthStr;

/// Minimum supported terminal size for the Story screen.
pub const MIN_WIDTH: u16 = 60;
pub const MIN_HEIGHT: u16 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListMode {
    History,
    Memory,
}

/// One immutable block in the chronological story transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoryStackEntry {
    pub turn: u32,
    pub scene_id: String,
    pub player_action: Option<String>,
    pub narration: String,
}

pub fn render_list(frame: &mut Frame, area: Rect, mode: ListMode, items: &[String], scroll: u16) {
    let title = match mode {
        ListMode::History => "이전 생성 기록",
        ListMode::Memory => "기억 목록",
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

/// Render the Story screen for `view`, highlighting `selected` (clamped to the
/// choice count). `selected` is an index into `view.choices`.
pub fn render_story(
    frame: &mut Frame,
    area: Rect,
    view: &AppViewModel,
    selected: usize,
    narration_scroll: u16,
    input_mode: bool,
    input: &InputBuffer,
    story_stack: &[StoryStackEntry],
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

    // Reserve the title rows before laying out the story body. Rendering both
    // against `area` made the first stack heading overlap the global title.
    let body = frame_chunks[1];
    let choice_height = choice_area_height(view, body.width);
    let chunks = if input_mode {
        Layout::vertical([
            Constraint::Fill(3),
            Constraint::Length(choice_height),
            Constraint::Length(3),
            Constraint::Length(2),
        ])
        .split(body)
    } else {
        Layout::vertical([
            Constraint::Fill(3),
            Constraint::Length(choice_height),
            Constraint::Length(2),
        ])
        .split(body)
    };

    // Narration.
    let narration_text = if story_stack.is_empty() {
        markdown_text(&view.narration)
    } else {
        story_stack_text(story_stack)
    };
    // Reserve a right-edge scrollbar only when the story has more lines than
    // the viewport. The visible thumb then communicates both the current
    // position and the remaining scroll range without taking space otherwise.
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

    // While generation is pending, this panel replaces the choices in place.
    // It avoids a detached center overlay and makes it clear that selection is
    // temporarily locked rather than changing under the player's cursor.
    if let Some(loading) = &view.loading {
        render_choice_loading(frame, chunks[1], &loading.message_ko);
    } else {
        render_choices(frame, chunks[1], view, selected);
    }

    if input_mode {
        render_input(frame, chunks[2], true, input);
        render_status(frame, chunks[3], view);
    } else {
        render_status(frame, chunks[2], view);
    }

    // Overlays on top.
    if let Some(ref err) = view.error {
        render_error_overlay(frame, area, err);
    }
    max_scroll
}

fn wrapped_line_count(text: &Text<'_>, width: u16) -> usize {
    let width = width.max(1) as usize;
    text.lines
        .iter()
        .map(|line| line.width().max(1).div_ceil(width))
        .sum()
}

fn render_input(frame: &mut Frame, area: Rect, active: bool, input: &InputBuffer) {
    let title = if active {
        "직접 입력 · Enter 전송 · Esc 취소"
    } else {
        "직접 입력 [I]"
    };
    let text = if active {
        format!("{}▌", input.text())
    } else {
        "원하는 행동이나 대사를 직접 입력할 수 있습니다.".to_string()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if active {
            Color::LightCyan
        } else {
            Color::DarkGray
        }))
        .title(Span::styled(
            title,
            Style::default().add_modifier(Modifier::BOLD),
        ));
    frame.render_widget(
        Paragraph::new(text)
            .style(Style::default().fg(if active {
                Color::White
            } else {
                Color::DarkGray
            }))
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
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

fn render_choice_loading(frame: &mut Frame, area: Rect, message: &str) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(Span::styled(
            "선택지 · 생성 중",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    let body = Text::from(vec![
        Line::from(Span::styled(
            message.to_string(),
            Style::default().fg(Color::LightYellow),
        )),
        Line::from(Span::styled(
            "선택지는 생성이 끝나면 표시됩니다. [C] 취소",
            Style::default().fg(Color::DarkGray),
        )),
    ]);
    frame.render_widget(
        Paragraph::new(body)
            .block(block)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: false }),
        area,
    );
}

/// Render the small Markdown subset used by Story Packs at the TUI boundary.
fn markdown_text(input: &str) -> Text<'static> {
    Text::from(markdown_lines(input))
}

fn markdown_lines(input: &str) -> Vec<Line<'static>> {
    input
        .lines()
        .map(|raw| {
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

fn render_choices(frame: &mut Frame, area: Rect, view: &AppViewModel, selected: usize) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            "선택지",
            Style::default().add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // A List does not wrap its items itself. Build each choice as a multi-line
    // item before rendering so Korean labels stay readable and selection still
    // highlights the entire choice.
    let content_width = choice_content_width(inner.width);
    let items: Vec<ListItem> = view
        .choices
        .iter()
        .enumerate()
        .map(|(i, choice)| choice_item(i, choice.label.as_str(), choice.risk, content_width))
        .collect();
    let list = List::new(items)
        .style(Style::default().fg(Color::White))
        .highlight_style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Yellow)
                .add_modifier(Modifier::REVERSED),
        )
        .highlight_symbol("▶ ")
        .highlight_spacing(HighlightSpacing::Always)
        .repeat_highlight_symbol(true);

    let mut state = ListState::default();
    let count = view.choices.len();
    if count > 0 {
        state.select(Some(selected.min(count - 1)));
    }
    frame.render_stateful_widget(list, inner, &mut state);
}

fn choice_area_height(view: &AppViewModel, outer_width: u16) -> u16 {
    let inner_width = outer_width.saturating_sub(2);
    let content_width = choice_content_width(inner_width);
    let rows = view
        .choices
        .iter()
        .enumerate()
        .map(|(index, choice)| {
            choice_line_count(index, choice.label.as_str(), choice.risk, content_width)
        })
        .sum::<usize>()
        .max(1);
    (rows as u16).saturating_add(2)
}

fn choice_content_width(inner_width: u16) -> usize {
    inner_width.saturating_sub("▶ ".width() as u16).max(1) as usize
}

fn choice_line_count(index: usize, label: &str, risk: RiskLevel, width: usize) -> usize {
    choice_lines(index, label, risk, width).len()
}

fn choice_item(index: usize, label: &str, risk: RiskLevel, width: usize) -> ListItem<'static> {
    ListItem::new(Text::from(choice_lines(index, label, risk, width)))
}

fn choice_lines(index: usize, label: &str, risk: RiskLevel, width: usize) -> Vec<Line<'static>> {
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

fn render_status(frame: &mut Frame, area: Rect, view: &AppViewModel) {
    let stat = |label: &str, v: Option<i16>| match v {
        Some(n) => format!("{label}:{n}"),
        None => format!("{label}:—"),
    };
    let mut spans = vec![
        Span::raw(stat("애정", view.status.affection)),
        Span::raw("  "),
        Span::raw(stat("신뢰", view.status.trust)),
        Span::raw("  "),
        Span::raw(stat("긴장", view.status.tension)),
        Span::raw("  "),
        Span::styled(
            format!("턴: {}", view.status.turn),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ];
    if view.status.turn == 0 && view.choices.is_empty() && view.loading.is_none() {
        spans.extend([
            Span::raw("    "),
            Span::styled(
                "[N] 게임 시작",
                Style::default()
                    .fg(Color::LightCyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
    } else if view.loading.is_none() && view.error.is_none() {
        spans.extend([
            Span::raw("    "),
            Span::styled(
                "[S] 저장  [L] 불러오기",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
    }
    let line = Line::from(spans);
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::DarkGray));
    let para = Paragraph::new(line).block(block);
    frame.render_widget(para, area);
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
    let h = 5u16;
    let centered = centered_rect(area, w, h);
    frame.render_widget(Clear, centered);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red))
        .title(Span::styled(
            title,
            Style::default().add_modifier(Modifier::BOLD),
        ));
    let para = Paragraph::new(body).block(block);
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
