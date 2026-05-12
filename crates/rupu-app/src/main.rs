//! rupu.app — native macOS desktop app.

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("rupu_app=debug,gpui=info")
        .init();
    tracing::info!("rupu.app starting");
    // GPUI boot lands in Task 10; workspace + palette types live in
    // the library (src/lib.rs) so integration tests can reach them.
}
