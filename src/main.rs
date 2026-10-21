//! Entrypoint: initialises Tokio runtime, tracing (stderr), parses CLI
//! arguments, and dispatches the requested subcommand. All logging goes to
//! stderr because stdout carries the documentation output (so it can be
//! piped into other shell commands).

use clap::Parser;
use cratedoc::cli::{Cli, Engine};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let engine = Engine::new()?;

    match engine.run(cli.command).await {
        Ok(output) => {
            println!("{output}");
            Ok(())
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
