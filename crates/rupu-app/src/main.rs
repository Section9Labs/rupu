//! rupu.app — native macOS desktop app.
//!
//! DIAGNOSTIC BUILD #2: stripped-down main.rs that does NOTHING our
//! real app does — no Tokio runtime, no menu install, no status item,
//! no workspace opening. Just `application().run` + open a hello-world
//! window. If text renders here, one of the setup steps we strip is
//! the culprit. If text still doesn't render, the problem is
//! lower-level (build cache, gpui dep mismatch, something else).

use gpui::{
    div, prelude::*, px, rgb, size, App, Bounds, Context, IntoElement, Render, SharedString,
    Window, WindowBounds, WindowOptions,
};

struct HelloWorldDiag;

impl Render for HelloWorldDiag {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .bg(rgb(0x505050))
            .size_full()
            .justify_center()
            .items_center()
            .text_xl()
            .text_color(rgb(0xffffff))
            .child(SharedString::from("Hello, stripped!"))
    }
}

fn main() {
    gpui_platform::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(500.0), px(500.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| HelloWorldDiag),
        )
        .unwrap();
        cx.activate(true);
    });
}
