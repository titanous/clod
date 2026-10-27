use clod::{router_with_groups, ProcessGroups, Settings};
use std::{future::IntoFuture, io::Write, process::ExitCode, time::Duration};
use tokio::signal::unix::{signal, SignalKind};

fn main() -> Result<ExitCode, Box<dyn std::error::Error>> {
    // Diagnostics go to stderr at `info` unless RUST_LOG says otherwise. Stdout
    // carries only the startup address line (see `serve`).
    let filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(tracing_subscriber::filter::LevelFilter::INFO.into())
        .from_env_lossy();
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()) && !no_color)
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
        let address = listener.local_addr()?;
        tracing::info!(%address, "listening");
        // Stable machine-readable line on stdout for supervisors and harnesses
        // that discover the port (they match `address=`). Write errors, such
        // as a closed pipe, are ignored rather than aborting startup.
        let _ = writeln!(std::io::stdout(), "listening address={address}");
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
