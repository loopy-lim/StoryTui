//! Settings screen renderer (ST-035, Technical Spec §14.2).
//!
//! Renders a [`SettingsView`] — a small, plain-data bundle assembled by the
//! host from a [`ProviderConfigSafe`] (NO secret fields by construction), the
//! most-recently-observed [`ProviderHealth`], sampling parameters, and the
//! theme name. Secrets are never displayed: `ProviderConfigSafe` carries none,
//! and the renderer has no field for an API key. A non-loopback `base_url`
//! shows a Korean warning so the player notices when their provider is
//! reachable from outside the host (ADR-002).
//!
//! The minimum supported size is [`MIN_WIDTH`] × [`MIN_HEIGHT`] = 80 × 24;
//! below that the screen renders a single Korean "terminal too small" warning.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use story_model::{ProviderConfigSafe, ProviderHealth};

use crate::screens::keymap::keymap_lines;

/// Minimum supported terminal size for the Settings screen.
pub const MIN_WIDTH: u16 = 80;
pub const MIN_HEIGHT: u16 = 24;

/// Korean health labels surfaced to the player.
pub const HEALTH_LABEL_READY: &str = "준비됨";
pub const HEALTH_LABEL_LOADING: &str = "로딩 중";
pub const HEALTH_LABEL_DEGRADED: &str = "성능 저하";
pub const HEALTH_LABEL_UNAVAILABLE: &str = "사용 불가";
pub const HEALTH_LABEL_MISCONFIGURED: &str = "설정 오류";

/// A Korean health label plus the color it should render in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HealthLabel(pub &'static str, pub Color);

/// Map a [`ProviderHealth`] to its Korean label + color (FR-037).
pub fn health_label(health: ProviderHealth) -> HealthLabel {
    match health {
        ProviderHealth::Ready => HealthLabel(HEALTH_LABEL_READY, Color::Green),
        ProviderHealth::Loading => HealthLabel(HEALTH_LABEL_LOADING, Color::Yellow),
        ProviderHealth::Degraded => HealthLabel(HEALTH_LABEL_DEGRADED, Color::Yellow),
        ProviderHealth::Unavailable => HealthLabel(HEALTH_LABEL_UNAVAILABLE, Color::Red),
        ProviderHealth::Misconfigured => HealthLabel(HEALTH_LABEL_MISCONFIGURED, Color::Magenta),
    }
}

/// The plain-data view the Settings screen renders. The host assembles this
/// from `EngineState` + `ProviderConfigSafe` + the latest health probe.
#[derive(Clone, Debug)]
pub struct SettingsView<'a> {
    /// Provider config with secrets already stripped.
    pub provider: &'a ProviderConfigSafe,
    /// Most-recently-observed provider health.
    pub health: ProviderHealth,
    /// Sampling temperature.
    pub temperature: f32,
    /// Nucleus sampling probability mass.
    pub top_p: f32,
    /// Top-k sampling cutoff.
    pub top_k: u32,
    /// Maximum output tokens per turn.
    pub max_tokens: u32,
    /// Theme name placeholder (no theme switcher yet).
    pub theme_name: &'a str,
}

/// Render the Settings screen.
pub fn render_settings(frame: &mut Frame, area: Rect, view: &SettingsView<'_>) {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        render_min_size_warning(frame, area);
        return;
    }

    // Layout: title (2) | provider (6) | health + sampling side-by-side (8) |
    // keymap (auto) | footer (2).
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(7),
        Constraint::Length(8),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .split(area);

    render_title(frame, chunks[0]);
    render_provider_block(frame, chunks[1], view);
    render_health_and_sampling(frame, chunks[2], view);
    render_keymap_block(frame, chunks[3]);
    render_footer(frame, chunks[4]);
}

fn render_title(frame: &mut Frame, area: Rect) {
    let line = Line::from(vec![Span::styled(
        "설정 · Settings",
        Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(Color::Cyan),
    )]);
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(Color::DarkGray));
    let para = Paragraph::new(line)
        .block(block)
        .alignment(Alignment::Center);
    frame.render_widget(para, area);
}

