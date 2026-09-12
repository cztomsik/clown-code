# Clown-Code: AI Coding Assistant

> Let's be honest - this is a tool created **out of necessity**. Big boys are
> doing changes all the time and I'm tired of it. I want to 100% focus on my
> work and I need a reliable tool for that. I don't expect this to be useful for
> anybody else except me. I am also likely to reject any feature-requests,
> and/or pull requests for anything other than fixing bugs. You should fork
> this, make your own changes, and keep it for yourself. Have fun.


A local, terminal-based AI coding assistant written in Rust, built on
ratatui/crossterm.

Clown-Code connects to a local LLM (via llama.cpp) and provides the AI with
tools to interact with your filesystem and shell.

It's a self-hosted, privacy-friendly alternative to tools like Codex or
Gemini-CLI, running entirely locally with an open-source model.

## Quick Start

Run a llama.cpp server using a preset file (`llama-server --models-preset
~/llama.ini`):

```ini
[*]
jinja = 1
spec-default = 1

[default]
hf = unsloth/Qwen3.6-35B-A3B-GGUF:UD-Q4_K_XL
temp = 0.6
top-p = 0.95
top-k = 20
min-p = 0.00
chat-template-kwargs  = {"preserve_thinking": false}
reasoning-budget = 1000
reasoning-budget-message = ... Considering the limited time by the user, I have to give the solution based on the thinking directly now. </think>

[advisor]
hf = unsloth/Qwen3.6-27B-GGUF:UD-Q4_K_XL
temp = 0.4
top-p = 0.95
top-k = 20
min-p = 0.00
```

Then build & run the app:

```bash
cargo build --release
./target/release/clown_code
```

The base URL defaults to `http://127.0.0.1:8080`; override it with
`CLOWN_API` (include `/v1` for vllm or other servers).

## Test

```bash
cargo test                 # unit + integration (mock-server E2E)
cargo test -- --ignored    # also loads the real session files from this repo
```

## Getting started

1. Clone this repo, build with `cargo build --release` and put
   `target/release/clown_code` somewhere on the PATH
2. Switch to your project's directory
3. Run `clown_code`
4. Type in `/init` and send with Enter
5. Inspect your newly created AGENTS.md and change whatever you want to be in your system prompt.

## Usage

- type a message, Enter to send
- `/exit` `/quit` — leave
- `/stop` — stop the in-flight generation (Ctrl-C also works; double
  Ctrl-C exits)
- `/clear` — clear the conversation (keeps the system prompt)
- `/clear-tools` — drop all tool results
- `/compact` — summarize the conversation into a compact summary
- `/init` — ask the model to create/update `AGENTS.md`
- `/retry` — re-run the last user message
- `/undo` — put the last user message back into the input box
- `/save` — save the session to `session-<timestamp>.json`
- `/load <file>` — load a session
- `/continue` — load the most recent session
- Esc Esc — clear the input box
- mouse wheel — scrollback

## Available tools

See `src/tools.rs`.
