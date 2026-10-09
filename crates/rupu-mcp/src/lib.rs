#![deny(clippy::all)]

//! rupu-mcp — the MCP transport over the `rupu-tools` catalog (spec W4).
//!
//! [`CatalogServer`] serves a resolved grant of the catalog over JSON-RPC:
//! `tools/list` lists the granted tools, `tools/call` runs one through the
//! same permission policy and tool bodies the agent loop and `action:`
//! workflow steps use (`rupu_tools::call`). It owns no tools.
//!
//! Transports: [`StdioTransport`] (`rupu mcp serve`, for Claude Desktop,
//! Cursor, …) and [`InProcessTransport`] (tests).

pub mod error;
pub mod server;
pub mod transport;

pub use error::{McpError, ServeError};
pub use server::{CatalogServer, MCP_DEFAULT_GRANT};
pub use transport::{InProcessTransport, StdioTransport, Transport};
