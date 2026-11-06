//! The `fuga` binary: server subcommands and headless commands.
//!
//! M0 only loads and validates the config; `index` (M1) and `server start` (M2) follow.

use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use fuga_core::config::{Config, Paths};

#[derive(Debug, Parser)]
#[command(name = "fuga", version, about = "Local-first Markdown notes")]
struct Cli {
    /// Path to config.toml (default: the platform config dir).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let paths = Paths::new()?;
    let config_path = cli.config.unwrap_or_else(|| paths.config_file());

    let config = if config_path.exists() {
        Config::load(&config_path).with_context(|| format!("loading {}", config_path.display()))?
    } else {
        Config::default()
    };

    println!(
        "fuga {}: {} source(s) configured",
        env!("CARGO_PKG_VERSION"),
        config.sources.len()
    );
    Ok(())
}
