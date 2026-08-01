//! Headless snapshot tests for the Story screen (ST-021).
//!
//! Renders an [`App`] into a `ratatui::TestBackend` buffer (no real terminal)
//! and pins the output via `insta`. Four cases are covered per the plan:
//! normal story, loading overlay, error overlay, and the below-min-size
//! warning — plus a wider 120×40 variant of the normal view.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use story_model::uuid::Uuid;
use story_model::{
    AppViewModel, ChoiceView, DomainErrorView, LoadingState, RiskLevel, Screen, StatusLine,
    TimeSlot,
};
use story_tui::App;

fn korean_narration() -> &'static str {
    "비가 저녁 내내 창을 두드린다. 카페 나무의 문을 열면 젖은 나무와 커피 냄새가 먼저 닿는다. \
     한유나는 창가 안쪽 자리에 앉아 있다. 짙은 남색 코트는 옆 의자에 반듯하게 접혀 있고, \
     오른손의 은색 시계가 잔을 들 때마다 빛난다."
}

fn base_view() -> AppViewModel {
    AppViewModel {
        screen: Screen::Story,
        story_title: "비가 그친 뒤".to_string(),
        chapter: "chapter-1".to_string(),
        location_display: "카페 나무".to_string(),
        time_slot: TimeSlot::Evening,
        narration: korean_narration().to_string(),
        choices: vec![
            ChoiceView {
                id: "choice-0".to_string(),
                label: "먼저 사과한다".to_string(),
                risk: RiskLevel::Low,
                preview: None,
            },
            ChoiceView {
                id: "choice-1".to_string(),
                label: "아무 말 없이 앉는다".to_string(),
                risk: RiskLevel::Medium,
                preview: None,
            },
            ChoiceView {
                id: "choice-2".to_string(),
                label: "빗소리를 핑계로 시선을 피한다".to_string(),
                risk: RiskLevel::Safe,
                preview: None,
            },
        ],
        status: StatusLine {
            route: None,
            affection: Some(3),
            trust: Some(1),
            tension: Some(8),
            turn: 1,
        },
        loading: None,
        error: None,
    }
}

fn render_to_string(view: AppViewModel, w: u16, h: u16) -> String {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let app = App::new(view);
    render_app_to_string(&mut terminal, &app)
}

fn render_to_string_with_scroll(view: AppViewModel, w: u16, h: u16, scroll: u16) -> String {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let mut app = App::new(view);
    app.narration_scroll = scroll;
    render_app_to_string(&mut terminal, &app)
}

fn render_app_to_string(terminal: &mut Terminal<TestBackend>, app: &App) -> String {
    terminal
        .draw(|f| {
            app.render(f, f.area());
        })
        .expect("draw");
    buffer_to_string(terminal.backend().buffer())
}

/// Convert a ratatui buffer into a trimmed string snapshot: each row is the
/// raw cell symbols with trailing whitespace stripped. The trailing cell of a
/// double-width character (which `TestBackend` records as a space) is skipped
/// so Hangul reads naturally (`비 가` → `비가`).
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
            // If this cell holds a single double-width char, the next cell is
            // its visual continuation (recorded as " " by TestBackend).
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
fn snapshot_story_normal_80x24() {
    insta::assert_snapshot!(render_to_string(base_view(), 80, 24));
}

#[test]
fn snapshot_story_normal_120x40() {
    insta::assert_snapshot!(render_to_string(base_view(), 120, 40));
}

#[test]
fn snapshot_story_long_choice_wraps_without_truncation() {
    let mut view = base_view();
    view.choices[0].label = "그날 무슨 일이 있었는지, 지금이라도 솔직하게 이야기해 줄 수 있나요? 그리고 제가 기다렸던 이유도 알고 싶어요."
        .to_string();
    insta::assert_snapshot!(render_to_string(view, 80, 24));
}

#[test]
fn snapshot_story_scrollbar_shows_progress_for_long_transcript() {
    let mut view = base_view();
    view.narration = (0..24)
        .map(|n| {
            format!(
                "{}번째 기록입니다. 비가 그친 카페에서 유나는 대답을 망설인다.",
                n + 1
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    insta::assert_snapshot!(render_to_string_with_scroll(view, 80, 24, 12));
}

#[test]
fn snapshot_story_loading_overlay() {
    let mut v = base_view();
    v.loading = Some(LoadingState {
        generation_id: Uuid::nil(),
        message_ko: "생성 중...".to_string(),
    });
    insta::assert_snapshot!(render_to_string(v, 80, 24));
}

#[test]
fn snapshot_story_error_overlay() {
    let mut v = base_view();
    v.error = Some(DomainErrorView {
        code: "patch_unknown_flag".to_string(),
        title_ko: "생성 실패".to_string(),
        message_ko: "알 수 없는 플래그를 참조했습니다: nonexistent_flag".to_string(),
        recoverable: true,
    });
    insta::assert_snapshot!(render_to_string(v, 80, 24));
}

#[test]
fn snapshot_story_below_min_size() {
    // 30×8 is well under MIN_WIDTH×MIN_HEIGHT; the warning fires.
    insta::assert_snapshot!(render_to_string(base_view(), 30, 8));
}
