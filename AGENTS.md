# Clown-Code Project Context

## Available System Tools

The following tools are available in the environment:

- **node** — v24.14.1 (`/Users/cztomsik/.nvm/versions/node/v24.14.1/bin/node`)
- **python3** — `/usr/bin/python3` (use `python3 -c 'print(2+2)'` for quick computations)
- **uv** — `/Users/cztomsik/.local/bin/uv` (Python package manager)
- **rg** (ripgrep) — `/opt/homebrew/bin/rg` (use `rg -o '^\s*(def|class|struct|function|fn)\s+\w+' .` for quick code navigation)
- **jq** — `/opt/homebrew/bin/jq` (JSON processing)
- **curl** — `/usr/bin/curl` (HTTP requests)
- **cargo** — Rust toolchain (required for building)

## Project Overview

**Clown-Code** is a local, terminal-based AI coding assistant written in **Rust**, using **ratatui** + **crossterm**. It connects to a local LLM (via llama.cpp) and provides the AI with tools to interact with the filesystem and shell. It is a self-hosted, privacy-friendly alternative to tools like Codex or Gemini-CLI. Session files and the OpenAI-compatible wire format are a stable contract: field names and JSON key order must not change, so existing `session-*.json` files stay loadable.

- **Tech stack**: Rust (edition 2021), ratatui 0.30, crossterm 0.28, reqwest (blocking, rustls), serde/serde_json (with `preserve_order`)
- **Build command**: `cargo build --release`
- **Tests**: `cargo test` — all tests are inline `#[cfg(test)]` unit tests within the source files (no `tests/` directory).
- **Output**: `target/release/clown_code`
- **AI server**: defaults to `http://127.0.0.1:8080`; override base URL with the `CLOWN_API` env var (see `Config::from_env` in `src/config.rs`). README documents a llama.cpp preset.

## Source Structure

| File | Purpose |
|------|---------|
| `src/main.rs` | Application entry point. Loads `Config`, supports `--print-prompt` (prints the assembled system prompt), then runs the TUI. |
| `src/tui.rs` | Terminal UI (ratatui). Event loop (render, key input, idle), fixed clown banner header, message display, footer (status row + input box), command handling (`/help`, `/exit`, `/quit`, `/stop`, `/clear`, `/clear-tools`, `/compact`, `/init`, `/retry`, `/retry-turn`, `/undo`, `/models`, `/model [name]`, `/save`, `/load <file>`, `/continue`; the argument is the entire rest of the line, so filenames with spaces work), and scrollback. |
| `src/input.rs` | The multi-line input buffer (`Input`): byte-offset cursor, codepoint-unit editing, bracketed paste. |
| `src/model.rs` | Core `Clown` struct — manages the AI agent lifecycle. Agent loop (`Agent`, which owns the client, the selected model, toolbox, and message history), system prompt loading, conversation management, snapshot save/load (JSON), and the worker thread (mpsc-based). The model is auto-discovered at startup (first id from the server's model list, fallback `"default"`); change it with `/model <name>`. Ctrl-C handling: stop the worker; double Ctrl-C exits. |
| `src/tools.rs` | All AI agent tools: `read_file`, `write_file`, `edit_file`, `run_command`, `load_skill`. Each tool has typed args and a hand-written JSON schema + description (surfaced to the model) in its `*_entry()` constructor. The built-in `/init` skill is `BUILTIN_INIT` (`src/skills/init.md`). Tool names are snake_case. |
| `src/llm.rs` | The LLM layer — OpenAI-compatible chat types, blocking HTTP client (`reqwest`, including a short-timeout `list_models()` for `/v1/models` discovery), and the tool registry/`Toolbox`. Wire format is a stable contract: only `null` optionals are omitted (`skip_serializing_if`) and field order is preserved. |
| `src/prompt.rs` | System prompt assembly: `PREFIX.md` (embedded at compile time) + `AGENTS.md`/`CLOWN.md` (loaded at runtime from cwd), plus today's date and the resolved cwd. |
| `src/config.rs` | `Config::from_env()` — base URL default and `CLOWN_API` override. |
| `src/PREFIX.md` | Base system prompt with guidelines. Embedded at compile time. |
| `src/skills/init.md` | Built-in `/init` skill — instructs the AI to explore the project and create an AGENTS.md file with system-specific context. |

## Architecture Notes

- **TUI Framework**: ratatui + crossterm.
- **AI Communication**: The agent loop runs in a worker thread and reports back over an mpsc channel as incremental `WorkerMsg`s (each new message as it is created, token count, or error), so the TUI can interrupt an in-flight LLM call. The parent's message history is appended to as messages arrive.
- **System Prompt**: Composed from `PREFIX.md` (embedded at compile time) + `AGENTS.md` (project context loaded at runtime from cwd), plus today's date and current working directory.
- **Conversation Persistence**: Snapshots (messages, total tokens) can be saved to `session-YYYY-MM-DD HH:MM:SS UTC.json` and reloaded. The `/continue` command auto-loads the most recent session. JSON key order is preserved (part of the session-file contract).
- **Skills System**: `load_skill` tool loads `.md` files from the `skills/` directory (or built-in skills like `init`), injecting their contents as system instructions.
- **CI**: `.github/workflows/ci.yml` runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` on push/PR.
