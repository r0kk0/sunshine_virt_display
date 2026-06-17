use clap::Parser;

/// Emergency display restore utility for sunshine-virt-display
#[derive(Parser, Debug)]
#[command(name = "svd-restore", version, about, long_about = None)]
struct Args {
    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,
}

fn main() {
    let _args = Args::parse();
    eprintln!("svd-restore: not implemented yet");
    std::process::exit(1);
}
