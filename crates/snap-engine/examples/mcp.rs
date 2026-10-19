//! The MCP server on stdio, for piping JSON-RPC sessions into:
//! `echo {"jsonrpc":"2.0","id":1,"method":"tools/list"} | cargo run -p snap-engine --example mcp`.
//!
//! Logs go to stderr; stdout carries protocol messages only.

use snap_core::Settings;
use snap_engine::{Engine, mcp};

fn main() -> snap_core::Result<()> {
    tracing_subscriber::fmt().with_writer(std::io::stderr).with_env_filter("warn").init();
    snap_platform::policy::load();
    mcp::serve_stdio(Engine::new(Settings::load_default()))
}
