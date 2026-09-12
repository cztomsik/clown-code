//! System prompt assembly.
//!
//! `PREFIX.md` (embedded) + `AGENTS.md`/`CLOWN.md` (cwd)
//! + today's date + resolved cwd path.

pub const PREFIX: &str = include_str!("PREFIX.md");

pub fn load_system_prompt() -> Result<String, String> {
    // AGENTS.md, falling back to CLOWN.md, else empty.
    let project_context = ["AGENTS.md", "CLOWN.md"]
        .iter()
        .find_map(|name| std::fs::read_to_string(name).ok())
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
