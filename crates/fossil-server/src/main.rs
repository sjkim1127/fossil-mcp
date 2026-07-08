use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::{EnvFilter, fmt};

mod tools;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Log to stderr so it doesn't interfere with the MCP stdio transport.
    fmt()
        .with_env_filter(
            EnvFilter::try_from_env("FOSSIL_LOG")
                .unwrap_or_else(|_| EnvFilter::new("fossil_server=info,warn")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    tracing::info!("fossil-mcp server starting (stdio transport)");

    let service = tools::FossilServer::new();
    let server = service.serve(stdio()).await?;
    server.waiting().await?;

    tracing::info!("fossil-mcp server stopped");
    Ok(())
}
