# Clown-Code Project Context

## Available System Tools

The following tools are available in the environment:

- **node** — v24.14.1 (`/Users/cztomsik/.nvm/versions/node/v24.14.1/bin/node`)
- **python3** — `/usr/bin/python3` (use `python3 -c 'print(2+2)'` for quick computations)
- **uv** — `/Users/cztomsik/.local/bin/uv` (Python package manager)
- **rg** (ripgrep) — `/opt/homebrew/bin/rg` (use `rg -o '^\s*(def|class|struct|function|fn)\s+\w+' .` for quick code navigation)
- **jq** — `/opt/homebrew/bin/jq` (JSON processing)
- **curl** — `/usr/bin/curl` (HTTP requests)
- **zig** — required for building (v0.15.2 minimum)

## Project Overview

**Clown-Code** is a local, terminal-based AI coding assistant written in **Zig**, using the **tokamak TUI framework**. It connects to a local LLM (via llama.cpp) and provides the AI with tools to interact with the filesystem and shell. It is a self-hosted, privacy-friendly alternative to tools like Codex or Gemini-CLI.

- **Tech stack**: Zig (v0.15.2+), tokamak TUI framework
- **Build command**: `zig build` / `zig build run`
- **Dependency**: tokamak (local path dependency at `../tokamak`)
- **Output**: `zig-out/bin/clown_code`

## Source Structure

| File | Purpose |
|------|---------|
| `src/main.zig` | Application entry point. Sets up the tokamak app with config, custom panic handler, debug logging, and tool registration. |
| `src/tui.zig` | Terminal UI implementation. Handles the event loop (render, key input, idle), message display, header/footer, command handling (`/exit`, `/clear`, `/compact`, `/init`, `/retry`, `/sudo`, `/save`, `/load`, `/continue`, `/help`, `/models`), and scrollback. |
| `src/model.zig` | Core Clown struct — manages the AI agent lifecycle. Handles system prompt loading (PREFIX.md + CLOWN.md), conversation management, snapshot save/load (JSON), worker process for AI inference (fork-based with pipe communication), todo tracking, and compaction. |
| `src/tools.zig` | All AI agent tools: `read_file`, `write_file`, `edit_file`, `run_command`, `scrape`, `hacker_news`, `reddit`, `update_todos`, `load_skill`, `advisor`. Each tool has typed args structs and docblock comments. Tools are registered via `registerAllTools()`. |
| `src/PREFIX.md` | Base system prompt with guidelines (be concise, explain actions, safety first, best practices for reading/writing/modifying code, running commands, multi-step tasks, skills, and using the advisor tool). |
| `src/panic.zig` | Custom panic handler. |
| `src/log.zig` | Debug logging function. |
| `src/skills/init.md` | Built-in `/init` skill — instructs the AI to explore the project and create a CLOWN.md file with system-specific context. |

## Architecture Notes

- **TUI Framework**: Built on tokamak, which provides a component-based TUI with a bundle system. Tools are registered via an init hook (`bundle.addInitHook(tools.registerAllTools)`).
- **AI Communication**: Uses a fork-based worker model. The main process forks a child that runs the AI agent loop (`agent.next()` → tool calls → `agent.acceptAll()`). Results are communicated back via a pipe as JSON messages (`WorkerMsg` union: `snapshot` or `err`).
- **System Prompt**: Composed from `PREFIX.md` (embedded at compile time) + `CLOWN.md` (project context loaded at runtime from cwd, up to 1MB), plus today's date and current working directory.
- **Conversation Persistence**: Snapshots (messages, todos, total tokens) can be saved to `session-YYYY-MM-DD HH:MM:SS UTC.json` and reloaded. The `/continue` command auto-loads the most recent session.
- **Skills System**: `load_skill` tool loads `.md` files from `skills/` directory (or built-in skills like `init`), injecting their contents as system instructions.
- **ADR Directory**: Architecture Decision Records in `adr/` (001-use-adr.md, 002-use-snake-case-tool-names.md, 003-edit-file-line-range.md).
