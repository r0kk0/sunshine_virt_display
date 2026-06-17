use clap::{Parser, Subcommand};

/// Sunshine Virtual Display CLI
#[derive(Parser, Debug)]
#[command(name = "svd", version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Show current display status
    Status,
    /// Add a virtual display
    Add,
    /// Remove a virtual display
    Remove,
}

fn main() {
    let _args = Args::parse();
    eprintln!("svd: not implemented yet");
}
