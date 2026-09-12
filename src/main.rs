//! Entry point.
//!
//! Sets up logging, loads `Config`, supports
//! `--print-prompt` (prints the assembled system prompt and exits),
//! and otherwise runs the TUI.

use clown_code::config::Config;
use clown_code::tui;

/// Append everything to `debug.log` in cwd.
fn init_logging() {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("debug.log")
        .expect("failed to open debug.log");
    let _ = tracing_subscriber::fmt()
        .with_writer(std::sync::Arc::new(file))
        .with_ansi(false)
        .with_target(true)
        .try_init();
}

fn main() -> anyhow::Result<()> {
    init_logging();

    let config = Config::from_env();
    tracing::debug!("base_url = {}", config.base_url);

    if std::env::args().any(|a| a == "--print-prompt") {
        print!("{}", clown_code::prompt::load_system_prompt()?);
        return Ok(());
    }

    tui::run(&config)
}
