# ADR 002: Use snake_case tool names (e.g. `read_file`, `write_file`)

- **Status**: Accepted
- **Date**: 2026-05-12

## Context

Tool names must match between `src/tools.zig` and what the LLM recognizes. We evaluated three conventions:

- `readFile` (camelCase)
- `read_file` (snake_case)
- `file_read` (prefix-style)

A perplexity evaluation (`scripts/eval-tool-naming.js`) against our default model (`unsloth/Qwen3.6-35B-A3B-GGUF:UD-Q8_K_XL`) varied only the tool name (and short seq of tokens that come after).

## Decision

Use **snake_case** for all tool names: `read_file`, `write_file`, `edit_file`, `run_command`, `scrape`, `hacker_news`, `reddit`, `update_todos`, `load_skill`.

Prefix-style (`file_read`) had the highest perplexity and was ruled out. CamelCase and snake_case were negligible in perplexity, but snake_case was chosen for:

1. **Industry Alignment**: Major LLM providers use snake_case in their tool APIs.
2. **Training Data**: Models are likely trained on more snake_case tool definitions.
3. **Readability**: More readable for multi-word identifiers.

## Consequences

- **Positive**: Aligns with dominant API conventions; more readable.
- **Negative**: Nothing.