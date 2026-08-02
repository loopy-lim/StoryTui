//! Initial home screen for choosing how to enter a StoryTUI session.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

/// Render the launcher menu. `selected` is an index into the four fixed rows.
pub fn render_home(frame: &mut Frame, area: Rect, selected: usize, can_continue: bool) {
    let rows = [
        ("새 게임", "새로운 이야기로 시작합니다.", true),
        ("이어하기", "자동 저장된 이야기를 이어갑니다.", can_continue),
        ("설정", "모델과 생성 설정을 확인합니다.", true),
        ("종료", "StoryTUI를 종료합니다.", true),
    ];
    let content = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(11),
        Constraint::Fill(1),
    ])
    .split(area)[1];
    let mut lines = vec![Line::from(Span::styled(
        "StoryTUI",
        Style::default()
            .fg(Color::LightCyan)
            .add_modifier(Modifier::BOLD),
    ))];
    for (index, (label, hint, enabled)) in rows.iter().enumerate() {
        let active = index == selected && *enabled;
        let prefix = if active { "▶ " } else { "  " };
        let style = if active {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
                .add_modifier(Modifier::REVERSED)
        } else if *enabled {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        lines.push(Line::from(Span::styled(format!("{prefix}{label}"), style)));
        lines.push(Line::from(Span::styled(
            format!("    {hint}"),
            Style::default().fg(Color::DarkGray),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "↑↓ 이동 · Enter 선택 · q 종료",
        Style::default().fg(Color::DarkGray),
    )));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::LightCyan))
        .title("시작 메뉴");
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .alignment(Alignment::Center),
        content,
    );
}

/// Resolve a left-click row inside the centered home menu to a menu item.
/// Both a label and its explanatory line are clickable.
pub fn menu_selection_at(area: Rect, row: u16) -> Option<usize> {
    let content = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(11),
        Constraint::Fill(1),
    ])
    .split(area)[1];
    let offset = row.checked_sub(content.y + 1)?;
    (offset < 8).then_some((offset / 2) as usize)
}
