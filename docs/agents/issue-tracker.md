# Issue tracker: GitHub

Issues and PRDs for this repo live in GitHub Issues under `NakiriYuuzu/Yuzora`. Use the `gh` CLI or the connected GitHub tools for operations.

Implementation lifecycle（Issue → Branch → Commit → Pull Request → Merge）見 [`pull-request-workflow.md`](pull-request-workflow.md)；Release、Updater 與 Pages 操作見 [`../operations.md`](../operations.md)。

## Conventions

- Create a new issue only when no existing issue represents the work.
- When the current conversation maps to an existing issue, update that issue rather than creating a duplicate.
- Read an issue together with its labels and comments before changing it.
- Publish a completed spec as the issue body and apply the canonical triage label from `triage-labels.md`.
- Use issue comments for execution updates or follow-up evidence; keep the issue body as the current specification.

## Pull requests as a triage surface

**PRs as a request surface: no.** Pull requests are implementation artifacts, not feature-request intake for the engineering skills.

## When a skill says "publish to the issue tracker"

Create a GitHub issue unless the current work already has a matching issue. If it does, update the existing issue in place.

## When a skill says "fetch the relevant ticket"

Fetch the GitHub issue body, labels, and comments before relying on local planning artifacts.
