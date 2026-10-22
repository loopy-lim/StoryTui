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

use crate::screens::keymap::keymap_lines_two_columns;
use crate::screens::model_display_label;
use crate::screens::popup::render_min_size_warning;
use crate::theme::{BORDER_TYPE, Palette, Styles};

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
        ProviderHealth::Ready => HealthLabel(HEALTH_LABEL_READY, Palette::SAFE),
        ProviderHealth::Loading => HealthLabel(HEALTH_LABEL_LOADING, Palette::CAUTION),
        ProviderHealth::Degraded => HealthLabel(HEALTH_LABEL_DEGRADED, Palette::CAUTION),
        ProviderHealth::Unavailable => HealthLabel(HEALTH_LABEL_UNAVAILABLE, Palette::DANGER),
        ProviderHealth::Misconfigured => HealthLabel(HEALTH_LABEL_MISCONFIGURED, Palette::DANGER),
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
    /// Whether narration TTS is currently on.
    pub tts_enabled: bool,
    /// Narration TTS backend name (`say` / `openai`).
    pub tts_backend: String,
    /// Configured TTS voice, when one is set.
    pub tts_voice: Option<String>,
    /// Whether scene images are currently shown.
    pub images_enabled: bool,
    /// Detected terminal graphics protocol (`kitty` 등). `None` = images
    /// unsupported by the terminal.
    pub image_backend: Option<String>,
    /// Whether the displayed protocol was player-forced (Settings `b` cycle)
    /// rather than auto-detected.
    pub image_backend_manual: bool,
    /// `true` = the two-stage Director → Writer path (streaming narration);
    /// `false` = the single-pass structured turn.
    pub use_director_writer: bool,
    /// Whether expression sprites are currently shown.
    pub expressions_enabled: bool,
    /// Theme name placeholder (no theme switcher yet).
    pub theme_name: &'a str,
    /// True when Settings was opened from the start menu, so `Esc` goes back
    /// to the menu; false means it was opened mid-game and `Esc` returns to
    /// the story.
    pub returns_home: bool,
}

/// Render the Settings screen.
pub fn render_settings(frame: &mut Frame, area: Rect, view: &SettingsView<'_>) {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        render_min_size_warning(frame, area, MIN_WIDTH, MIN_HEIGHT);
        return;
    }

    // Layout: title (2) | provider (6) | health + sampling side-by-side (9) |
    // keymap (auto) | footer (2). The health block grew by the `표정 (f)` row.
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(7),
        Constraint::Length(9),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .split(area);

    render_title(frame, chunks[0]);
    render_provider_block(frame, chunks[1], view);
    render_health_and_sampling(frame, chunks[2], view);
    render_keymap_block(frame, chunks[3]);
    render_footer(frame, chunks[4], view.returns_home);
}

fn render_title(frame: &mut Frame, area: Rect) {
    let line = Line::from(vec![Span::styled(" ✦ 설정 ✦ ", Styles::title())]);
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Styles::border_muted());
    let para = Paragraph::new(line)
        .block(block)
        .alignment(Alignment::Center);
    frame.render_widget(para, area);
}

