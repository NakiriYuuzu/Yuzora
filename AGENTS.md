# AGENTS.md

## Release platform support

- 從 v0.0.9 起，macOS App 僅支援 Apple Silicon（M 系列）。候選版與正式版使用 `aarch64-apple-darwin`，不得重新加入 Intel／universal App 安裝包或 `darwin-x86_64` updater entry。
- macOS 固定下載檔名為 `Yuzora-macos-aarch64.dmg`；更新發布流程時須同步核對官網、README、`docs/operations.md` 與 updater metadata。
- 遠端 Host 保留 `macos-x86_64` 與 `macos-aarch64`；App 停止 Intel 支援不代表移除既有 Intel macOS 遠端工作區。
- 已發布的舊版 Intel／universal artifacts 保持不可變，不重寫舊版 release 或 tag。

## UI components — shadcn first

All UI components **must** prefer [shadcn/ui](https://ui.shadcn.com) (this project: `radix-nova`, alias `@/components/ui`).

Rules:

1. Before writing a custom component, check the shadcn registry / docs (`bunx --bun shadcn@latest search`, `docs <name>`).
2. If shadcn provides the component, install or reuse it from `src/components/ui/` and compose it. Do **not** reimplement buttons, dialogs, menus, scroll areas, tabs, inputs, etc.
3. Custom UI is allowed **only** when the shadcn registry has no suitable component (or the need is domain-specific chrome that is not a generic primitive). Document why in the PR / change notes when inventing a new primitive.
4. App-owned scrollable surfaces use `ScrollArea` from `@/components/ui/scroll-area`. Do not use bare `overflow-auto` / `overflow-y-auto` / `overflow-x-auto` as the scrollbar UI for content lists and reading panes.
5. Exceptions that keep native / library overflow (not ScrollArea): structural `overflow-hidden` clipping, text truncate, native `<textarea>`, CodeMirror `.cm-scroller` / merge hosts, xterm viewports, iframe / native child webview documents, and intentional hidden-overflow chrome (e.g. tab strips with keyboard-only overflow navigation).

## Agent skills

### Issue tracker

工作項目與 PRD 追蹤於 GitHub `NakiriYuuzu/Yuzora` Issues。見 `docs/agents/issue-tracker.md`。

### Triage labels

使用五個 canonical triage roles：`needs-triage`、`needs-info`、`ready-for-agent`、`ready-for-human`、`wontfix`。見 `docs/agents/triage-labels.md`。

### Domain docs

本 repo 採 single-context domain layout，以 `.yuuzu/CONTEXT.html` 與 `.yuuzu/adr/` 為權威來源。見 `docs/agents/domain.md`。
