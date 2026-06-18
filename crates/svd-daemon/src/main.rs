//! svd-daemon entry point.
//!
//! Parses CLI arguments, loads config, starts the IPC server.

use std::sync::{
    Arc,
    atomic::AtomicBool,
};

use clap::Parser;
use signal_hook::consts::{SIGINT, SIGTERM};
use svd_daemon::{
    config::load_config,
    error::DaemonError,
    handler::RealHandler,
    ipc::{run_server, RequestHandler, ServerError},
    strategy::{DisplayStrategy, kwin::KWinStrategy},
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

    let strategy = Arc::new(KWinStrategy::new(
        std::path::PathBuf::from(&config.state_path),
        config.output_ready_timeout_secs,
    ));

    // Attempt to restore state from a previous run (daemon restart).
    if let Err(e) = strategy.restore() {
        tracing::warn!(error = %e, "restore() failed on startup — starting fresh");
    }

    let handler: Arc<dyn RequestHandler> = Arc::new(RealHandler::new(strategy));
    let shutdown = Arc::new(AtomicBool::new(false));

    signal_hook::flag::register(SIGTERM, Arc::clone(&shutdown))
        .expect("signal registration");
    signal_hook::flag::register(SIGINT, Arc::clone(&shutdown))
        .expect("signal registration");

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
    // Priority: RUST_LOG env var > --verbose flag > default "info".
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| {
            let level = if args.verbose { "debug" } else { "info" };
            tracing_subscriber::EnvFilter::new(level)
        });

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .init();

    tracing::info!("svd-daemon starting");

    if let Err(e) = run(&args) {
        tracing::error!(error = %e, "svd-daemon failed");
        std::process::exit(1);
    }
}
