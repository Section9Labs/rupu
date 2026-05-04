#![deny(clippy::all)]

//! rupu-mcp — embedded MCP server for the unified SCM tool catalog.
//!
//! Two transports:
//!   - [`InProcessTransport`] — used by the agent runtime; tools dispatched
//!     by direct calls without serialization round-trips.
//!   - [`StdioTransport`] — used by `rupu mcp serve` (Plan 3 Task 1) for
//!     external MCP-aware clients (Claude Desktop, Cursor).
//!
//! Spec: docs/superpowers/specs/2026-05-03-rupu-slice-b2-scm-design.md §6.

pub mod dispatcher;
pub mod error;
pub mod permission;
pub mod schema;
pub mod server;
pub mod tools;
pub mod transport;

pub use error::McpError;
pub use server::{serve_in_process, McpServer, ServeHandle};
pub use transport::{InProcessTransport, StdioTransport, Transport};
