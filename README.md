# Clown-Code: AI Coding Assistant

A local, terminal-based AI coding assistant written in Zig, using the tokamak TUI framework.

Clown-Code connects to a local LLM (via llama.cpp) and provides the AI with tools to interact with your filesystem and shell — including file read/write/edit, todo management, shell commands, web scraping, and fetching from Hacker News and Reddit.

It's a self-hosted, privacy-friendly alternative to tools like Cursor or GitHub Copilot, running entirely locally with an open-source model.

## Quick Start

```bash
# Start llama.cpp server (pick a model)
# llama-server -hf unsloth/Qwen3.6-27B-GGUF:Q8_0 --mlock --spec-default --offline
# llama-server -hf ggml-org/gemma-4-31B-it-GGUF:Q8_0 --mlock --spec-default --offline
# llama-server -hf unsloth/Qwen3.6-35B-A3B-GGUF:UD-Q4_K_XL --mlock --temp 0.6 --top-p 0.95 --top-k 20 --min-p 0.00 --chat-template-kwargs '{"preserve_thinking": true}' --spec-default --offline

zig build run
```

## Getting started

> **Note**: This project requires **Zig v0.15.2**. Make sure you have this version installed before building.

1. Clone this repo, build with `zig build` and put the `zig-out/bin/clown-code` somewhere on the PATH
2. Switch to your project's directory
3. Run `clown-code`
4. Type in `/init` and send with Enter
5. Inspect your newly created CLOWN.md and change whatever you want to be in your system prompt.

## Available tools

- **todos_update** - Create or update todo item(s)
- **file_read** - Read the contents of a file
- **file_write** - Write content to a file, creating directories if needed
- **file_edit** - Edit a file by replacing specific content
- **run_command** - Execute a shell command and return its output
- **scrape** - Scrape a web page and convert it to markdown
- **hacker_news** - Get stories from Hacker News
- **reddit** - Get posts from a Reddit subreddit

See `src/tools.zig` for more.
