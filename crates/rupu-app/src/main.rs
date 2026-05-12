//! rupu.app — native macOS desktop app.
//!
//! See `docs/superpowers/specs/2026-05-11-rupu-slice-d-app-design.md`.

mod palette;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("rupu_app=debug,gpui=info")
        .init();
    tracing::info!("rupu.app starting");
    // GPUI boot lands in Task 10.
}
