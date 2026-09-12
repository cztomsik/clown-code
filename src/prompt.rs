//! System prompt assembly — port of `Clown.loadSystemPrompt()`.
//!
//! `PREFIX.md` (embedded) + `AGENTS.md`/`CLOWN.md` (cwd, up to 1MB)
//! + today's date + resolved cwd path.

use std::io::Read;

pub const PREFIX: &str = include_str!("PREFIX.md");

const MAX_PROJECT_CONTEXT: usize = 1024 * 1024;

pub fn load_system_prompt() -> Result<String, String> {
    // AGENTS.md, falling back to CLOWN.md, else empty.
    let project_context = ["AGENTS.md", "CLOWN.md"]
        .iter()
        .find_map(|name| {
            let file = std::fs::File::open(name).ok()?;
            let mut bytes = Vec::new();
            file.take(MAX_PROJECT_CONTEXT as u64)
                .read_to_end(&mut bytes)
                .ok()?;
            Some(String::from_utf8_lossy(&bytes).into_owned())
        })
        .unwrap_or_default();

    let today = chrono::Local::now().format("%Y-%m-%d");
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;

    Ok(format!(
        "{PREFIX}{}{}\n\nCurrent date: {today}\nCurrent working directory: {}\n",
        if project_context.is_empty() {
            ""
        } else {
            "\n\n"
        },
        project_context,
        cwd.display(),
    ))
}
