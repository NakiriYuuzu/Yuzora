# AGENTS.md

## Release platform support

- 從 v0.0.9 起，macOS App 僅支援 Apple Silicon（M 系列）。候選版與正式版使用 `aarch64-apple-darwin`，不得重新加入 Intel／universal App 安裝包或 `darwin-x86_64` updater entry。
- macOS 固定下載檔名為 `Yuzora-macos-aarch64.dmg`；更新發布流程時須同步核對官網、README、`docs/operations.md` 與 updater metadata。
- 遠端 Host 保留 `macos-x86_64` 與 `macos-aarch64`；App 停止 Intel 支援不代表移除既有 Intel macOS 遠端工作區。
- 已發布的舊版 Intel／universal artifacts 保持不可變，不重寫舊版 release 或 tag。
- macOS 發布不使用 Apple Developer ID 簽章／公證；Stable 仍須 Tauri updater 簽章。正式 build 不可使用會一併停用 updater 簽章的 `--no-sign`；README 與 release notes 須說明 Gatekeeper 提示。

## UI components — shadcn first

All UI components **must** prefer [shadcn/ui](https://ui.shadcn.com) (this project: `radix-nova`, alias `@/components/ui`).

Rules:

1. Before writing a custom component, check the shadcn registry / docs (`bunx --bun shadcn@latest search`, `docs <name>`).
2. If shadcn provides the component, install or reuse it from `src/components/ui/` and compose it. Do **not** reimplement buttons, dialogs, menus, scroll areas, tabs, inputs, etc.
3. Custom UI is allowed **only** when the shadcn registry has no suitable component (or the need is domain-specific chrome that is not a generic primitive). Document why in the PR / change notes when inventing a new primitive.
4. App-owned scrollable surfaces use `ScrollArea` from `@/components/ui/scroll-area`. Do not use bare `overflow-auto` / `overflow-y-auto` / `overflow-x-auto` as the scrollbar UI for content lists and reading panes.
5. Exceptions that keep native / library overflow (not ScrollArea): structural `overflow-hidden` clipping, text truncate, native `<textarea>`, CodeMirror `.cm-scroller` / merge hosts, xterm viewports, iframe / native child webview documents, and intentional hidden-overflow chrome (e.g. tab strips with keyboard-only overflow navigation).

## Rust 建置目錄重用

`src-tauri`（app）、`src-tauri/host`（host）、`src-tauri/companion`（gateway）與 `crates/yuzora-core` 同屬根目錄的 Cargo workspace，共用一份 `Cargo.lock` 與一個 target（根 `.cargo/config.toml` 設為 `src-tauri/target`）。一份完整的 target 從數 GB 到上百 GB 都有可能。過去每條 lane、每輪實驗都各開一個 `CARGO_TARGET_DIR`，曾經累積超過 450 GB，把磁碟塞滿。

- 主 checkout（`yuzora/`）使用預設的 `target/`，不另外設定 `CARGO_TARGET_DIR`。
- 不在主 checkout 的建置，包括 `../.yuzora-lanes/<lane>` worktree、`output/` 底下的原始碼快照，一律重用固定的 slot。不得以 lane 或 experiment 名稱建立新的 target：

  ```bash
  LANES="$(dirname "$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")")/.yuzora-lanes"
  export CARGO_TARGET_DIR="$LANES/_targets/slot-1-app"   # workspace 的所有 crate 共用這個 slot，不再依 crate 分 slot
  ```

- slot 只有 `slot-1`、`slot-2` 兩組。cargo 會對 target 目錄上鎖，出現 `Blocking waiting for file lock on build directory` 代表這個 slot 正在使用中，請改用 `slot-2`。兩組都在使用中就等待，不要建立 `slot-3` 或 lane 專屬的 target。
- 不同 worktree 共用同一個 slot 時，第三方依賴可以直接重用，只有 yuzora 自己的 crate 會重新編譯。slot 裡的執行檔會被下一個使用者覆寫，量測或交付需要的 binary 請先複製出去。
- 效能或評估迴圈不要每一輪都把整份 repo 複製到 `output/`，請改用 lane worktree 或 patch。必要的快照在量測完成後刪除，只保留報告與數據。
- 開始 Rust build 前，如果 `df -h` 顯示可用空間低於 50 GB，先停下來向使用者回報，不要自行刪除其他 lane 或實驗的資料。

## Agent skills

### Issue tracker

工作項目與 PRD 追蹤於 GitHub `NakiriYuuzu/Yuzora` Issues。見 `docs/agents/issue-tracker.md`。

### Triage labels

使用五個 canonical triage roles：`needs-triage`、`needs-info`、`ready-for-agent`、`ready-for-human`、`wontfix`。見 `docs/agents/triage-labels.md`。

### Domain docs

本 repo 採 single-context domain layout，以 `.yuuzu/CONTEXT.html` 與 `.yuuzu/adr/` 為權威來源。見 `docs/agents/domain.md`。
