# ADR 001: Use Architecture Decision Records

- **Status**: Accepted
- **Date**: 2026-05-12

## Context

Clown-Code is a terminal-based AI coding assistant written in Zig. As the project grows, design and architectural choices will accumulate. Without a lightweight way to capture these decisions, their rationale is easily lost — leading to repeated debates, inconsistent patterns, and knowledge concentrated in a single person's head.

## Decision

We will use [Architecture Decision Records (ADRs)](https://martinfowler.com/bliki/ArchitectureDecisionRecord.html) to document significant architectural and design choices. Each ADR captures:

1. **Title & number** — short, descriptive, sequentially numbered.
2. **Status** — one of *proposed*, *accepted*, *deprecated*, or *superseded*.
3. **Context** — the forces and constraints at play.
4. **Decision** — what we actually decided.
5. **Consequences** — the resulting trade-offs and implications.

ADRs live in the `adr/` directory as Markdown files (`001-<title>.md`). They are version-controlled alongside the source code.

## Consequences

- **Positive**: Decisions are transparent and searchable. New contributors can understand *why* things are the way they are.
- **Positive**: Lightweight — no tooling overhead beyond Git.
- **Negative**: Requires discipline to keep ADRs up to date. Superseded decisions should be marked as such rather than deleted.
- **Negative**: ADRs are not a substitute for good code comments or documentation — they cover *architectural* decisions only.
