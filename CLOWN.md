# Clown-Code: AI Coding Assistant

## Project Overview

Clown-Code is a local, terminal-based AI coding assistant written in **Zig**, using the **tokamak** TUI framework. It connects to a local LLM (via llama.cpp on `localhost:8080`) and provides the AI with tools to interact with the filesystem and shell — including file read/write/edit, todo management, shell commands, web scraping, and fetching from Hacker News and Reddit. It's a self-hosted, privacy-friendly alternative to Cursor or GitHub Copilot.

**Tech stack**: Zig 0.15.2+, tokamak (TUI + AI client + HTTP + DOM), llama.cpp (external, for the LLM backend)
**Build**: `zig build` (install to `zig-out/bin/clown-code`)
**Run**: `zig build run` (requires a running llama.cpp server on `localhost:8080`)

## Source Structure

| File | Purpose |
|------|---------|
| `src/main.zig` | Application entry point. Configures the tokamak app with `Config` and `App`, wires up the toolbox init hook, and launches the TUI. |
| `src/model.zig` | Core `Clown` struct — manages the AI agent lifecycle (init, send, retry, save, load, continue), background worker process for streaming agent responses, snapshot serialization (messages, todos, tokens) via a named pipe, and system prompt loading from `CLOWN.md`. |
| `src/tools.zig` | Tool implementations registered with the AI agent's toolbox: `read_file`, `write_file`, `edit_file`, `run_command`, `scrape`, `hacker_news`, `reddit`, `update_todos`, `load_skill`, `advisor`. Each tool uses tokamak's `AgentTool` for JSON schema generation and input validation. |
| `src/tui.zig` | Terminal UI implementation using tokamak. Renders a header (banner + collapsible todos), scrollable message area (user/assistant/tool messages with tool call details), and footer (user input + token count). Handles commands (`/clear`, `/init`, `/retry`, `/save`, `/load`, `/continue`, `/exit`, `/quit`, `/help`) and keyboard input. |
| `src/skills/init.md` | Skill definition for the `/init` command — instructs the AI to explore the project and create a `CLOWN.md` with project-specific context plus the canonical system prompt. |
| `src/skills/compact.md` | Skill definition for context compaction — summarize conversation history to reduce token usage. |

## Architecture Notes

- **Agent architecture**: `Clown` owns a `tk.ai.Agent` backed by the tokamak AI client (pointing to llama.cpp). The agent runs in a **forked child process** connected via a pipe — the child streams JSON snapshots of the agent state (messages, todos, token count) to the parent, which updates the TUI on each snapshot. This keeps the TUI responsive while the LLM is processing.
- **Tool registration**: All tools are registered via `tools.registerAllTools()`, called from an `App.configure` init hook. Tools use tokamak's `AgentTool` type which auto-generates JSON schemas for tool calling.
- **Skill loading**: `loadSkill` first checks for built-in skills (`init`, `compact` embedded at compile time via `@embedFile`), then falls back to user skills in `skills/<name>.md`. Path traversal is not yet guarded.
- **System prompt**: Loaded from `CLOWN.md` in the current working directory at startup. Falls back to the embedded `src/CLOWN.md` if not found. This is the key customization point — `/init` generates a project-aware version.
- **Session management**: Conversations can be saved to `session-<timestamp>.json` files and reloaded. The `/continue` command auto-loads the most recent session.
- **Todo management**: The `Clown` struct maintains an `ArrayList(TodoItem)` that is shared between the worker (reads/writes snapshots) and the TUI (displays in header). The `update_todos` tool merges items by name.
- **Dependencies**: Single external dependency — `tokamak` (path dependency `../tokamak`). Provides TUI, AI client, HTTP client, DOM parsing, HTML-to-markdown, Hacker News/Reddit extensions, JSON serialization, and agent tooling.
- **ADR**: Architecture decisions are documented in `adr/` using the Architecture Decision Record format.
- **Logging**: Custom debug logging via `src/log.zig` (writes to `debug.log`); panic handling via `src/panic.zig` (writes to `error.log`).
- **Worker model**: The `Clown.tick()` method polls the forked worker for pipe data and completed status, deserializing JSON snapshots line-by-line. The worker runs `agent.next()` in a loop, accepting tool call results and emitting snapshots.
