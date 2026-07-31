//! Story screen renderer (ST-021).
//!
//! Renders an [`AppViewModel`] into ratatui widgets inside a given `Rect`:
//!
//! ```text
//! ┌─ Title bar: story_title · chapter · location · time ─────┐
//! │  Narration (Korean-aware wrap)                            │
//! │                                                            │
//! │  ▸ [1] label                        (risk)                 │
//! │    [2] label                        (risk)                 │
//! │    [3] label                        (risk)                 │
//! │                                                            │
│ │  경로: —  애정:0  신뢰:0  긴장:0  턴:1                       │
//! └────────────────────────────────────────────────────────────┘
//! ```
//!
//! Overlays:
//! - [`AppViewModel::loading`] → a centered loading box.
//! - [`AppViewModel::error`] → a centered error box with a dismiss hint.
//! - area smaller than [`MIN_WIDTH`]×[`MIN_HEIGHT`] → a min-size warning.
//!
//! All Korean text is rendered via [`ratatui::text::Line`] / `Paragraph` with
//! `Wrap { trim: false }`; ratatui 0.30 measures display width through
//! `unicode-width`, so double-width Hangul aligns correctly.

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, List, ListState, Paragraph, Wrap};
use ratatui::Frame;
use story_model::{AppViewModel, RiskLevel, TimeSlot};

/// Minimum supported terminal size for the Story screen.
pub const MIN_WIDTH: u16 = 60;
pub const MIN_HEIGHT: u16 = 16;

/// Render the Story screen for `view`, highlighting `selected` (clamped to the
/// choice count). `selected` is an index into `view.choices`.
pub fn render_story(frame: &mut Frame, area: Rect, view: &AppViewModel, selected: usize) {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        render_min_size_warning(frame, area);
        return;
    }

    // Title bar.
    let title_line = Line::from(vec![
        Span::styled(
            view.story_title.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(" · "),
        Span::raw(view.chapter.clone()),
        Span::raw(" · "),
        Span::raw(view.location_display.clone()),
        Span::raw(" · "),
        Span::raw(time_slot_label(view.time_slot)),
    ]);
    let title_block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::DarkGray));
    let title_para = Paragraph::new(title_line)
        .block(title_block)
        .wrap(Wrap { trim: false });
    frame.render_widget(title_para, area);

    // Body below the title rule.
    let inner = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::DarkGray));
    let body = inner.inner(area);
    // Split body into narration / choices / status.
    let chunks = Layout::vertical([
        Constraint::Fill(3),
        Constraint::Length(view.choices.len().max(1) as u16 + 2),
        Constraint::Length(2),
    ])
    .split(body);

    // Narration.
    let narration_style = Style::default().fg(Color::Gray);
    let narration = Paragraph::new(Text::from(view.narration.clone()))
        .wrap(Wrap { trim: false })
        .style(narration_style);
    frame.render_widget(narration, chunks[0]);

    // Choices.
    render_choices(frame, chunks[1], view, selected);

    // Status line.
    render_status(frame, chunks[2], view);

    // Overlays on top.
    if let Some(ref err) = view.error {
        render_error_overlay(frame, area, err);
    } else if let Some(ref loading) = view.loading {
        render_loading_overlay(frame, area, &loading.message_ko);
    }
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

    let items: Vec<Line> = view
        .choices
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let marker = format!("{}. ", i + 1);
            let label = Span::raw(format!("{}{}", marker, c.label));
            let risk = Span::styled(
                format!("  ({})", risk_label(c.risk)),
                Style::default().fg(risk_color(c.risk)),
            );
            Line::from(vec![label, risk])
        })
        .collect();
    let list = List::new(items)
        .style(Style::default().fg(Color::White))
        .highlight_style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Yellow)
                .add_modifier(Modifier::REVERSED),
        )
        .highlight_symbol("▶ ");

    let mut state = ListState::default();
    let count = view.choices.len();
    if count > 0 {
        state.select(Some(selected.min(count - 1)));
    }
    frame.render_stateful_widget(list, inner, &mut state);
}

fn render_status(frame: &mut Frame, area: Rect, view: &AppViewModel) {
    let route = view
        .status
        .route
        .clone()
        .unwrap_or_else(|| "—".to_string());
    let stat = |label: &str, v: Option<i16>| match v {
        Some(n) => format!("{label}:{n}"),
        None => format!("{label}:—"),
    };
    let line = Line::from(vec![
        Span::styled(
            format!("경로: {route}"),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw("  "),
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
    ]);
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::DarkGray));
    let para = Paragraph::new(line).block(block);
    frame.render_widget(para, area);
}

fn render_loading_overlay(frame: &mut Frame, area: Rect, message: &str) {
    let h = 3u16;
    let w = (message.width() as u16 + 4).max(20);
    let centered = centered_rect(area, w, h);
    frame.render_widget(Clear, centered);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(Span::styled(
            "로딩",
            Style::default().add_modifier(Modifier::BOLD),
        ));
    let para = Paragraph::new(Line::from(message))
        .alignment(Alignment::Center)
        .block(block);
    frame.render_widget(para, centered);
}

fn render_error_overlay(frame: &mut Frame, area: Rect, err: &story_model::DomainErrorView) {
    let title = format!("오류: {}", err.title_ko);
    let body = Text::from(vec![
        Line::from(err.message_ko.clone()),
        Line::from(""),
        Line::from(Span::styled(
            "[Esc] 닫기".to_string(),
            Style::default().add_modifier(Modifier::DIM),
        )),
    ]);
    let w = (err.message_ko.width() as u16 + 4).max(title.width() as u16 + 4).min(area.width);
    let h = 5u16;
    let centered = centered_rect(area, w, h);
    frame.render_widget(Clear, centered);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red))
        .title(Span::styled(title, Style::default().add_modifier(Modifier::BOLD)));
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

// Pull `unicode_width::UnicodeWidthStr::width` into scope as a method.
use unicode_width::UnicodeWidthStr;
