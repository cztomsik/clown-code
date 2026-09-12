//! AI agent tools: filesystem and shell.
//!
//! Tool names and behaviors are part of the prompt contract (and of
//! saved session files), so they must not change.

use std::io::Read;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::llm::ToolEntry;

pub const MAX_READ_SIZE: usize = 2 * 1024 * 1024;

/// Register all standard tools with a toolbox.
pub fn register_all_tools(toolbox: &mut crate::llm::Toolbox) {
    toolbox.add(read_file_entry());
    toolbox.add(write_file_entry());
    toolbox.add(edit_file_entry());
    toolbox.add(run_command_entry());
    toolbox.add(load_skill_entry());
}

/// Map `io::Error` to a short error name like "FileNotFound"
/// surfaced to the model.
fn io_err(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::NotFound => "FileNotFound".into(),
        std::io::ErrorKind::PermissionDenied => "AccessDenied".into(),
        _ => format!("IoError: {e}"),
    }
}

/// Read a file from the current working directory, up to `MAX_READ_SIZE`
/// bytes (truncated, not an error).
fn read_capped(path: &str) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path).map_err(|e| io_err(&e))?;
    let mut content = Vec::new();
    file.take(MAX_READ_SIZE as u64)
        .read_to_end(&mut content)
        .map_err(|e| io_err(&e))?;
    Ok(content)
}

fn validate_utf8(bytes: &[u8]) -> Result<&str, String> {
    std::str::from_utf8(bytes).map_err(|_| "InvalidUtf8".to_string())
}

// ---------------------------------------------------------------- read_file

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadFileArgs {
    path: String,
    #[serde(default)]
    raw: bool,
}

/// Read the contents of a file.
/// If `raw` is false (default), output is prefixed with line numbers (e.g., "1:content").
fn read_file(args: &Value) -> Result<String, String> {
    let args: ReadFileArgs = serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;

    let bytes = read_capped(&args.path)?;
    let contents = validate_utf8(&bytes)?;
    if args.raw {
        return Ok(contents.to_string());
    }

    // Split into lines and format as "N:content" (note: a trailing
    // newline yields a final empty line).
    let out = contents
        .split('\n')
        .enumerate()
        .map(|(i, line)| format!("{}:{}\n", i + 1, line))
        .collect();
    Ok(out)
}

pub fn read_file_entry() -> ToolEntry {
    ToolEntry {
        name: "read_file",
        description: "Read the contents of a file",
        parameters: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "raw": { "type": "boolean", "default": false }
            },
            "required": ["path"],
            "additionalProperties": false
        }),
        handler: read_file,
    }
}

// --------------------------------------------------------------- write_file

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteFileArgs {
    path: String,
    content: String,
}

/// Write content to a file, creating parent directories if needed.
fn write_file(args: &Value) -> Result<String, String> {
    let args: WriteFileArgs = serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;

    if let Some(dir) = std::path::Path::new(&args.path).parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).map_err(|e| io_err(&e))?;
        }
    }

    std::fs::write(&args.path, &args.content).map_err(|e| io_err(&e))?;
    Ok("File written successfully".into())
}

pub fn write_file_entry() -> ToolEntry {
    ToolEntry {
        name: "write_file",
        description: "Write content to a file, creating directories if needed",
        parameters: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "content": { "type": "string" }
            },
            "required": ["path", "content"],
            "additionalProperties": false
        }),
        handler: write_file,
    }
}

// ---------------------------------------------------------------- edit_file

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditFileArgs {
    path: String,
    old_content: String,
    new_content: String,
    #[serde(default)]
    replace_all: bool,
}

/// Edit a file by replacing specific content.
/// If replace_all is false (default), old_content must exist exactly once.
fn edit_file(args: &Value) -> Result<String, String> {
    let args: EditFileArgs = serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;

    let bytes = read_capped(&args.path)?;
    let content = validate_utf8(&bytes)?;
    let new_content = if args.replace_all {
        content.replace(&args.old_content, &args.new_content)
    } else {
        let pos = content
            .find(&args.old_content)
            .ok_or_else(|| "ContentNotFound".to_string())?;

        // Check that it only appears once
        if content[pos + args.old_content.len()..].contains(&args.old_content) {
            return Err("AmbiguousMatch".into());
        }

        let mut out = String::with_capacity(content.len() + args.new_content.len());
        out.push_str(&content[..pos]);
        out.push_str(&args.new_content);
        out.push_str(&content[pos + args.old_content.len()..]);
        out
    };

    std::fs::write(&args.path, new_content).map_err(|e| io_err(&e))?;
    Ok("File edited successfully".into())
}

