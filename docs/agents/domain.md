# Domain Docs

Yuzora uses a single domain context.

## Before exploring

- Read `.yuuzu/CONTEXT.html` for the domain glossary and current product boundaries.
- Read relevant decisions under `.yuuzu/adr/` before changing an affected subsystem.
- Follow the repository-wide architecture and verification rules in `CLAUDE.md`.

If no ADR covers the area, proceed without inventing a decision record. Create or revise an ADR only when the task explicitly resolves a durable architectural decision.

## Use the glossary vocabulary

Use the terms defined by `.yuuzu/CONTEXT.html` in issue titles, specifications, tests, UI copy, and implementation reports. In particular, keep `Terminal Session` distinct from `Agent Session`, and keep `Workspace canonical path` distinct from `Workspace display path`.

## Flag ADR conflicts

If planned work conflicts with an existing ADR, state the conflict explicitly and obtain approval before implementation. Do not silently override an accepted decision.
