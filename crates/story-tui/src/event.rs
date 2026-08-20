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
pub fn map_key(key: KeyEvent, choice_count: usize, loading: bool, has_error: bool) -> KeyAction {
    // Ctrl+C always quits.
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return KeyAction::Quit;
    }
    // A recoverable-error overlay is modal. Its footer advertises only retry,
    // dismiss, and quit, so never let hidden story commands execute behind it.
    if has_error {
        return match key.code {
            KeyCode::Char('q' | 'ㅂ') => KeyAction::Quit,
            KeyCode::Char('r' | 'ㄱ') => KeyAction::Command(EngineCommand::RegenerateCurrentTurn),
            KeyCode::Esc => KeyAction::Command(EngineCommand::DismissError),
            _ => KeyAction::None,
        };
    }
    match key.code {
        // Accept the same physical shortcuts while a Korean 2-beolsik layout
        // is active: q=ㅂ, r=ㄱ, c=ㅊ, j=ㅓ, k=ㅏ, n=ㅜ.
        KeyCode::Char('q' | 'ㅂ') => KeyAction::Quit,
        KeyCode::Char('n' | 'ㅜ') => KeyAction::Command(EngineCommand::StartNewGame),
        KeyCode::Char('j' | 'ㅓ') | KeyCode::Down => KeyAction::MoveSelection(1),
        KeyCode::Char('k' | 'ㅏ') | KeyCode::Up => KeyAction::MoveSelection(-1),
        KeyCode::Enter => {
            // Enter on a highlighted choice: the App rewrites the sentinel to
            // the real choice id at `selected`.
            KeyAction::Command(EngineCommand::SubmitChoice {
                choice_id: SELECT_SENTINEL.to_string(),
            })
        }
        KeyCode::Char(c @ '1'..='9') => {
            let idx = (c as u8 - b'1') as usize;
            if idx < choice_count {
                KeyAction::Command(EngineCommand::SubmitChoice {
                    choice_id: format!("{SELECT_SENTINEL}{idx}"),
                })
            } else {
                KeyAction::None
            }
        }
        KeyCode::Char('c' | 'ㅊ') if loading => {
            KeyAction::Command(EngineCommand::CancelGeneration {
                // The host fills in the pending generation id from the view's
                // loading state; nil here, resolved by the App.
                generation_id: story_model::uuid::Uuid::nil(),
            })
        }
        KeyCode::Char('r' | 'ㄱ') if has_error => {
            KeyAction::Command(EngineCommand::RegenerateCurrentTurn)
        }
        KeyCode::Esc => KeyAction::Command(EngineCommand::DismissError),
        _ => KeyAction::None,
    }
}

/// Sentinel the App rewrites into the real `selected` choice id.
pub const SELECT_SENTINEL: &str = "__select__";

/// Returns `Some(idx)` when `id` is a number-row direct confirm sentinel
/// (`__select__{idx}`).
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
        assert_eq!(map_key(k, 3, false, false), KeyAction::Quit);
    }

    #[test]
    fn j_down_arrow_move_down() {
        assert_eq!(
            map_key(key(KeyCode::Char('j')), 3, false, false),
            KeyAction::MoveSelection(1)
        );
        assert_eq!(
            map_key(key(KeyCode::Down), 3, false, false),
            KeyAction::MoveSelection(1)
        );
    }

    #[test]
    fn k_up_arrow_move_up() {
        assert_eq!(
            map_key(key(KeyCode::Char('k')), 3, false, false),
            KeyAction::MoveSelection(-1)
        );
        assert_eq!(
            map_key(key(KeyCode::Up), 3, false, false),
            KeyAction::MoveSelection(-1)
        );
    }

    #[test]
    fn number_row_confirms_within_range() {
        let a = map_key(key(KeyCode::Char('2')), 3, false, false);
        let KeyAction::Command(EngineCommand::SubmitChoice { choice_id }) = a else {
            panic!("not a submit: {a:?}");
        };
        assert_eq!(parse_direct_select_index(&choice_id), Some(1));
    }

    #[test]
    fn number_row_out_of_range_is_none() {
        assert_eq!(
            map_key(key(KeyCode::Char('9')), 2, false, false),
            KeyAction::None
        );
    }

    #[test]
    fn cancel_only_when_loading() {
        let idle = map_key(key(KeyCode::Char('c')), 3, false, false);
        assert_eq!(idle, KeyAction::None, "c ignored when not loading");
        let loading = map_key(key(KeyCode::Char('c')), 3, true, false);
        assert!(matches!(
            loading,
            KeyAction::Command(EngineCommand::CancelGeneration { .. })
        ));
    }

    #[test]
    fn q_quits() {
        assert_eq!(
            map_key(key(KeyCode::Char('q')), 3, false, false),
            KeyAction::Quit
        );
    }

    #[test]
    fn n_starts_a_new_game() {
        assert_eq!(
            map_key(key(KeyCode::Char('n')), 0, false, false),
            KeyAction::Command(EngineCommand::StartNewGame)
        );
        assert_eq!(
            map_key(key(KeyCode::Char('ㅜ')), 0, false, false),
            KeyAction::Command(EngineCommand::StartNewGame)
        );
    }

    #[test]
    fn korean_layout_shortcuts_match_physical_keys() {
        assert_eq!(
            map_key(key(KeyCode::Char('ㄱ')), 3, false, true),
            KeyAction::Command(EngineCommand::RegenerateCurrentTurn)
        );
        assert_eq!(
            map_key(key(KeyCode::Char('ㅂ')), 3, false, false),
            KeyAction::Quit
        );
    }

    #[test]
    fn r_retries_only_when_error_is_visible() {
        assert_eq!(
            map_key(key(KeyCode::Char('r')), 3, false, false),
            KeyAction::None
        );
        assert_eq!(
            map_key(key(KeyCode::Char('r')), 3, false, true),
            KeyAction::Command(EngineCommand::RegenerateCurrentTurn)
        );
    }

    #[test]
    fn error_overlay_blocks_story_commands_and_navigation() {
        for code in [
            KeyCode::Enter,
            KeyCode::Down,
            KeyCode::Char('1'),
            KeyCode::Char('n'),
        ] {
            assert_eq!(map_key(key(code), 3, false, true), KeyAction::None);
        }
        assert_eq!(
            map_key(key(KeyCode::Esc), 3, false, true),
            KeyAction::Command(EngineCommand::DismissError)
        );
    }
}