pub fn edit_file_entry() -> ToolEntry {
    ToolEntry {
        name: "edit_file",
        description: "Edit a file by replacing specific content. Set replace_all=true to replace all occurrences",
        parameters: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "old_content": { "type": "string" },
                "new_content": { "type": "string" },
                "replace_all": { "type": "boolean", "default": false }
            },
            "required": ["path", "old_content", "new_content"],
            "additionalProperties": false
        }),
        handler: edit_file,
    }
}

// -------------------------------------------------------------- run_command

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunCommandArgs {
    command: String,
    #[serde(default)]
    cwd: Option<String>,
}

fn to_capped_str(bytes: &[u8]) -> Result<String, String> {
    // NOTE: stdout/stderr are validated as UTF-8 before checking the
    // exit code, so non-UTF-8 output errors out even on success.
    std::str::from_utf8(&bytes[..bytes.len().min(MAX_READ_SIZE)])
        .map(|s| s.to_string())
        .map_err(|_| "InvalidUtf8".to_string())
}

/// Execute a shell command and return its output.
/// Captures both stdout and stderr. TODO: timeout.
fn run_command(args: &Value) -> Result<String, String> {
    let args: RunCommandArgs = serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;

    let mut cmd = std::process::Command::new("sh");
    cmd.arg("-c").arg(&args.command);
    if let Some(cwd) = &args.cwd {
        cmd.current_dir(cwd);
    }

    let output = cmd.output().map_err(|e| e.to_string())?;

    let stdout = to_capped_str(&output.stdout)?;
    let stderr = to_capped_str(&output.stderr)?;

    let Some(exit_code) = output.status.code() else {
        // Process was killed by a signal.
        return Err("CommandFailed".into());
    };

    if exit_code != 0 {
        return Ok(format!(
            "Command failed with exit code {exit_code}\nStdout:\n{stdout}\nStderr:\n{stderr}"
        ));
    }

    if !stderr.is_empty() {
        return Ok(format!("{stdout}\n\nStderr:\n{stderr}"));
    }

    Ok(stdout)
}

pub fn run_command_entry() -> ToolEntry {
    ToolEntry {
        name: "run_command",
        description: "Execute a shell command and return its output",
        parameters: json!({
            "type": "object",
            "properties": {
                "command": { "type": "string" },
                "cwd": { "type": "string" }
            },
            "required": ["command"],
            "additionalProperties": false
        }),
        handler: run_command,
    }
}

// --------------------------------------------------------------- load_skill

/// Built-in skill, embedded at compile time.
pub const BUILTIN_INIT: &str = include_str!("skills/init.md");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoadSkillArgs {
    skill_name: String,
}

/// Load a skill file and inject its contents as system instructions into the agent's context.
/// Builtin skills (init) take precedence over user-provided skills.
fn load_skill(args: &Value) -> Result<String, String> {
    let args: LoadSkillArgs = serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;

    if args.skill_name == "init" {
        return Ok(BUILTIN_INIT.to_string());
    }

    // TODO: Check for path traversal.
    let file = std::fs::File::open(format!("skills/{name}.md", name = args.skill_name))
        .map_err(|e| io_err(&e))?;
    let mut content = Vec::new();
    file.take(MAX_READ_SIZE as u64)
        .read_to_end(&mut content)
        .map_err(|e| io_err(&e))?;
    String::from_utf8(content).map_err(|_| "InvalidUtf8".to_string())
}

pub fn load_skill_entry() -> ToolEntry {
    ToolEntry {
        name: "load_skill",
        description: "Load a set of specialized instructions (a skill) into the current context to improve performance on a specific task.",
        parameters: json!({
            "type": "object",
            "properties": {
                "skill_name": { "type": "string" }
            },
            "required": ["skill_name"],
            "additionalProperties": false
        }),
        handler: load_skill,
    }
}
