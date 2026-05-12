//! rupu.app — native macOS desktop app.
//!
//! See `docs/superpowers/specs/2026-05-11-rupu-slice-d-app-design.md`.

use gpui::App;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "rupu_app=debug,gpui=info".into()),
        )
        .init();
    tracing::info!("rupu.app starting");

    gpui_platform::application().run(|cx: &mut App| {
        // Activate the app so it gets focus on launch.
        cx.activate(true);

        // No windows yet — they land in Task 11. For now we boot
        // the app loop so `cargo run -p rupu-app` proves the binary
        // works.
        tracing::info!("rupu.app app-loop entered (no windows yet)");
    });
}
