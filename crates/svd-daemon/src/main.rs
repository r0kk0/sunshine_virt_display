//! svd-daemon entry point.
//!
//! Parses CLI arguments, loads config, starts the IPC server.

use std::sync::{atomic::AtomicBool, Arc};

use clap::Parser;
use signal_hook::consts::{SIGINT, SIGTERM};
use svd_daemon::{
    config::{load_config, Config},
    error::DaemonError,
    handler::RealHandler,
    ipc::{run_server, RequestHandler, ServerError},
    sleep::spawn_sleep_handler,
    strategy::{kwin::KWinStrategy, DisplayStrategy},
};

/// Sunshine Virtual Display daemon (privileged)
#[derive(Parser, Debug)]
#[command(name = "svd-daemon", version, about, long_about = None)]
struct Args {
    /// Enable verbose logging (wired to tracing level in T2.3/config)
    #[arg(short, long)]
    verbose: bool,
}

const CONFIG_PATH: &str = "/etc/sunshine-vd/config.toml";
const SOCKET_PATH: &str = "/run/sunshine-vd/svd.sock";
const STATE_PATH: &str = "/var/lib/sunshine-vd/state.json";

fn run(config: &Config) -> Result<(), DaemonError> {
    let strategy = Arc::new(KWinStrategy::new(
        std::path::PathBuf::from(STATE_PATH),
        config.output_ready_timeout_secs,
        config.disable_outputs.clone(),
        config.device.clone(),
    ));

    // Attempt to restore state from a previous run (daemon restart).
    if let Err(e) = strategy.restore() {
        tracing::warn!(error = %e, "restore() failed on startup — starting fresh");
    }

    // Create the shutdown flag first so it can be shared with both the signal
    // handlers and the RealHandler (which propagates it to the crash watcher).
    let shutdown = Arc::new(AtomicBool::new(false));

    signal_hook::flag::register(SIGTERM, Arc::clone(&shutdown)).expect("signal registration");
    signal_hook::flag::register(SIGINT, Arc::clone(&shutdown)).expect("signal registration");

    // Clone the strategy Arc before it is moved into RealHandler so that the
    // sleep handler can share the same strategy instance.
    let sleep_strategy: Arc<dyn DisplayStrategy> = strategy.clone();

    let handler: Arc<dyn RequestHandler> = Arc::new(RealHandler::new(
        strategy,
        config.extra_allowed_modes.clone(),
        Arc::clone(&shutdown),
    ));

    // Spawn the sleep/wake D-Bus listener thread.  It holds a logind inhibitor
    // delay lock and disconnects the virtual display before system sleep.
    spawn_sleep_handler(sleep_strategy, Arc::clone(&shutdown));

    run_server(
        std::path::Path::new(SOCKET_PATH),
        handler,
        shutdown,
        std::time::Duration::from_secs(config.ipc_timeout_secs),
    )
    .map_err(|e| match e {
        ServerError::Bind { path, source } => DaemonError::Ipc(format!(
            "failed to bind socket at {}: {}",
            path.display(),
            source
        )),
        ServerError::Io(e) => DaemonError::Io(e),
        ServerError::Framing(e) => DaemonError::Ipc(e.to_string()),
        ServerError::UnsafeSocketPath { path } => DaemonError::Ipc(format!(
            "refusing to replace non-socket path at {}",
            path.display()
        )),
    })
}

fn main() {
    // Parse args first so that --help / --version exit 0 before any setup.
    let args = Args::parse();

    let config = match load_config(std::path::Path::new(CONFIG_PATH)) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("svd-daemon: {error}");
            std::process::exit(1);
        }
    };

    // Initialise structured tracing to stderr.
    // Priority: RUST_LOG env var > --verbose flag > default "info".
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        let level = if args.verbose {
            "debug"
        } else {
            config.log_level.as_str()
        };
        tracing_subscriber::EnvFilter::new(level)
    });

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .init();

    tracing::info!("svd-daemon starting");

    if let Err(e) = run(&config) {
        tracing::error!(error = %e, "svd-daemon failed");
        std::process::exit(1);
    }
}
