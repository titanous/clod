use clod::{router, Settings};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .init();
    let settings = Settings::from_env()?;
    settings.check_version().await;
    let listener = tokio::net::TcpListener::bind(&settings.listen).await?;
    tracing::info!(address = %listener.local_addr()?, "listening");
    axum::serve(listener, router(settings)).await?;
    Ok(())
}
