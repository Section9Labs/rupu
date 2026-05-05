use crossterm::event::{KeyCode, KeyEvent};

pub mod approval;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    NoOp,
    Quit,
    FocusNext,
    FocusPrev,
    ToggleView,
    Expand,
    ApproveFocused,
    RejectFocused,
    FilterCompleted,
    Search,
    Help,
}

pub fn dispatch(ev: KeyEvent) -> Action {
    match ev.code {
        KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
        KeyCode::Tab => Action::FocusNext,
        KeyCode::BackTab => Action::FocusPrev,
        KeyCode::Char('v') => Action::ToggleView,
        KeyCode::Enter => Action::Expand,
        KeyCode::Char('a') => Action::ApproveFocused,
        KeyCode::Char('r') => Action::RejectFocused,
        KeyCode::Char('f') => Action::FilterCompleted,
        KeyCode::Char('/') => Action::Search,
        KeyCode::Char('?') => Action::Help,
        _ => Action::NoOp,
    }
}
