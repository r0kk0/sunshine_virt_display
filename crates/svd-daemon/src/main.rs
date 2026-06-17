use clap::Parser;

/// Sunshine Virtual Display daemon (privileged)
#[derive(Parser, Debug)]
#[command(name = "svd-daemon", version, about, long_about = None)]
struct Args {
    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,
}

fn main() {
    let _args = Args::parse();
    eprintln!("svd-daemon: not implemented yet");
    std::process::exit(1);
}
