//! svd-daemon entry point.
//!
//! Parses CLI arguments, loads config, starts the IPC server.

use std::sync::{
    Arc,
    atomic::AtomicBool,
};

use clap::Parser;
use svd_daemon::{
    config::load_config,
    error::DaemonError,
    ipc::{run_server, ServerError, StubHandler},
};

/// Sunshine Virtual Display daemon (privileged)
#[derive(Parser, Debug)]
#[command(name = "svd-daemon", version, about, long_about = None)]
struct Args {
    /// Enable verbose logging (wired to tracing level in T2.3/config)
    #[arg(short, long)]
    verbose: bool,
}

fn run(_args: &Args) -> Result<(), DaemonError> {
    let config_path = std::path::Path::new("/etc/sunshine-vd/config.toml");

    let config = load_config(config_path).map_err(|e| DaemonError::Config(e.to_string()))?;

    let socket_path = std::path::PathBuf::from(&config.socket_path);

    let handler = Arc::new(StubHandler);
    // Graceful shutdown: set to true to stop the accept loop.
    // No external signal hook yet (T3.3+); the daemon runs until killed.
    let shutdown = Arc::new(AtomicBool::new(false));

    run_server(&socket_path, handler, shutdown).map_err(|e| match e {
        ServerError::Bind { path, source } => {
            DaemonError::Ipc(format!("failed to bind socket at {}: {}", path.display(), source))
        }
        ServerError::Io(e) => DaemonError::Io(e),
        ServerError::Framing(e) => DaemonError::Ipc(e.to_string()),
    })
}

fn main() {
    // Parse args first so that --help / --version exit 0 before any setup.
    let args = Args::parse();

    // Initialise structured tracing to stderr.
    // Default level is INFO; can be overridden via RUST_LOG env var.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tracing::info!("svd-daemon starting");

    if let Err(e) = run(&args) {
        tracing::error!(error = %e, "svd-daemon failed");
        std::process::exit(1);
    }
}
