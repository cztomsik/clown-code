//! Entry point.
//!
//! Sets up logging and the panic handler, loads `Config`, supports
//! `--print-prompt` (prints the assembled system prompt and exits),
//! and otherwise runs the TUI.

use std::io::Write;

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

/// Write the panic + backtrace to `error.log`.
fn set_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "Unknown panic".to_string()
        };

        let bt = std::backtrace::Backtrace::force_capture();
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open("error.log")
        {
            let _ = writeln!(file, "panic: {msg}");
            let _ = writeln!(file, "stack backtrace:");
            let _ = writeln!(file, "{bt}");
        }
        default(info);
    }));
}

fn main() -> std::io::Result<()> {
    set_panic_hook();
    init_logging();

    let config = Config::from_env();
    tracing::debug!("base_url = {}", config.base_url);

    if std::env::args().any(|a| a == "--print-prompt") {
        match clown_code::prompt::load_system_prompt() {
            Ok(prompt) => print!("{prompt}"),
            Err(e) => {
                eprintln!("failed to load system prompt: {e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    tui::run(&config)?;
    Ok(())
}
