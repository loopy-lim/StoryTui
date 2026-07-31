//! Key mapping for the Story screen (ST-021).
//!
//! Translates a [`crossterm`]/`ratatui` [`KeyEvent`] into either a selection
//! mutation (handled inside [`crate::app::App`]) or an [`EngineCommand`] the
//! host should feed back to the engine. Full keymap (per Technical Spec) lands
//! later; only the Story-screen keys needed for the W5 slice are mapped here.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use story_core::engine_cmd::EngineCommand;

/// Outcome of mapping one key.
#[derive(Clone, Debug, PartialEq)]
pub enum KeyAction {
    /// Nothing to do (unmapped key).
    None,
    /// Move the selection cursor by `delta` (clamped by the App).
    MoveSelection(i32),
    /// An engine command the host should run.
    Command(EngineCommand),
    /// The host should terminate the event loop.
    Quit,
}

/// Map a Story-screen key. The number-row keys `1..=9` select + confirm the
/// nth choice directly; arrow/j/k move the cursor; Enter confirms the
/// highlighted choice.
pub fn map_key(key: KeyEvent, choice_count: usize, loading: bool) -> KeyAction {
    // Ctrl+C always quits.
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return KeyAction::Quit;
    }
    match key.code {
        KeyCode::Char('q') => KeyAction::Quit,
        KeyCode::Char('j') | KeyCode::Down => KeyAction::MoveSelection(1),
        KeyCode::Char('k') | KeyCode::Up => KeyAction::MoveSelection(-1),
        KeyCode::Enter => {
            // Enter on a highlighted choice: host resolves the actual id.
            // We signal "confirm current selection" by emitting a placeholder
            // SubmitChoice with the raw index-encoded id; the App rewrites it.
            confirm_selection()
        }
        KeyCode::Char(c @ '1'..='9') => {
            let idx = (c as u8 - b'1') as usize;
            if idx < choice_count {
                direct_confirm(idx)
            } else {
                KeyAction::None
            }
        }
        KeyCode::Char('c') if loading => {
            KeyAction::Command(EngineCommand::CancelGeneration {
                // The host fills in the pending generation id from the view's
                // loading state; we send nil and let the host resolve it.
                generation_id: story_model::uuid::Uuid::nil(),
            })
        }
        KeyCode::Esc => KeyAction::Command(EngineCommand::DismissError),
        _ => KeyAction::None,
    }
}

/// `Enter` — confirm the currently highlighted choice. Encoded as a
/// `SubmitChoice` whose `choice_id` is the sentinel `__select__{n}`; the App
/// rewrites it to the real choice id at `selected` before forwarding to the
/// engine. (This keeps `map_key` free of mutable selection state.)
fn confirm_selection() -> KeyAction {
    KeyAction::Command(EngineCommand::SubmitChoice {
        choice_id: SELECT_SENTINEL.to_string(),
    })
}

/// Number-row direct confirm of choice `idx`. Same sentinel scheme with the
/// index baked in: `__select__{idx}`.
fn direct_confirm(idx: usize) -> KeyAction {
    KeyAction::Command(EngineCommand::SubmitChoice {
        choice_id: format!("{SELECT_SENTINEL}{idx}"),
    })
}

/// Sentinel the App rewrites into the real `selected` choice id.
pub const SELECT_SENTINEL: &str = "__select__";

/// Returns `Some(idx)` when `id` is a number-row direct confirm sentinel.
pub fn parse_direct_select_index(id: &str) -> Option<usize> {
    id.strip_prefix(SELECT_SENTINEL)
        .and_then(|rest| rest.parse::<usize>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn ctrl_c_quits() {
        let k = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(map_key(k, 3, false), KeyAction::Quit);
    }

    #[test]
    fn j_down_arrow_move_down() {
        assert_eq!(map_key(key(KeyCode::Char('j')), 3, false), KeyAction::MoveSelection(1));
        assert_eq!(map_key(key(KeyCode::Down), 3, false), KeyAction::MoveSelection(1));
    }

    #[test]
    fn k_up_arrow_move_up() {
        assert_eq!(map_key(key(KeyCode::Char('k')), 3, false), KeyAction::MoveSelection(-1));
        assert_eq!(map_key(key(KeyCode::Up), 3, false), KeyAction::MoveSelection(-1));
    }

    #[test]
    fn number_row_confirms_within_range() {
        let a = map_key(key(KeyCode::Char('2')), 3, false);
        let KeyAction::Command(EngineCommand::SubmitChoice { choice_id }) = a else {
            panic!("not a submit: {a:?}");
        };
        assert_eq!(parse_direct_select_index(&choice_id), Some(1));
    }

    #[test]
    fn number_row_out_of_range_is_none() {
        assert_eq!(map_key(key(KeyCode::Char('9')), 2, false), KeyAction::None);
    }

    #[test]
    fn cancel_only_when_loading() {
        let idle = map_key(key(KeyCode::Char('c')), 3, false);
        assert_eq!(idle, KeyAction::None, "c ignored when not loading");
        let loading = map_key(key(KeyCode::Char('c')), 3, true);
        assert!(matches!(loading, KeyAction::Command(EngineCommand::CancelGeneration { .. })));
    }

    #[test]
    fn q_quits() {
        assert_eq!(map_key(key(KeyCode::Char('q')), 3, false), KeyAction::Quit);
    }
}
