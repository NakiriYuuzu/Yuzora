# AGENTS.md

## beta.2 架構（`app/`）

v0.1.0-beta.2 以 [herdr-app](https://github.com/herdrdev/herdr-app)（Rust＋GPUI＋libghostty＋QuickJS mods）為基底，用 `git subtree` 放在 `app/`。`src-tauri/` 與 `src/`（React）是 v0.1.0-beta.1 以前的 Tauri app，beta.2 期間只當移植來源，不刪也不當設計依據。

- 架構決策（ADR）：`docs/beta2/architecture.md`。lane 範圍、目錄所有權、共用檔規則與流程：`docs/beta2/lanes.md`。研究資料：`docs/beta2/research/`。
- `app/` 是獨立的 Cargo workspace（root `Cargo.toml` 將它 exclude），有自己的 `Cargo.lock` 與 target（`app/target`）。版本 `0.1.0-beta.2`，app 與所有 `yuzora-*` crate 共用。
- 對上游檔只做最少改動，並以 `// yuzora:`（lane 用 `// yuzora(L<n>):`）註記。能新增檔案就不改上游檔。
- 上游同步：`git subtree pull --prefix=app https://github.com/herdrdev/herdr-app.git master --squash`。L0 PR 與上游同步 PR 必須用 merge commit 合併，否則會弄丟 subtree metadata。
- `app/AGENTS.md` 是上游給 herdr-app 開發者的指南（建置、測試、隔離執行），在 `app/` 內工作時一併遵守；它的 commit 規則（例如不加 AI co-author）只適用於向上游貢獻。`app/defaults/AGENTS.md` 是寫進使用者 app home、給使用者 agent 讀的操作指南。

### 目錄地圖

| 路徑 | 內容 |
| --- | --- |
| `app/src/` | herdr-app 殼（上游）。`app/src/yuzora/mod.rs` 是 Yuzora 唯一整合點：品牌常數、外連開關、掛上所有 `yuzora:*` HostModule |
| `app/crates/yuzora-core/` | 共用層：app home、`hc exec --on` 遠端執行、keyring、i18n、Yuzora 設定、mod 探索、design tokens |
| `app/crates/yuzora-{shell,git,db,ssh,files,settings}/` | L1–L6 的原生層；各自提供 `host_module()`（JS `import … from "yuzora:<name>"`） |
| `app/defaults/mods/yuzora-*` | Yuzora 的 JS mods（側欄模式、模式列、狀態列、tool panes）；i18n 在各 mod 的 `i18n/{en,zh-TW}.json` |
| `app/defaults/layouts/yuzora.json` | 新安裝的預設 layout（248px 側欄＋5 模式 stack＋右側 tab 區與狀態列） |
| `app/defaults/themes/yuzora-dark.json`、`app/defaults/yuzora/tokens.json` | theme 色名與 theme 沒有的設計 tokens |
| `app/vendor/` | 上游 vendored crates、ghostty submodule、Yuzora 的 russh／russh-sftp／tiberius patches |
| `app/scripts/yuzora/` | `check-mods`（在複本上跑 `herdr-app check`）、`bundle-macos`（包成 `Yuzora.app`） |
| `.github/workflows/app-ci.yml` | app 的 CI（base 為 `beta2/integration` 的 PR，Linux＋macOS） |

### 建置與測試

```bash
export PATH="$HOME/.local/zig-0.16.0:$PATH"   # ghostty 需要 Zig 0.16.0
git submodule update --init app/vendor/ghostty
cd app
cargo test --locked                # app、ghostty-sys 與全部 yuzora-* crate
cargo test --locked -p yuzora-git  # 單一 crate
scripts/yuzora/check-mods          # 每個 yuzora-* mod 的 herdr-app check
```

- Rust 1.95 以上（上游 CI 用 1.95.0）。
- 不要直接對 `app/defaults/mods/` 執行 `herdr-app check`：它會寫入 `gpui-kit.d.ts`、`jsconfig.json`，`build.rs` 會把它們打包進 app。
- `app/Cargo.lock` 有變動就要重產 `app/THIRD-PARTY-NOTICES`（cargo-about 0.9.3，`bun scripts/notices.ts`），否則 `tests/notices.rs` 失敗。
- 實機執行一律用暫存 `YUZORA_APP_HOME` 與隔離的 `HC_SESSION`／`XDG_STATE_HOME`，並先清掉所有 `HERDR_*` 環境變數，不得碰使用者真實的 Herdr、herdr-core 或 Yuzora 資料。完整指令見 `docs/beta2/lanes.md`。
- 要用 computer-use 或瀏覽器自動化做 GUI 驗收前，先取得使用者明確同意；優先用 `herdr-app status`／`shot`／`check` 留證。

### Lane 段落

各 lane 只改自己的標記區塊（規則見 `docs/beta2/lanes.md`）。

<!-- yuzora:lane:L1:start -->
<!-- yuzora:lane:L1:end -->

<!-- yuzora:lane:L2:start -->
<!-- yuzora:lane:L2:end -->

<!-- yuzora:lane:L3:start -->
<!-- yuzora:lane:L3:end -->

<!-- yuzora:lane:L4:start -->
<!-- yuzora:lane:L4:end -->

<!-- yuzora:lane:L5:start -->
<!-- yuzora:lane:L5:end -->

<!-- yuzora:lane:L6:start -->
#### L6 設定＋MOD／Template

- crate `app/crates/yuzora-settings`（`yuzora:settings`）：`templates`（與 hc 0.1.13 對齊的模板解析、檢查、寫入、遠端 sh 腳本）、`launch`（`hc run` 參數與 label）、`mods`（mods／layouts 分類、還原、從資料夾安裝）、`trash`（各平台垃圾桶）。
- mods：`yuzora-settings-general`、`-templates`、`-mods`（設定頁）與 `-launch`（「從模板啟動」對話框，`place: none`）。
- 上游改動只有 `app/defaults/frame/settings/main.js` 帶 `// yuzora(L6):` 的行；邏輯在新檔 `frame/settings/yuzora.js`，字串在 `frame/settings/i18n/`。
- 測試：`cargo test --locked -p yuzora-settings`；mods 用 `scripts/yuzora/check-mods yuzora-settings-general yuzora-settings-templates yuzora-settings-launch yuzora-settings-mods`；frame 由 `app/src/mods/render_tests.rs` 守，測試環境沒有任何 Yuzora 設定頁，frame 在 `settingsPages()` 為空時也要照常畫出。
- 測遠端模板腳本時不得碰到真正的垃圾桶：測試的 PATH 只放 cat／mkdir／mv／rm／date。
- 設計與決策：`docs/beta2/settings.md`。
<!-- yuzora:lane:L6:end -->

<!-- yuzora:lane:L7:start -->
<!-- yuzora:lane:L7:end -->

## Release platform support

- 從 v0.0.9 起，macOS App 僅支援 Apple Silicon（M 系列）。候選版與正式版使用 `aarch64-apple-darwin`，不得重新加入 Intel／universal App 安裝包或 `darwin-x86_64` updater entry。
- macOS 固定下載檔名為 `Yuzora-macos-aarch64.dmg`；更新發布流程時須同步核對官網、README、`docs/operations.md` 與 updater metadata。
- 遠端 Host 保留 `macos-x86_64` 與 `macos-aarch64`；App 停止 Intel 支援不代表移除既有 Intel macOS 遠端工作區。
- 已發布的舊版 Intel／universal artifacts 保持不可變，不重寫舊版 release 或 tag。
- macOS 發布不使用 Apple Developer ID 簽章／公證；Stable 仍須 Tauri updater 簽章。正式 build 不可使用會一併停用 updater 簽章的 `--no-sign`；README 與 release notes 須說明 Gatekeeper 提示。

## UI components — shadcn first（Tauri／React app）

本節只適用 `src/` 的 React UI；beta.2 的 `app/` 用 GPUI 與 herdr mods，沒有 shadcn。

All UI components **must** prefer [shadcn/ui](https://ui.shadcn.com) (this project: `radix-nova`, alias `@/components/ui`).

Rules:

1. Before writing a custom component, check the shadcn registry / docs (`bunx --bun shadcn@latest search`, `docs <name>`).
2. If shadcn provides the component, install or reuse it from `src/components/ui/` and compose it. Do **not** reimplement buttons, dialogs, menus, scroll areas, tabs, inputs, etc.
3. Custom UI is allowed **only** when the shadcn registry has no suitable component (or the need is domain-specific chrome that is not a generic primitive). Document why in the PR / change notes when inventing a new primitive.
4. App-owned scrollable surfaces use `ScrollArea` from `@/components/ui/scroll-area`. Do not use bare `overflow-auto` / `overflow-y-auto` / `overflow-x-auto` as the scrollbar UI for content lists and reading panes.
5. Exceptions that keep native / library overflow (not ScrollArea): structural `overflow-hidden` clipping, text truncate, native `<textarea>`, CodeMirror `.cm-scroller` / merge hosts, xterm viewports, iframe / native child webview documents, and intentional hidden-overflow chrome (e.g. tab strips with keyboard-only overflow navigation).

## Rust 建置目錄重用

repo 有兩個 Cargo workspace，各自一份 `Cargo.lock` 與 target：

- 根目錄 workspace：`src-tauri`（app）、`src-tauri/host`（host）與 `src-tauri/companion`（gateway），共用根 `.cargo/config.toml` 設定的 `src-tauri/target`（`companion:prepare` 另用其下的 `src-tauri/target/companion`）。
- `app/` workspace（beta.2）：`app/.cargo/config.toml` 設定的 `app/target`。

一份完整的 target 從數 GB 到上百 GB 都有可能。過去每條 lane、每輪實驗都各開一個 `CARGO_TARGET_DIR`，曾經累積超過 450 GB，把磁碟塞滿。

- 主 checkout（`yuzora/`）使用上述預設 target，不另外設定 `CARGO_TARGET_DIR`。
- 不在主 checkout 的建置，包括 `../.yuzora-lanes/<lane>` worktree、`output/` 底下的原始碼快照，一律重用固定的 slot。不得以 lane 或 experiment 名稱建立新的 target：

  ```bash
  LANES="$(dirname "$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")")/.yuzora-lanes"
  export CARGO_TARGET_DIR="$LANES/_targets/slot-1-app"   # workspace 的所有 crate 共用這個 slot，不再依 crate 分 slot
  ```

- slot 平時只有 `slot-1`、`slot-2` 兩組。cargo 會對 target 目錄上鎖，出現 `Blocking waiting for file lock on build directory` 代表這個 slot 正在使用中，請改用 `slot-2`。兩組都在使用中就等待，不要建立 `slot-3` 或 lane 專屬的 target。
- **beta.2 並行期間暫准 `slot-1`～`slot-4`**，依 lane 固定分配：L1＋L5 → `slot-1`、L2＋L3 → `slot-2`、L4＋L6 → `slot-3`、L7 → `slot-4`。遇到鎖就等待，不要再建新的 slot。beta.2 結束後回到兩組。
- 不同 worktree 共用同一個 slot 時，第三方依賴可以直接重用，只有 yuzora 自己的 crate 會重新編譯。slot 裡的執行檔會被下一個使用者覆寫，量測或交付需要的 binary 請先複製出去。
- 效能或評估迴圈不要每一輪都把整份 repo 複製到 `output/`，請改用 lane worktree 或 patch。必要的快照在量測完成後刪除，只保留報告與數據。
- 開始 Rust build 前，如果 `df -h` 顯示可用空間低於 50 GB，先停下來向使用者回報，不要自行刪除其他 lane 或實驗的資料。

## Agent skills

### Issue tracker

工作項目與 PRD 追蹤於 GitHub `NakiriYuuzu/Yuzora` Issues。見 `docs/agents/issue-tracker.md`。

### Triage labels

使用五個 canonical triage roles：`needs-triage`、`needs-info`、`ready-for-agent`、`ready-for-human`、`wontfix`。見 `docs/agents/triage-labels.md`。

### Domain docs

本 repo 採 single-context domain layout，以 `.yuuzu/CONTEXT.html` 與 `.yuuzu/adr/` 為權威來源。見 `docs/agents/domain.md`。beta.2 的架構決策以 `docs/beta2/architecture.md` 為準。
