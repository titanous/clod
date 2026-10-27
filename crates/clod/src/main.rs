use clod::{router, Settings};
use std::{future::IntoFuture, process::ExitCode};
use tokio::signal::unix::{signal, SignalKind};

#[tokio::main]
async fn main() -> Result<ExitCode, Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .init();
    // Register before starting children. Tokio keeps these handlers installed, so
    // another signal during runtime teardown cannot bypass process cleanup.
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let settings = Settings::from_env()?;
    let serve = async {
        settings.check_version().await;
        let listener = tokio::net::TcpListener::bind(&settings.listen).await?;
        tracing::info!(address = %listener.local_addr()?, "listening");
        axum::serve(listener, router(settings)).into_future().await
    };
    let code = tokio::select! {
        result = serve => { result?; 0 }
        _ = terminate.recv() => 128 + libc::SIGTERM as u8,
        _ = interrupt.recv() => 128 + libc::SIGINT as u8,
    };
    // No draining: dropping serve closes the listener. Returning destroys the
    // runtime, cancelling every runner (including ones waiting for a permit or
    // key lock). Each owned Process drops through its unreaped-only group kill;
    // no separate PID registry can race with wait() or retain a reused PGID.
    Ok(ExitCode::from(code))
}
