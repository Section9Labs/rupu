//! Reviewed operator writes, deliberately excluded from model MCP tools.
use crate::state::AppState;
pub const FEATURE: &str = "forge.operator_write.v1";
pub fn available(s: &AppState) -> bool {
    cfg!(unix) && s.repos.as_ref().and_then(|p| p.forge_writer()).is_some()
}
#[cfg(unix)]
mod owner;
#[cfg(unix)]
mod routes;
#[cfg(unix)]
pub use routes::routes;
#[cfg(not(unix))]
pub fn routes() -> Router<AppState> {
    Router::new()
}
