mod error;

use clap::Parser;
use error::DaemonError;

/// Sunshine Virtual Display daemon (privileged)
#[derive(Parser, Debug)]
#[command(name = "svd-daemon", version, about, long_about = None)]
struct Args {
    /// Enable verbose logging (wired to tracing level in T2.3/config)
    #[arg(short, long)]
    verbose: bool,
}

fn run(_args: &Args) -> Result<(), DaemonError> {
    Err(DaemonError::Ipc("not implemented yet".into()))
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
