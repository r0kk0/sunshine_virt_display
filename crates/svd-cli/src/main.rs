// T2.2 — CLI clap subcommands → proto::Request
//
// Design notes:
//   - clap is responsible only for type-parsing and required-ness.
//   - All semantic validation (numeric bounds, mode allowlist, device pattern)
//     is delegated to svd_proto::validate_request, which is the sole gate
//     that exits 1.  This keeps clap's exit code (2 for usage errors) distinct
//     from a validation failure (1), enabling callers to distinguish the two.
//   - The `--json` global flag is present for forward-compat (T3.2), where it
//     will select JSON-formatted response output.  In the stub it is inert
//     because we always print the request JSON.

use clap::{Parser, Subcommand};
use svd_proto::{validate_request, Request};

// ──────────────────────────────────────────────────────────────────────────────
// CLI types
// ──────────────────────────────────────────────────────────────────────────────

/// Sunshine Virtual Display CLI
#[derive(Parser, Debug)]
#[command(name = "svd", version, about, long_about = None)]
struct Args {
    /// Output responses as machine-readable JSON.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Connect a virtual display with the given mode.
    Connect {
        /// Horizontal resolution in pixels (1–16384).
        #[arg(long)]
        width: u32,

        /// Vertical resolution in pixels (1–16384).
        #[arg(long)]
        height: u32,

        /// Refresh rate in Hz (24–480).
        #[arg(long)]
        refresh: u32,

        /// DRM device to use, e.g. card0 (optional, must match ^card[0-9]+$).
        #[arg(long)]
        device: Option<String>,

        /// Validate and print the request without actually connecting.
        #[arg(long)]
        dry_run: bool,
    },

    /// Disconnect the virtual display.
    Disconnect,

    /// Show current display status.
    Status,

    /// Restore the display to its previous configuration.
    Restore,
}

// ──────────────────────────────────────────────────────────────────────────────
// Request builder
// ──────────────────────────────────────────────────────────────────────────────

fn build_request(cmd: Commands) -> Request {
    match cmd {
        Commands::Connect { width, height, refresh, device, dry_run } => {
            Request::Connect { width, height, refresh, device, dry_run }
        }
        Commands::Disconnect => Request::Disconnect {},
        Commands::Status => Request::Status {},
        Commands::Restore => Request::Restore {},
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Entry point
// ──────────────────────────────────────────────────────────────────────────────

fn main() {
    let args = Args::parse();
    let req = build_request(args.command);

    // Validate before any (future) IPC send.
    if let Err(e) = validate_request(&req, &[]) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }

    // Stub transport: print the request as JSON and exit 0.
    // In T3.2 this will be replaced by a real IPC send, and `--json` will
    // toggle whether the *response* is rendered as JSON or human-readable text.
    println!("{}", serde_json::to_string_pretty(&req).unwrap());
}
