use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rupu_tui::control::{dispatch, Action};

fn k(c: KeyCode) -> KeyEvent {
    KeyEvent::new(c, KeyModifiers::NONE)
}

#[test]
fn q_is_quit() {
    assert_eq!(dispatch(k(KeyCode::Char('q'))), Action::Quit);
}

#[test]
fn tab_is_focus_next() {
    assert_eq!(dispatch(k(KeyCode::Tab)), Action::FocusNext);
}

#[test]
fn v_is_toggle_view() {
    assert_eq!(dispatch(k(KeyCode::Char('v'))), Action::ToggleView);
}

#[test]
fn a_is_approve() {
    assert_eq!(dispatch(k(KeyCode::Char('a'))), Action::ApproveFocused);
}

#[test]
fn r_is_reject() {
    assert_eq!(dispatch(k(KeyCode::Char('r'))), Action::RejectFocused);
}

#[test]
fn unknown_is_noop() {
    assert_eq!(dispatch(k(KeyCode::Char('x'))), Action::NoOp);
}
