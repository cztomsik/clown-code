# ADR 003: Extending `edit_file` with `line_range` and sed-style edits

- **Status**: Rejected
- **Date**: 2026-05-13
- **Context**: Token efficiency in file editing operations
- **Decision**: Do not implement alternative editing tools

## Context

The `edit_file` tool requires exact `old_content`/`new_content` strings, which becomes token-heavy for large files.

## Exploration

We tried several approaches with Qwen: `edit_file` with `line_range` params, a dedicated `sed` tool, and API variations (optional params, `mode` switch, Zig tagged unions).

## Observation

The model consistently ignored the new tools and reverted to exact string matching, only using the new tools when explicitly prompted. This indicates a strong bias toward the `old_content`/`new_content` pattern in the model's training.

## Decision

Do not implement `line_range` edits or a `sed` tool. `edit_file` will continue using exact `old_content`/`new_content`.
