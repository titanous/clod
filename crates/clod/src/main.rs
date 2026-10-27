use clod::{router_with_groups, ProcessGroups, Settings};
use std::{future::IntoFuture, process::ExitCode, time::Duration};
use tokio::signal::unix::{signal, SignalKind};

fn main() -> Result<ExitCode, Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .init();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let groups = ProcessGroups::default();
    let result = runtime.block_on(serve(groups.clone()));
    // Independent of task drops: a worker may be stuck in synchronous tracing or
    // measurement I/O. Kill registered unreaped groups and prohibit new spawns
    // before imposing the worker shutdown deadline. Never log on this path.
    groups.shutdown();
    runtime.shutdown_timeout(Duration::from_secs(3));
    result.map(ExitCode::from)
}

async fn serve(groups: ProcessGroups) -> Result<u8, Box<dyn std::error::Error>> {
    // Register before starting children. Further signals remain handled during
    // bounded teardown; SIGQUIT is deliberately left at its default disposition.
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut hangup = signal(SignalKind::hangup())?;
    let settings = Settings::from_env()?;
    let serve = async {
        settings.check_version().await;
        let listener = tokio::net::TcpListener::bind(&settings.listen).await?;
        tracing::info!(address = %listener.local_addr()?, "listening");
        axum::serve(listener, router_with_groups(settings, groups))
            .into_future()
            .await
    };
    let code = tokio::select! {
        result = serve => { result?; 0 }
        _ = terminate.recv() => 128 + libc::SIGTERM as u8,
        _ = interrupt.recv() => 128 + libc::SIGINT as u8,
        _ = hangup.recv() => 128 + libc::SIGHUP as u8,
    };
    // No draining: dropping serve closes the listener and cuts off requests.
    Ok(code)
}
