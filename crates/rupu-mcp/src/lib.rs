#![deny(clippy::all)]

//! rupu-mcp — the MCP transport over the `rupu-tools` catalog (spec W4).
//!
//! [`CatalogServer`] serves a resolved grant of the catalog over JSON-RPC:
//! `tools/list` lists the granted tools, `tools/call` runs one through the
//! same permission policy and tool bodies the agent loop and `action:`
//! workflow steps use (`rupu_tools::call`). It owns no tools.
//!
//! [`McpServer`] is the protocol core over pluggable [`ToolSource`]s.
//!
//! Transports: [`StdioTransport`] (`rupu mcp serve`, for Claude Desktop,
//! Cursor, …) and [`InProcessTransport`] (tests).

pub mod catalog;
pub mod error;
pub mod filter;
pub mod server;
pub mod source;
pub mod transport;

pub use catalog::{CatalogSource, ContextFactory, HIDDEN_BY_OPERATOR_TOOLS};
pub use error::{McpError, ServeError};
pub use filter::ToolFilter;
pub use server::{
    negotiate_protocol_version, CatalogServer, McpServer, MCP_DEFAULT_GRANT, PROTOCOL_VERSIONS,
};
pub use source::{CallCx, ToolDef, ToolHints, ToolResult, ToolSource};
pub use transport::{decode_line, InProcessTransport, StdioTransport, Transport};