fn render_provider_block(frame: &mut Frame, area: Rect, view: &SettingsView<'_>) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            "공급자 · Provider",
            Style::default().add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::with_capacity(5);
    lines.push(Line::from(vec![
        Span::raw("base_url   : "),
        Span::raw(view.provider.base_url.clone()),
    ]));
    lines.push(Line::from(vec![
        Span::raw("model      : "),
        Span::raw(if view.provider.model.is_empty() {
            "(미지정)".to_string()
        } else {
            view.provider.model.clone()
        }),
    ]));
    lines.push(Line::from(vec![
        Span::raw("structured : "),
        Span::raw(view.provider.structured_output.clone()),
        Span::raw("  · retries "),
        Span::raw(view.provider.max_retries.to_string()),
        Span::raw("  · timeout "),
        Span::raw(view.provider.request_timeout_ms.to_string()),
        Span::raw("ms"),
    ]));
    // Non-loopback warning.
    if !view.provider.is_loopback {
        lines.push(Line::from(vec![Span::styled(
            "⚠ 주의: base_url이 루프백이 아닙니다 — 외부로 전송될 수 있습니다.",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )]));
    } else {
        lines.push(Line::from(vec![Span::styled(
            "✓ 루프백(127.0.0.1) — 이 머신 안에서만 통신합니다.",
            Style::default().fg(Color::DarkGray),
        )]));
    }

    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(para, inner);
}

fn render_health_and_sampling(frame: &mut Frame, area: Rect, view: &SettingsView<'_>) {
    let chunks =
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).split(area);

    // Health block.
    let h_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            "상태 · Health",
            Style::default().add_modifier(Modifier::BOLD),
        ));
    let h_inner = h_block.inner(chunks[0]);
    frame.render_widget(h_block, chunks[0]);
    let HealthLabel(label, color) = health_label(view.health);
    let health_lines = vec![Line::from(vec![
        Span::raw("provider   : "),
        Span::styled(
            label,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
    ])];
    frame.render_widget(
        Paragraph::new(health_lines).wrap(Wrap { trim: false }),
        h_inner,
    );

    // Sampling block.
    let s_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            "샘플링 · Sampling",
            Style::default().add_modifier(Modifier::BOLD),
        ));
    let s_inner = s_block.inner(chunks[1]);
    frame.render_widget(s_block, chunks[1]);
    let sampling_lines = vec![
        Line::from(vec![
            Span::raw("temperature : "),
            Span::raw(format!("{:.2}", view.temperature)),
        ]),
        Line::from(vec![
            Span::raw("top_p       : "),
            Span::raw(format!("{:.2}", view.top_p)),
        ]),
        Line::from(vec![
            Span::raw("top_k       : "),
            Span::raw(view.top_k.to_string()),
        ]),
        Line::from(vec![
            Span::raw("max_tokens  : "),
            Span::raw(view.max_tokens.to_string()),
        ]),
        Line::from(vec![
            Span::raw("theme       : "),
            Span::styled(
                view.theme_name.to_string(),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(sampling_lines).wrap(Wrap { trim: false }),
        s_inner,
    );
}

fn render_keymap_block(frame: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            "단축키 · Keymap",
            Style::default().add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    // Cap the visible rows at the available height so a too-short keymap block
    // never overflows.
    let lines = keymap_lines();
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_footer(frame: &mut Frame, area: Rect) {
    let para = Paragraph::new(Line::from(Span::styled(
        "Esc 닫기 · q 종료",
        Style::default().fg(Color::DarkGray),
    )))
    .alignment(Alignment::Center);
    frame.render_widget(para, area);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn safe() -> ProviderConfigSafe {
        ProviderConfigSafe {
            base_url: "http://127.0.0.1:8080".to_string(),
            model: "m".to_string(),
            structured_output: "auto".to_string(),
            request_timeout_ms: 180_000,
            connect_timeout_ms: 3_000,
            max_retries: 2,
            is_loopback: true,
        }
    }

    #[test]
    fn health_label_colors_are_distinct() {
        let ready = health_label(ProviderHealth::Ready);
        let unav = health_label(ProviderHealth::Unavailable);
        assert_eq!(ready.0, HEALTH_LABEL_READY);
        assert_eq!(ready.1, Color::Green);
        assert_eq!(unav.1, Color::Red);
    }

    #[test]
    fn settings_view_holds_no_secret_field() {
        // Compile-time guarantee: there is no field for an API key.
        let v = SettingsView {
            provider: &safe(),
            health: ProviderHealth::Ready,
            temperature: 0.85,
            top_p: 0.92,
            top_k: 40,
            max_tokens: 1800,
            theme_name: "default",
        };
        // Touch every field to ensure renaming/removal is intentional.
        let _ = (
            v.provider.base_url.len(),
            v.health,
            v.temperature,
            v.top_p,
            v.top_k,
            v.max_tokens,
            v.theme_name,
        );
    }
}
