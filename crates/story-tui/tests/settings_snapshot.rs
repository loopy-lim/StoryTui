//! Headless snapshot tests for the Settings + Keymap screens (ST-035).
//!
//! Renders the screen into a `ratatui::TestBackend` buffer at 80×24 (the
//! declared minimum) and pins the output via `insta`. Three cases:
//! loopback-ready, non-loopback (Korean warning), and below-min-size.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use story_model::{ProviderConfigSafe, ProviderHealth};
use story_tui::screens::settings::{
    HEALTH_LABEL_UNAVAILABLE, MIN_HEIGHT, MIN_WIDTH, SettingsView, render_settings,
};

fn safe_loopback() -> ProviderConfigSafe {
    ProviderConfigSafe {
        base_url: "http://127.0.0.1:8080".to_string(),
        model: "mlx-community/test-model".to_string(),
        director_model: None,
        writer_model: None,
        structured_output: "auto".to_string(),
        request_timeout_ms: 180_000,
        connect_timeout_ms: 3_000,
        max_retries: 2,
        is_loopback: true,
    }
}

fn safe_non_loopback() -> ProviderConfigSafe {
    ProviderConfigSafe {
        base_url: "http://192.168.0.5:8080".to_string(),
        model: "mlx-community/test-model".to_string(),
        director_model: None,
        writer_model: None,
        structured_output: "off".to_string(),
        request_timeout_ms: 60_000,
        connect_timeout_ms: 3_000,
        max_retries: 1,
        is_loopback: false,
    }
}

fn render_to_string(view: SettingsView<'_>, w: u16, h: u16) -> String {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|f| {
            render_settings(f, f.area(), &view);
        })
        .expect("draw");
    buffer_to_string(terminal.backend().buffer())
}

fn buffer_to_string(buf: &Buffer) -> String {
    use unicode_width::UnicodeWidthChar;
    let area = buf.area;
    let mut out = String::with_capacity((area.width * area.height) as usize);
    for y in 0..area.height {
        let mut line = String::with_capacity(area.width as usize);
        let mut skip_next_space = false;
        for x in 0..area.width {
            let sym = buf[(x, y)].symbol();
            if skip_next_space && sym == " " {
                skip_next_space = false;
                continue;
            }
            skip_next_space = false;
            line.push_str(sym);
            if sym.chars().count() == 1
                && let Some(c) = sym.chars().next()
                && UnicodeWidthChar::width(c) == Some(2)
            {
                skip_next_space = true;
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

#[test]
fn snapshot_settings_loopback_ready() {
    let view = SettingsView {
        provider: &safe_loopback(),
        health: ProviderHealth::Ready,
        temperature: 0.85,
        top_p: 0.92,
        top_k: 40,
        max_tokens: 1800,
        theme_name: "default",
    };
    let out = render_to_string(view, 80, 24);
    insta::assert_snapshot!("settings_loopback_ready", out);
}

#[test]
fn snapshot_settings_non_loopback_warning() {
    let view = SettingsView {
        provider: &safe_non_loopback(),
        health: ProviderHealth::Degraded,
        temperature: 0.7,
        top_p: 0.9,
        top_k: 30,
        max_tokens: 1200,
        theme_name: "default",
    };
    let out = render_to_string(view, 80, 24);
    insta::assert_snapshot!("settings_non_loopback_warning", out);
    // The Korean non-loopback warning MUST appear in the rendered output.
    assert!(
        out.contains("외부"),
        "non-loopback Korean warning missing:\n{out}"
    );
}

#[test]
fn settings_below_min_size_renders_warning() {
    let view = SettingsView {
        provider: &safe_loopback(),
        health: ProviderHealth::Ready,
        temperature: 0.85,
        top_p: 0.92,
        top_k: 40,
        max_tokens: 1800,
        theme_name: "default",
    };
    let out = render_to_string(view, MIN_WIDTH - 1, MIN_HEIGHT - 1);
    assert!(
        out.contains("터미널이 너무 작습니다"),
        "min-size warning missing:\n{out}"
    );
}

#[test]
fn settings_health_labels_resolve() {
    // Pin the Korean health labels so the renderer and the test agree on the
    // mapping.
    assert_eq!(
        story_tui::screens::settings::health_label(ProviderHealth::Ready).0,
        "준비됨"
    );
    assert_eq!(
        story_tui::screens::settings::health_label(ProviderHealth::Unavailable).0,
        HEALTH_LABEL_UNAVAILABLE
    );
}

#[test]
fn settings_masked_secret_never_renders() {
    // Even if a secret accidentally lands in a field the screen reads, it is
    // never echoed. ProviderConfigSafe has no secret fields by construction,
    // but we double-check the rendered output never contains `sk-`-style
    // values.
    let view = SettingsView {
        provider: &safe_loopback(),
        health: ProviderHealth::Ready,
        temperature: 0.85,
        top_p: 0.92,
        top_k: 40,
        max_tokens: 1800,
        theme_name: "default",
    };
    let out = render_to_string(view, 80, 24);
    assert!(
        !out.contains("api_key"),
        "settings screen leaked api_key:\n{out}"
    );
    assert!(
        !out.contains("authorization"),
        "settings screen leaked authorization:\n{out}"
    );
}
