//! Headless snapshot test for the SaveLoad screen (W8, ST-028).

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use story_persistence::SlotMeta;
use story_tui::render_save_load;

use chrono::DateTime;

fn slot(slot_id: &str, story_id: &str, desc: Option<&str>, when: &str) -> SlotMeta {
    SlotMeta {
        slot_id: slot_id.to_string(),
        story_id: story_id.to_string(),
        saved_at: DateTime::parse_from_rfc3339(when)
            .unwrap()
            .with_timezone(&chrono::Utc),
        chapter_label: "chapter-1".to_string(),
        scene_label: "cafe-namu".to_string(),
        description: desc.map(str::to_string),
    }
}

fn render_to_string(slots: &[SlotMeta], selected: usize, w: u16, h: u16) -> String {
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|f| {
            render_save_load(
                f,
                f.area(),
                "저장 / 불러오기",
                slots,
                selected,
                "↑↓ 이동 · Enter 불러오기 · Del 삭제 · Esc 닫기",
                None,
            );
        })
        .expect("draw");
    buffer_to_string(terminal.backend().buffer())
}

/// Convert a ratatui buffer into a trimmed string snapshot (same shape as the
/// Story-screen snapshot helper).
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
fn snapshot_save_load_three_slots() {
    let slots = vec![
        slot(
            "autosave",
            "rainy-reunion",
            Some("턴 12 — 카페 나무"),
            "2026-08-01T09:30:00Z",
        ),
        slot(
            "slot-1",
            "rainy-reunion",
            Some("처음 만남 직후"),
            "2026-07-30T22:11:00Z",
        ),
        slot("slot-2", "rainy-reunion", None, "2026-07-29T18:00:00Z"),
    ];
    insta::assert_snapshot!(render_to_string(&slots, 1, 80, 16));
}

#[test]
fn snapshot_save_load_empty() {
    let slots: Vec<SlotMeta> = Vec::new();
    insta::assert_snapshot!(render_to_string(&slots, 0, 80, 16));
}
