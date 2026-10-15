//! Render performance probe (증거 계약: 측정 기선 → 개선 → 재측정).
//!
//! 실제 그리기 경로(App::render → render_story)를 큰 대화 스택으로 반복
//! 측정한다. 어설션은 넉넉한 상한(회귀 게이트)이고, 정밀 수치는
//! `--nocapture` 출력으로 기록해 증거로 남긴다.

use std::time::Instant;

use ratatui::{Terminal, backend::TestBackend};
use story_model::{AppViewModel, ChoiceView, RiskLevel, StatusLine, TimeSlot};
use story_tui::App;

fn sample_view(turn: u32) -> AppViewModel {
    AppViewModel {
        screen: story_model::Screen::Story,
        story_title: "성능 프로브".to_string(),
        chapter: "chapter-1".to_string(),
        location_display: "카페 나무".to_string(),
        time_slot: TimeSlot::Evening,
        narration: "비가 그쳤다. 그녀는 창밖을 보았다.".to_string(),
        choices: vec![ChoiceView {
            id: "c1".to_string(),
            label: "시계가 멈춘 이유를 묻는다".to_string(),
            risk: RiskLevel::Medium,
            preview: Some("회피하던 시간의 의미를 시험한다".to_string()),
        }],
        status: StatusLine {
            route: None,
            affection: Some(3),
            trust: Some(1),
            tension: Some(8),
            turn,
        },
        loading: None,
        error: None,
    }
}

/// 턴당 대략 400자 내외의 내레이션을 가진 엔트리를 채운다.
fn entry(turn: u32) -> story_tui::StoryStackEntry {
    let narration = format!(
        "{turn}턴 — 유나: “그건 네 해석이고.”\n\n카페 나무 창가에 비가 그쳤다. 그녀는 **시계**를 \
         톡톡 두드리고, 창밖의 흐린 하늘을 오래 바라보았다. 대화는 천천히 이어진다.\n\n- 관찰: 오른손 시계\n- 감정: 경계"
    );
    story_tui::StoryStackEntry {
        turn,
        scene_id: format!("scene-{turn}"),
        model: Some("test-model".to_string()),
        generation_secs: Some(42),
        player_action: Some("조용히 자리에 앉는다".to_string()),
        narration,
    }
}

#[test]
fn render_probe_full_stack_draw_timing() {
    let stack: Vec<_> = (1..=120).map(entry).collect();
    let mut app = App::new(sample_view(120));
    app.set_lists(&[], &[]);
    app.replace_story_stack(stack);

    let mut terminal = Terminal::new(TestBackend::new(100, 40)).expect("terminal");

    // 워밍업 5프레임 후 60프레임 측정.
    for _ in 0..5 {
        terminal
            .draw(|frame| app.render(frame, frame.area()))
            .expect("draw");
    }
    let frames = 60;
    let start = Instant::now();
    for _ in 0..frames {
        terminal
            .draw(|frame| app.render(frame, frame.area()))
            .expect("draw");
    }
    let elapsed = start.elapsed();
    let per_frame_ms = elapsed.as_secs_f64() * 1000.0 / frames as f64;
    println!("스택 120턴, 100x40: 프레임당 {per_frame_ms:.2}ms ({frames}프레임, 총 {elapsed:?})");

    // 회귀 게이트: 넉넉한 상한. 캐시 없는 매 프레임 재조합 경로는 이 상한을
    // 넘는 머신도 있다 — 수치 자체는 --nocapture 출력으로 판단한다.
    assert!(
        per_frame_ms < 50.0,
        "프레임당 {per_frame_ms:.2}ms — 50ms 상한 초과"
    );
}