fn render_provider_block(frame: &mut Frame, area: Rect, view: &SettingsView<'_>) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BORDER_TYPE)
        .border_style(Styles::border_muted())
        .title(Span::styled(" ✦ 공급자 ✦ ", Styles::title()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::with_capacity(5);
    lines.push(Line::from(vec![
        Span::styled("base_url   : ", Styles::subtle()),
        Span::styled(view.provider.base_url.clone(), Styles::item_normal()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("model      : ", Styles::subtle()),
        Span::styled(
            model_display_label(&view.provider.model),
            Styles::item_normal(),
        ),
    ]));
    if let Some(dm) = &view.provider.director_model {
        lines.push(Line::from(vec![
            Span::styled("director   : ", Style::default().fg(Palette::ACCENT)),
            Span::styled(model_display_label(dm), Styles::item_normal()),
        ]));
    }
    if let Some(cm) = &view.provider.choice_model {
        lines.push(Line::from(vec![
            Span::styled("choice     : ", Style::default().fg(Palette::PRIMARY)),
            Span::styled(model_display_label(cm), Styles::item_normal()),
        ]));
    }
    if let Some(wm) = &view.provider.writer_model {
        lines.push(Line::from(vec![
            Span::styled("writer     : ", Style::default().fg(Palette::SAFE)),
            Span::styled(model_display_label(wm), Styles::item_normal()),
        ]));
    }
    lines.push(Line::from(vec![
        Span::styled("structured : ", Styles::subtle()),
        Span::styled(
            view.provider.structured_output.clone(),
            Styles::item_normal(),
        ),
        Span::styled("  · retries ", Styles::muted()),
        Span::styled(view.provider.max_retries.to_string(), Styles::item_normal()),
        Span::styled("  · timeout ", Styles::muted()),
        Span::styled(
            view.provider.request_timeout_ms.to_string(),
            Styles::item_normal(),
        ),
        Span::styled("ms", Styles::muted()),
    ]));
    // Non-loopback warning.
    if !view.provider.is_loopback {
        lines.push(Line::from(vec![Span::styled(
            "⚠ 주의: base_url이 루프백이 아닙니다 — 외부로 전송될 수 있습니다.",
            Style::default()
                .fg(Palette::DANGER)
                .add_modifier(Modifier::BOLD),
        )]));
    } else {
        lines.push(Line::from(vec![Span::styled(
            "✓ 루프백(127.0.0.1) — 이 머신 안에서만 통신합니다.",
            Styles::muted(),
        )]));
    }

    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(para, inner);
}

fn render_health_and_sampling(frame: &mut Frame, area: Rect, view: &SettingsView<'_>) {
    // Balanced columns: the longest health row ("꺼짐 · halfblocks (수동)")
    // must fit unwrapped at the 80-col minimum, while sampling values are
    // short.
    let chunks =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(area);

    // Health block.
    let h_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BORDER_TYPE)
        .border_style(Styles::border_muted())
        .title(Span::styled(" ✦ 상태 ✦ ", Styles::title()));
    let h_inner = h_block.inner(chunks[0]);
    frame.render_widget(h_block, chunks[0]);
    let HealthLabel(label, color) = health_label(view.health);
    let health_lines = vec![
        Line::from(vec![
            Span::styled("provider   : ", Styles::subtle()),
            Span::styled(
                label,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("tts (v)    : ", Styles::subtle()),
            Span::styled(
                if view.tts_enabled { "켜짐" } else { "꺼짐" },
                Style::default()
                    .fg(if view.tts_enabled {
                        Palette::SAFE
                    } else {
                        Palette::MUTED
                    })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" · {}", view.tts_backend), Styles::muted()),
        ]),
        Line::from(vec![
            Span::styled("tts voice  : ", Styles::subtle()),
            Span::styled(
                view.tts_voice.as_deref().unwrap_or("자동(기본 보이스)"),
                Styles::item_normal(),
            ),
        ]),
        Line::from(vec![
            Span::styled("image (i)  : ", Styles::subtle()),
            Span::styled(
                match (view.images_enabled, view.image_backend.as_deref()) {
                    (true, Some(backend)) => {
                        format!(
                            "켜짐 · {backend} ({})",
                            image_mode_label(view.image_backend_manual)
                        )
                    }
                    (true, None) => "터미널 미지원".to_string(),
                    (false, Some(backend)) => {
                        format!(
                            "꺼짐 · {backend} ({})",
                            image_mode_label(view.image_backend_manual)
                        )
                    }
                    (false, None) => "꺼짐 (터미널 미지원)".to_string(),
                },
                Style::default()
                    .fg(match (view.images_enabled, view.image_backend.as_deref()) {
                        (true, Some(_)) => Palette::SAFE,
                        (true, None) => Palette::CAUTION,
                        (false, _) => Palette::MUTED,
                    })
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("gen (g)    : ", Styles::subtle()),
            Span::styled(
                if view.use_director_writer {
                    "2단계 (Director → Writer 스트리밍)"
                } else {
                    "단일 패스"
                },
                Style::default()
                    .fg(if view.use_director_writer {
                        Palette::SAFE
                    } else {
                        Palette::MUTED
                    })
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("표정 (f)   : ", Styles::subtle()),
            Span::styled(
                if view.expressions_enabled {
                    "켜짐"
                } else {
                    "꺼짐"
                },
                Style::default()
                    .fg(if view.expressions_enabled {
                        Palette::SAFE
                    } else {
                        Palette::MUTED
                    })
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(health_lines).wrap(Wrap { trim: false }),
        h_inner,
    );

    // Sampling block.
    let s_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BORDER_TYPE)
        .border_style(Styles::border_muted())
        .title(Span::styled(" ✦ 샘플링 ✦ ", Styles::title()));
    let s_inner = s_block.inner(chunks[1]);
    frame.render_widget(s_block, chunks[1]);
    let sampling_lines = vec![
        Line::from(vec![
            Span::styled("temperature : ", Styles::subtle()),
            Span::styled(format!("{:.2}", view.temperature), Styles::item_normal()),
        ]),
        Line::from(vec![
            Span::styled("top_p       : ", Styles::subtle()),
            Span::styled(format!("{:.2}", view.top_p), Styles::item_normal()),
        ]),
        Line::from(vec![
            Span::styled("top_k       : ", Styles::subtle()),
            Span::styled(view.top_k.to_string(), Styles::item_normal()),
        ]),
        Line::from(vec![
            Span::styled("max_tokens  : ", Styles::subtle()),
            Span::styled(view.max_tokens.to_string(), Styles::item_normal()),
        ]),
        Line::from(vec![
            Span::styled("theme       : ", Styles::subtle()),
            Span::styled(view.theme_name.to_string(), Styles::muted()),
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
        .border_type(BORDER_TYPE)
        .border_style(Styles::border_muted())
        .title(Span::styled(" ✦ 단축키 ✦ ", Styles::title()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    // Two balanced columns keep the full table visible on mid-size
    // terminals; the block's height cap already guards tiny ones.
    let (left, right) = keymap_lines_two_columns();
    let columns =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).split(inner);
    frame.render_widget(Paragraph::new(left).wrap(Wrap { trim: false }), columns[0]);
    frame.render_widget(Paragraph::new(right).wrap(Wrap { trim: false }), columns[1]);
}

fn render_footer(frame: &mut Frame, area: Rect, returns_home: bool) {
    let esc_hint = if returns_home {
        "시작 메뉴"
    } else {
        "게임으로"
    };
    let para = Paragraph::new(Line::from(vec![
        Span::styled("[P]", Styles::keycap()),
        Span::styled(" 모델  ", Styles::button_label()),
        Span::styled("[E]", Styles::keycap()),
        Span::styled(" 프로바이더  ", Styles::button_label()),
        Span::styled("[G]", Styles::keycap()),
        Span::styled(" 생성방식  ", Styles::button_label()),
        Span::styled("[F]", Styles::keycap()),
        Span::styled(" 표정  ", Styles::button_label()),
        Span::styled("[Esc]", Styles::keycap()),
        Span::styled(format!(" {esc_hint}  "), Styles::button_label()),
        Span::styled("[Q]", Styles::keycap()),
        Span::styled(" 종료", Styles::button_label()),
    ]))
    .alignment(Alignment::Center);
    frame.render_widget(para, area);
}

/// `자동`/`수동` marker for the image backend label — kept visible even while
/// images are off so the `b` protocol cycle always shows its effect.
fn image_mode_label(manual: bool) -> &'static str {
    if manual { "수동" } else { "자동" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn safe() -> ProviderConfigSafe {
        ProviderConfigSafe {
            base_url: "http://127.0.0.1:8080".to_string(),
            model: "m".to_string(),
            director_model: None,
            choice_model: None,
            writer_model: None,
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
        assert_eq!(ready.1, Palette::SAFE);
        assert_eq!(unav.1, Palette::DANGER);
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
            tts_enabled: true,
            tts_backend: "say".to_string(),
            tts_voice: None,
            images_enabled: true,
            image_backend: Some("kitty".to_string()),
            image_backend_manual: false,
            use_director_writer: false,
            expressions_enabled: true,
            theme_name: "default",
            returns_home: false,
        };
        // Touch every field to ensure renaming/removal is intentional.
        let _ = (
            v.provider.base_url.len(),
            v.health,
            v.temperature,
            v.top_p,
            v.top_k,
            v.max_tokens,
            v.tts_enabled,
            v.tts_backend,
            v.tts_voice,
            v.use_director_writer,
            v.theme_name,
            v.returns_home,
        );
    }
}
