#![deny(clippy::all)]

//! rupu-mcp — the MCP transport over the `rupu-tools` catalog (spec W4).
//!
//! [`McpServer`] is the protocol core over pluggable [`ToolSource`]s;
//! [`CatalogSource`] serves the catalog through the same permission policy
//! and tool bodies the agent loop and `action:` workflow steps use
//! (`rupu_tools::call`). It owns no tools.
//!
//! The only transport that ships is HTTP (in `rupu-cp`);
//! [`InProcessTransport`] is for tests.

pub mod catalog;
pub mod error;
pub mod filter;
pub mod server;
pub mod source;
pub mod transport;

pub use catalog::{CatalogSource, ContextFactory};
pub use error::{McpError, ServeError};
pub use filter::ToolFilter;
pub use server::{negotiate_protocol_version, HandleCx, McpServer, PROTOCOL_VERSIONS};
pub use source::{CallCx, EmbeddedResource, ToolDef, ToolHints, ToolResult, ToolSource};
pub use transport::{decode_line, InProcessTransport, Transport};
