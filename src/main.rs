//! Entrypoint: initialises Tokio runtime, tracing (stderr), and the MCP
//! stdio transport. All logging goes to stderr because stdout carries the
//! MCP wire protocol.

use mcp_rustdoc::server::RustdocServer;
use rmcp::{ServiceExt, transport::stdio};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    tracing::info!("Starting mcp-rustdoc");

    let server = RustdocServer::new()?;
    let service = server.serve(stdio()).await?;
    service.waiting().await?;

    Ok(())
}
