// T4.9 — CLI transport: send request to daemon over Unix socket
//
// Design notes:
//   - clap is responsible only for type-parsing and required-ness.
//   - All semantic validation (numeric bounds, mode allowlist, device pattern)
//     is delegated to svd_proto::validate_request, which is the sole gate
//     that exits 1.  This keeps clap's exit code (2 for usage errors) distinct
//     from a validation failure (1), enabling callers to distinguish the two.
//   - The `--json` global flag selects machine-readable JSON response output.
//   - `--dry-run` on Connect skips socket I/O and prints "Dry run OK".
//   - Transport uses svd_proto::framing (newline-delimited JSON, max 4096 bytes).

use clap::{Parser, Subcommand};
use svd_proto::{validate_request, Request, Response};

const SOCKET_PATH: &str = "/run/sunshine-vd/svd.sock";

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
        /// Horizontal resolution in pixels. Must be part of an allowed mode (VIC table or
        /// extra_allowed_modes in config). Sanity range: 1–16384.
        #[arg(long)]
        width: u32,

        /// Vertical resolution in pixels. Must be part of an allowed mode (VIC table or
        /// extra_allowed_modes in config). Sanity range: 1–16384.
        #[arg(long)]
        height: u32,

        /// Refresh rate in Hz. Must be part of an allowed mode (VIC table or
        /// extra_allowed_modes in config). Sanity range: 24–480.
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
// Human-readable response printer
// ──────────────────────────────────────────────────────────────────────────────

/// Print a human-readable representation of `resp` to stdout/stderr.
/// Returns true if the response indicates success (for exit code determination).
fn print_human(resp: &Response) -> bool {
    match resp {
        Response::Connect { ok: true, connector, mode, .. } => {
            let c = connector.as_deref().unwrap_or("?");
            let m = mode.as_deref().unwrap_or("?");
            println!("virtual display connected: {c} {m}");
            true
        }
        Response::Connect { ok: false, error, .. } => {
            let e = error.as_deref().unwrap_or("unknown error");
            eprintln!("error: {e}");
            false
        }
        Response::Disconnect { ok: true, .. } => {
            println!("virtual display disconnected");
            true
        }
        Response::Disconnect { ok: false, error, .. } => {
            let e = error.as_deref().unwrap_or("unknown error");
            eprintln!("error: {e}");
            false
        }
        Response::Status { connected: true, card, connector, mode, .. } => {
            println!("connected: yes");
            println!("  card:      {}", card.as_deref().unwrap_or("?"));
            println!("  connector: {}", connector.as_deref().unwrap_or("?"));
            println!("  mode:      {}", mode.as_deref().unwrap_or("?"));
            true
        }
        Response::Status { connected: false, .. } => {
            println!("connected: no");
            true
        }
        Response::Restore { ok: true, .. } => {
            println!("restore ok");
            true
        }
        Response::Restore { ok: false, error, .. } => {
            let e = error.as_deref().unwrap_or("unknown error");
            eprintln!("error: {e}");
            false
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Entry point
// ──────────────────────────────────────────────────────────────────────────────

fn main() {
    let args = Args::parse();
    let json = args.json;
    let req = build_request(args.command);

    // Validate before any IPC send.
    if let Err(e) = validate_request(&req, &[]) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }

    // Dry-run: skip socket I/O and exit successfully.
    if let Request::Connect { dry_run: true, .. } = &req {
        println!("Dry run OK");
        std::process::exit(0);
    }

    // Open the Unix socket to the daemon.
    let mut stream = match std::os::unix::net::UnixStream::connect(SOCKET_PATH) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: daemon not running ({e})");
            std::process::exit(1);
        }
    };

    // Send the request as a newline-delimited JSON frame.
    if let Err(e) = svd_proto::framing::write_frame(&mut stream, &req) {
        eprintln!("error: failed to send request ({e})");
        std::process::exit(1);
    }

    // Read the response frame.
    let frame = match svd_proto::framing::read_frame(&mut stream) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error: failed to read response ({e})");
            std::process::exit(1);
        }
    };

    // Deserialize the response.
    let resp: Response = match serde_json::from_str(&frame) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: invalid response from daemon ({e})");
            std::process::exit(1);
        }
    };

    // Output and exit.
    if json {
        println!("{}", serde_json::to_string_pretty(&resp).unwrap());
        // Derive exit code from the ok/connected field.
        let ok = match &resp {
            Response::Connect { ok, .. } => *ok,
            Response::Disconnect { ok, .. } => *ok,
            Response::Status { .. } => true,
            Response::Restore { ok, .. } => *ok,
        };
        std::process::exit(if ok { 0 } else { 1 });
    } else {
        let ok = print_human(&resp);
        std::process::exit(if ok { 0 } else { 1 });
    }
}
