use std::time::{Duration, Instant};

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

#[derive(Debug, Clone)]
pub struct Toast {
    pub text: String,
    pub kind: ToastKind,
    pub created_at: Instant,
    pub ttl: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Ok,
    Warn,
    Err,
    /// Persistent gate prompt — never auto-expires; cleared when gate resolves.
    GatePrompt,
}

impl Toast {
    pub fn ok(text: impl Into<String>) -> Self {
        Self::new(text, ToastKind::Ok, Duration::from_secs(2))
    }
    pub fn err(text: impl Into<String>) -> Self {
        Self::new(text, ToastKind::Err, Duration::from_secs(5))
    }
    pub fn warn(text: impl Into<String>) -> Self {
        Self::new(text, ToastKind::Warn, Duration::from_secs(3))
    }
    pub fn gate(text: impl Into<String>) -> Self {
        // ttl ignored when GatePrompt — App clears explicitly.
        Self::new(text, ToastKind::GatePrompt, Duration::from_secs(0))
    }

    fn new(text: impl Into<String>, kind: ToastKind, ttl: Duration) -> Self {
        Self { text: text.into(), kind, created_at: Instant::now(), ttl }
    }

    pub fn expired(&self, now: Instant) -> bool {
        self.kind != ToastKind::GatePrompt && now.duration_since(self.created_at) >= self.ttl
    }
}

pub fn render_toast(frame: &mut Frame, area: Rect, toast: &Toast) {
    let color = match toast.kind {
        ToastKind::Info       => Color::Cyan,
        ToastKind::Ok         => Color::Green,
        ToastKind::Warn       => Color::Yellow,
        ToastKind::Err        => Color::Red,
        ToastKind::GatePrompt => Color::LightYellow,
    };
    let mut style = Style::default().fg(color);
    // Pulse for gate prompts: bold every other second.
    if toast.kind == ToastKind::GatePrompt {
        let secs = toast.created_at.elapsed().as_secs();
        if secs.is_multiple_of(2) {
            style = style.add_modifier(Modifier::BOLD);
        }
    }
    let block = Block::default().borders(Borders::TOP);
    frame.render_widget(Paragraph::new(toast.text.clone()).style(style).block(block), area);
}
