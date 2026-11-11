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
        choice_model: None,
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
        choice_model: None,
        writer_model: None,
        structured_output: "off".to_string(),
        request_timeout_ms: 60_000,
        connect_timeout_ms: 3_000,
        max_retries: 1,
        is_loopback: false,
    }
}

fn render_to_string(view: &SettingsView<'_>, w: u16, h: u16) -> String {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|f| {
            render_settings(f, f.area(), view);
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
    let mut view = SettingsView {
        provider: &safe_loopback(),
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
        use_director_writer: true,
        stream_turns: true,
        stream_supported: true,
        expressions_enabled: true,
        sampling_style: "자동",
        persona_label: "없음",
        theme_name: "default",
        returns_home: false,
    };
    let out = render_to_string(&view, 80, 24);
    insta::assert_snapshot!("settings_loopback_ready", out);
    // The footer is exactly 80 cols with its [S] 스트림 entry — the trailing
    // quit hint must never clip on an 80-col terminal.
    assert!(
        out.contains("[S] 스트림"),
        "[S] footer entry missing:\n{out}"
    );
    assert!(
        out.contains("[Q] 종료"),
        "footer clips the quit hint:\n{out}"
    );

    // The home-menu entry variant (`returns_home`, Esc hint 시작메뉴) must
    // fit the same 80 cols — the spaced spelling pushed it to 81 and clipped
    // [Q] 종료's last column (3rd-party review P3).
    view.returns_home = true;
    let home_out = render_to_string(&view, 80, 24);
    assert!(
        home_out.contains("[Esc] 시작메뉴"),
        "home-entry Esc hint missing:\n{home_out}"
    );
    assert!(
        home_out.contains("[Q] 종료"),
        "home-entry footer clips the quit hint:\n{home_out}"
    );
}

#[test]
fn snapshot_settings_image_backend_manual() {
    // Images OFF + a player-forced protocol: the auto/manual marker must
    // stay visible so the `b` cycle shows its effect without enabling
    // images.
    let view = SettingsView {
        provider: &safe_loopback(),
        health: ProviderHealth::Ready,
        temperature: 0.85,
        top_p: 0.92,
        top_k: 40,
        max_tokens: 1800,
        tts_enabled: true,
        tts_backend: "say".to_string(),
        tts_voice: None,
        images_enabled: false,
        image_backend: Some("halfblocks".to_string()),
        image_backend_manual: true,
        use_director_writer: false,
        stream_turns: false,
        stream_supported: true,
        expressions_enabled: false,
        sampling_style: "자동",
        persona_label: "없음",
        theme_name: "default",
        returns_home: false,
    };
    let out = render_to_string(&view, 80, 24);
    insta::assert_snapshot!("settings_image_backend_manual", out);
    assert!(
        out.contains("꺼짐 · halfblocks (수동)"),
        "manual halfblocks label missing:\n{out}"
    );
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
        tts_enabled: false,
        tts_backend: "openai".to_string(),
        tts_voice: Some("shimmer".to_string()),
        images_enabled: false,
        image_backend: None,
        image_backend_manual: false,
        use_director_writer: false,
        stream_turns: false,
        stream_supported: true,
        expressions_enabled: false,
        sampling_style: "자동",
        persona_label: "없음",
        theme_name: "default",
        returns_home: false,
    };
    let out = render_to_string(&view, 80, 24);
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
        tts_enabled: true,
        tts_backend: "say".to_string(),
        tts_voice: None,
        images_enabled: true,
        image_backend: Some("kitty".to_string()),
        image_backend_manual: false,
        use_director_writer: true,
        stream_turns: true,
        stream_supported: true,
        expressions_enabled: true,
        sampling_style: "자동",
        persona_label: "없음",
        theme_name: "default",
        returns_home: false,
    };
    let out = render_to_string(&view, MIN_WIDTH - 1, MIN_HEIGHT - 1);
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
        tts_enabled: true,
        tts_backend: "say".to_string(),
        tts_voice: None,
        images_enabled: true,
        image_backend: Some("kitty".to_string()),
        image_backend_manual: false,
        use_director_writer: true,
        stream_turns: true,
        stream_supported: true,
        expressions_enabled: true,
        sampling_style: "자동",
        persona_label: "없음",
        theme_name: "default",
        returns_home: false,
    };
    let out = render_to_string(&view, 80, 24);
    assert!(
        !out.contains("api_key"),
        "settings screen leaked api_key:\n{out}"
    );
    assert!(
        !out.contains("authorization"),
        "settings screen leaked authorization:\n{out}"
    );
}

#[test]
fn settings_stream_row_states() {
    // The Settings `s` row renders five states: on/off on the single-pass
    // path, "on but the provider has no SSE" caution, and the stored
    // preference echoed through a "2단계 중" note while the two-stage
    // Director → Writer path is active (its Writer always streams, so a
    // press there must still be visible).
    fn view(
        use_director_writer: bool,
        stream_turns: bool,
        stream_supported: bool,
    ) -> SettingsView<'static> {
        let provider: &'static ProviderConfigSafe = Box::leak(Box::new(safe_loopback()));
        SettingsView {
            provider,
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
            use_director_writer,
            stream_turns,
            stream_supported,
            expressions_enabled: true,
            sampling_style: "자동",
            persona_label: "없음",
            theme_name: "default",
            returns_home: false,
        }
    }
    let single_on = render_to_string(&view(false, true, true), 80, 24);
    assert!(
        single_on.contains("스트림 (s) : 켜짐"),
        "single-pass stream-on row missing:\n{single_on}"
    );
    // On + no SSE: the stored preference stays 켜짐 but the effective turn
    // falls back to non-streaming — the row must say so instead of a clean
    // success tick.
    let single_on_unsupported = render_to_string(&view(false, true, false), 80, 24);
    assert!(
        single_on_unsupported.contains("스트림 (s) : 켜짐 · SSE 미지원"),
        "SSE-unsupported stream-on row missing:\n{single_on_unsupported}"
    );
    let single_off = render_to_string(&view(false, false, true), 80, 24);
    assert!(
        single_off.contains("스트림 (s) : 꺼짐"),
        "single-pass stream-off row missing:\n{single_off}"
    );
    let two_stage_on = render_to_string(&view(true, true, true), 80, 24);
    assert!(
        two_stage_on.contains("스트림 (s) : 2단계 중 (켜짐)"),
        "two-stage stored-on note missing:\n{two_stage_on}"
    );
    let two_stage_off = render_to_string(&view(true, false, true), 80, 24);
    assert!(
        two_stage_off.contains("스트림 (s) : 2단계 중 (꺼짐)"),
        "two-stage stored-off note missing:\n{two_stage_off}"
    );
}
