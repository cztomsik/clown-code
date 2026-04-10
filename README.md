# Clown-Code: AI Coding Assistant

A lightweight AI coding assistant built with tokamak.

```bash
# Start llama.cpp server
# llama-server -hf ggml-org/gemma-4-31B-it-GGUF:Q8_0 --spec-type ngram-cache --offline
# llama-server -hf unsloth/Qwen3.6-35B-A3B-GGUF:UD-Q4_K_XL --temp 0.6 --top-p 0.95 --top-k 20 --min-p 0.00 --chat-template-kwargs '{"preserve_thinking": true}' --spec-type ngram-cache --offline
# llama-server -m ~/Downloads/models/gemma-4-E4B-it-IQ4_NL.gguf --temp 1.0 --top-p 0.95 --top-k 64 --spec-type ngram-cache

zig build run
```

## Getting started

1. Clone this repo, build with `zig build` and put the `zig-out/bin/clown-code` somewhere on the PATH
2. Switch to your project's directory
3. Run `clown-code`
4. Type in `/init` and send with Enter
5. Inspect your newly created CLOWN.md and change whatever you want to be in your system prompt.

## Available tools

- **read_file** - Read file contents
- **write_file** - Create or overwrite file
- **edit_file** - Replace content in existing file
- **run_command** - Exec shell command

See `src/tools.zig` for more.
