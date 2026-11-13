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
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use story_model::{LorebookEntry, ProviderConfigSafe, ProviderHealth};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::screens::keymap::keymap_lines_two_columns;
use crate::screens::model_display_label;
use crate::screens::popup::{POPUP_WIDTH_WIDE, centered_rect, render_min_size_warning};
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
    /// Single-pass SSE streaming render (Settings `s` toggle). Shown dimmed
    /// with a "2단계 중" note while `use_director_writer` is on, because the
    /// two-stage Writer always streams; the note carries the stored
    /// preference so flipping `s` there is never invisible.
    pub stream_turns: bool,
    /// Whether the active provider advertises SSE streaming. A `켜짐`
    /// preference on a provider without it renders as
    /// "켜짐 · SSE 미지원" — the effective generation stays non-streaming.
    pub stream_supported: bool,
    /// Whether expression sprites are currently shown.
    pub expressions_enabled: bool,
    /// Response-style preset label (`자동` when the profile's own tuning is
    /// in charge); shown at the top of the sampling block.
    pub sampling_style: &'a str,
    /// Persona row label (`없음` / the player's name / `설정됨`).
    pub persona_label: &'a str,
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

    // Layout: title (2) | provider (7) | health + sampling side-by-side
    // (10) | keymap (auto) | footer (2). The health block grew by the
    // `표정 (f)` and `스트림 (s)` rows (8 content lines + borders); at the
    // 24-row floor the keymap teaser gives up one row — the full keymap
    // stays in the `?` overlay, and taller terminals grow `Min(3)` back.
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(7),
        Constraint::Length(10),
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
    // Non-loopback warning / offline built-in notice.
    if view.provider.base_url.starts_with("builtin://") {
        lines.push(Line::from(vec![Span::styled(
            "✓ 오프라인 내장 엔진 — 외부 서버 없이 동작합니다.",
            Styles::muted(),
        )]));
    } else if !view.provider.is_loopback {
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
                    "2단계 스트리밍"
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
            Span::styled("스트림 (s) : ", Styles::subtle()),
            Span::styled(
                if view.use_director_writer {
                    // The Writer always streams on this path; the note still
                    // shows the stored single-pass preference so a `s` press
                    // here is visible, not a silent flip.
                    if view.stream_turns {
                        "2단계 중 (켜짐)"
                    } else {
                        "2단계 중 (꺼짐)"
                    }
                } else if view.stream_turns {
                    if view.stream_supported {
                        "켜짐"
                    } else {
                        "켜짐 · SSE 미지원"
                    }
                } else {
                    "꺼짐"
                },
                Style::default()
                    .fg(if view.use_director_writer || !view.stream_turns {
                        // Two-stage note and plain off both read as quiet.
                        Palette::MUTED
                    } else if view.stream_supported {
                        Palette::SAFE
                    } else {
                        // On, but the provider cannot stream: the preference
                        // stands while the effective turn stays
                        // non-streaming — worth a caution, not a success.
                        Palette::CAUTION
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
        Line::from(vec![
            Span::styled("persona (a): ", Styles::subtle()),
            Span::styled(view.persona_label, Styles::item_normal()),
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
            Span::styled("style (x)   : ", Styles::subtle()),
            Span::styled(
                view.sampling_style,
                if view.sampling_style == "자동" {
                    Styles::muted()
                } else {
                    Style::default()
                        .fg(Palette::SAFE)
                        .add_modifier(Modifier::BOLD)
                },
            ),
        ]),
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
        // Compound spelling (시작메뉴, 8 cols) keeps this variant at the
        // same width as 게임으로 — the spaced form would clip [Q] 종료's
        // last column on an 80-col terminal.
        "시작메뉴"
    } else {
        "게임으로"
    };
    // Single-space separators: with the [S] 스트림 entry the row is exactly
    // 80 cols — double spaces would clip the trailing [Q] 종료 on an
    // 80-col terminal (the snapshot test pins this).
    let para = Paragraph::new(Line::from(vec![
        Span::styled("[P]", Styles::keycap()),
        Span::styled(" 모델 ", Styles::button_label()),
        Span::styled("[E]", Styles::keycap()),
        Span::styled(" 프로바이더 ", Styles::button_label()),
        Span::styled("[G]", Styles::keycap()),
        Span::styled(" 생성방식 ", Styles::button_label()),
        Span::styled("[S]", Styles::keycap()),
        Span::styled(" 스트림 ", Styles::button_label()),
        Span::styled("[F]", Styles::keycap()),
        Span::styled(" 표정 ", Styles::button_label()),
        Span::styled("[Esc]", Styles::keycap()),
        Span::styled(format!(" {esc_hint} "), Styles::button_label()),
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

/// The persona editor modal opened from the Settings screen (`a`). `focus`
/// selects which field owns the cursor cell (0 = 이름, 1 = 페르소나); each
/// field arrives as `(before, after)` around the cursor.
pub fn render_persona_dialog(
    frame: &mut Frame,
    area: Rect,
    name: (&str, &str),
    persona: (&str, &str),
    focus: usize,
) {
    let mut body = vec![
        Line::from(Span::styled(
            "이름과 페르소나를 편집하세요 — 비우면 그 항목이 꺼집니다.",
            Styles::item_normal(),
        )),
        Line::from(""),
    ];
    let fields = [("이름      ", name), ("페르소나  ", persona)];
    for (index, (label, (before, after))) in fields.iter().enumerate() {
        let focused = index == focus;
        let label_style = if focused {
            Style::default()
                .fg(Palette::ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Styles::subtle()
        };
        let mut line = vec![Span::styled((*label).to_string(), label_style)];
        line.push(Span::styled((*before).to_string(), Styles::item_normal()));
        if focused {
            line.push(Span::styled("▌", Style::default().fg(Palette::SELECTED)));
        }
        line.push(Span::styled((*after).to_string(), Styles::item_normal()));
        body.push(Line::from(line));
    }
    body.push(Line::from(""));
    body.push(Line::from(Span::styled(
        "Tab 항목 이동 · Enter 적용 · Esc 취소",
        Styles::muted(),
    )));

    let height = (body.len() as u16 + 2).clamp(7, area.height.max(7));
    let popup = centered_rect(area, POPUP_WIDTH_WIDE, height);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BORDER_TYPE)
        .border_style(Styles::border_primary())
        .title(Span::styled(" ✦ 페르소나 ✦ ", Styles::title()));
    frame.render_widget(
        Paragraph::new(body).block(block).wrap(Wrap { trim: false }),
        popup,
    );
}

/// The author's note editor opened from the Settings screen (`t`). One
/// field; the submitted text goes into every turn's prompt verbatim
/// (trimmed by the app), an empty submit clears the note.
pub fn render_author_note_dialog(frame: &mut Frame, area: Rect, note: (&str, &str)) {
    let mut line = vec![Span::styled(
        "노트      ".to_string(),
        Style::default()
            .fg(Palette::ACCENT)
            .add_modifier(Modifier::BOLD),
    )];
    line.push(Span::styled(note.0.to_string(), Styles::item_normal()));
    line.push(Span::styled("▌", Style::default().fg(Palette::SELECTED)));
    line.push(Span::styled(note.1.to_string(), Styles::item_normal()));
    let body = vec![
        Line::from(line),
        Line::from(""),
        Line::from(Span::styled(
            "Enter 적용 · Esc 취소 — 매 턴 프롬프트에 그대로 들어갑니다",
            Styles::muted(),
        )),
    ];

    let height = (body.len() as u16 + 2).clamp(7, area.height.max(7));
    let popup = centered_rect(area, POPUP_WIDTH_WIDE, height);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BORDER_TYPE)
        .border_style(Styles::border_primary())
        .title(Span::styled(" ✦ 작가 노트 ✦ ", Styles::title()));
    frame.render_widget(
        Paragraph::new(body).block(block).wrap(Wrap { trim: false }),
        popup,
    );
}

/// The lorebook editor opened from the Settings screen (`w`). Lists the
/// working copy one entry per row (`✔/✘ 키|키 상수 · 내용 첫 줄`); edits go
/// through a single-line sub-editor rendered below the list. `persisted ==
/// false` shows the session-only notice — the pack has no lorebook.toml to
/// save into. `edit` is `Some((필드 라벨, 커서 앞, 커서 뒤))` while an edit
/// is open.
pub fn render_lorebook_dialog(
    frame: &mut Frame,
    area: Rect,
    entries: &[LorebookEntry],
    selected: usize,
    persisted: bool,
    edit: Option<(&'static str, &str, &str)>,
) {
    let mut header: Vec<Line> = Vec::new();
    if !persisted {
        header.push(Line::from(Span::styled(
            "저장 파일 없음 — 세션 전용",
            Style::default().fg(Palette::CAUTION),
        )));
    }
    if entries.is_empty() {
        header.push(Line::from(Span::styled("엔트리 없음", Styles::muted())));
    }
    let mut footer: Vec<Line> = Vec::new();
    let mut edit_line: Option<Line> = None;
    if let Some((label, before, after)) = edit {
        let mut line = vec![Span::styled(
            format!("{label}: "),
            Style::default()
                .fg(Palette::ACCENT)
                .add_modifier(Modifier::BOLD),
        )];
        line.push(Span::styled(before.to_string(), Styles::item_normal()));
        line.push(Span::styled("▌", Style::default().fg(Palette::SELECTED)));
        line.push(Span::styled(after.to_string(), Styles::item_normal()));
        edit_line = Some(Line::from(line));
        // Only the keys editor splits on commas — the escape hint belongs
        // there and only there.
        let edit_hint = if label == "키" {
            "Enter 적용 · \\, 쉼표 · Esc 목록으로"
        } else {
            "Enter 적용 · Esc 목록으로"
        };
        footer.push(Line::from(Span::styled(edit_hint, Styles::muted())));
    } else {
        footer.push(Line::from(Span::styled(
            "↑↓ 이동 · Space 활성 · Enter 내용 · k 키 · c 상수 · p 확률 · Esc 닫기",
            Styles::muted(),
        )));
    }

    let fixed = header.len() as u16 + footer.len() as u16 + edit_line.is_some() as u16;
    let list_rows = entries.len() as u16;
    let height = (fixed + list_rows + 2).clamp(7, area.height.max(7));
    let popup = centered_rect(area, POPUP_WIDTH_WIDE, height);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BORDER_TYPE)
        .border_style(Styles::border_primary())
        .title(Span::styled(" ✦ 로어북 ✦ ", Styles::title()));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let mut constraints = vec![Constraint::Length(header.len() as u16), Constraint::Min(0)];
    if edit_line.is_some() {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(footer.len() as u16));
    let chunks = Layout::vertical(constraints).split(inner);
    let footer_index = chunks.len() - 1;
    if !header.is_empty() {
        frame.render_widget(Paragraph::new(header), chunks[0]);
    }
    // Sliding window so the selection stays visible in a short popup.
    let visible = chunks[1].height as usize;
    let first = selected.saturating_sub(visible.saturating_sub(1));
    let rows: Vec<Line> = entries
        .iter()
        .enumerate()
        .skip(first)
        .take(visible.max(1))
        .map(|(index, entry)| {
            let text = lorebook_row_text(entry, chunks[1].width as usize);
            if index == selected {
                Line::from(Span::styled(
                    text,
                    Style::default()
                        .fg(Palette::ACCENT)
                        .add_modifier(Modifier::BOLD),
                ))
            } else {
                Line::from(Span::styled(text, Styles::item_normal()))
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(rows), chunks[1]);
    if let Some(line) = edit_line {
        frame.render_widget(Paragraph::new(line), chunks[2]);
    }
    frame.render_widget(Paragraph::new(footer), chunks[footer_index]);
}

/// One lorebook list row: `✔ 키1|키2 상수 75% · 내용 첫 줄`. The 상수 tag
/// shows a pinned entry, the `n%` hint only appears while the activation
/// probability is off the 100 default. The keys column is width-budgeted so
/// a long key list still leaves room for the content head.
fn lorebook_row_text(entry: &LorebookEntry, max_width: usize) -> String {
    let marker = if entry.enabled { "✔" } else { "✘" };
    let keys = if entry.keys.is_empty() {
        "키 없음".to_string()
    } else {
        entry.keys.join("|")
    };
    // Marker + separator + possible 상수/확률 tags + spacing slack.
    let keys_budget = max_width.saturating_sub(10).max(6);
    let keys = truncate_width(&keys, keys_budget);
    let mut prefix = format!("{marker} {keys}");
    if entry.constant {
        prefix.push_str(" 상수");
    }
    if entry.probability < 100 {
        prefix.push_str(&format!(" {}%", entry.probability));
    }
    prefix.push_str(" · ");
    let content_head = entry.content.lines().next().unwrap_or("");
    let budget = max_width.saturating_sub(prefix.width()).max(1);
    format!("{prefix}{}", truncate_width(content_head, budget))
}

/// Width-aware truncation with a `…` tail so a row never wraps inside the
/// popup at the 80-column minimum.
fn truncate_width(text: &str, max_width: usize) -> String {
    if text.width() <= max_width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for grapheme in text.graphemes(true) {
        let width = grapheme.width();
        if used + width > max_width.saturating_sub(1) {
            break;
        }
        out.push_str(grapheme);
        used += width;
    }
    out.push('…');
    out
}

/// Outer `(width, height)` the prompt viewer's popup draws with — shared by
/// the renderer and the app's scroll clamp so both agree on the wrapped rows
/// (the clamp subtracts the 2 border lines itself).
pub(crate) fn prompt_viewer_frame(area: Rect) -> (u16, u16) {
    let height = 20.min(area.height.max(7));
    (POPUP_WIDTH_WIDE.min(area.width), height)
}

/// The last-sent prompt viewer opened from the Settings screen (`r`). The
/// host captured these strings right before the provider call; read-only,
/// session memory only.
pub fn render_prompt_viewer(frame: &mut Frame, area: Rect, system: &str, user: &str, scroll: u16) {
    let body = prompt_viewer_body_lines(system, user);
    let (width, height) = prompt_viewer_frame(area);
    let popup = centered_rect(area, width, height);
    frame.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BORDER_TYPE)
        .border_style(Styles::border_primary())
        .title(Span::styled(" ✦ 마지막 프롬프트 ✦ ", Styles::title()));
    frame.render_widget(
        Paragraph::new(body)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        popup,
    );
}

/// The viewer body: labeled System / User sections. Shared with the app's
/// scroll clamp so key handling and rendering agree on the line count.
pub(crate) fn prompt_viewer_body_lines(system: &str, user: &str) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled("System", Styles::subtle()))];
    lines.extend(system.lines().map(|text| Line::from(text.to_string())));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("User", Styles::subtle())));
    lines.extend(user.lines().map(|text| Line::from(text.to_string())));
    lines
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
            stream_turns: true,
            stream_supported: true,
            expressions_enabled: true,
            sampling_style: "자동",
            persona_label: "없음",
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
            v.stream_turns,
            v.stream_supported,
            v.theme_name,
            v.returns_home,
        );
    }
}
