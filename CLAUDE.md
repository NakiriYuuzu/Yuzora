# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Yuzora：Tauri 2 + React 19 桌面開發工作台——ACP agents、SSH/SFTP、資料庫、終端機、git 面板共用同一個 workspace。GitHub：`NakiriYuuzu/Yuzora`。

## 指令

前端（bun）：

```bash
bun run tauri:dev        # 桌面 app（Vite dev server 固定 port 1420）
bun run test             # vitest 全跑；單檔：bun run test src/lib/paths.test.ts
bun run test:watch
bun run lint             # eslint
bun run typecheck        # @typescript/native（TS7 tsgo）跑兩份 tsconfig，非一般 tsc
bun run build            # typecheck + vite build
```

Rust（於 `src-tauri/`）：

```bash
cargo check --locked --all-targets
cargo test --locked                          # 單元測試；DB integration 另計（見下）
cargo fmt --package yuzora -- --check
ruby ../.github/scripts/verify-clippy-baseline.rb ../.github/clippy-baseline.json
```

資料庫 integration 測試（需 Docker；密碼為 fixture 專用，非機密）：

```bash
docker compose -f tests/database/docker-compose.yml --profile mssql up -d --wait
cd src-tauri && YUZORA_P8_DATABASE_PASSWORD='Yuzora-P8-Only-2026!' \
  YUZORA_DATABASE_TEST_ENGINES=sqlite,postgres,mssql \
  cargo test --locked --test database_integration -- --ignored
```

Release：推 `v*` tag 觸發 `release.yml`；tag 必須等於 `src-tauri/tauri.conf.json` 的 version，否則 guard job 直接擋下。

## 架構

**IPC 邊界**（理解全 app 的關鍵）：React 前端 ←→ `src/lib/ipc.ts`（各 domain 的 typed invoke wrappers）←→ 約 110 個 `#[tauri::command]`，統一註冊在 `src-tauri/src/lib.rs` 的 `generate_handler![]`；Rust 側一個 service 一個 module（`git_*`、`db_*`、`lsp_*`、`pty_service`、`ssh_service`⋯）。慣例：只有 `src/lib/ipc.ts` 與 `src/lib/platform.ts` 可以 import `@tauri-apps/api/core`（註解約定，lint 不強制）。

**前端分層**：

- `src/app/`＝外殼 chrome：`AppShell.tsx`（版面根）＋`workbench/`（rail、nav、SettingsDialog、CommandPalette、StatusBar）＋`panels/`（AgentZone／Editor／Git／Database／Ssh／Preview）。
- `src/workbench/`＝功能表面與 glue：EditorArea、TabBar、各 SplitView，以及 **Bridge 模式**——`*Bridge`／`*Host` 無畫面元件在 `src/App.tsx` 以扁平兄弟節點掛載，把 Tauri 事件接進 stores；`AppShell` 保持純版面，跨 store 協調寫在 Bridge。
- `src/state/`＝約 20 個 zustand stores（`agentStore`、`dbStore` 最大）；store 間直接 import，無事件匯流排。
- 子系統：`src/agent/`（ACP client——`acpConnection` 包 `@agentclientprotocol/sdk`、`agentRouter` 以 command+cwd 多工共用子行程、`fakeAcpAgent` 測試假件）、`src/editor/`（CodeMirror 6；`documentRegistry`／`viewRegistry` 以路徑索引開啟中的 buffer，LSP 與 ACP 都靠它們）、`src/lsp/`（client 協定層，Rust `lsp_service` 負責 spawn／下載 server）、`src/terminal/`（xterm；本機 PTY＋SSH shell；SFTP 是 SSH 面板的子功能，見 `sftpStore`）、`src/preview/`（原生子 webview 疊在面板上，靠 tauri `unstable` multiwebview API）。
- 路徑 alias：`@/` → `src/`。

**i18n**：`src/lib/i18n/locales/{en,zh-TW}/<ns>.json`，`import.meta.glob` 自動註冊——新增 namespace 只要在兩個語系各放一個 JSON；UI 文字兩語系都要填。

## 測試慣例

- vitest + jsdom（`src/test/setup.ts`、globals on）；測試檔與原始碼同目錄、`.test.ts(x)` 後綴。
- root `tests/`＝DB docker-compose 與 fixtures，root `fixtures/`＝fixture 產生器與可執行假 ACP agent——都不是單元測試。
- jsdom 測不到的（watcher、tab 拖曳、dialog、視覺）→ 用 `gui-acceptance` skill 實機驗收。
- Rust 單元測試 inline；`lib.rs` 的 `command_inventory_tests` 會解析自身原始碼守護 shutdown 順序——改 `run()` 結構前先看它。

## CI 守門（`ci.yml`）

frontend（lint→typecheck→test→build）＋三平台 `cargo check --locked --all-targets`＋macOS 上 fmt／clippy／`cargo test`＋Linux 真實資料庫 integration。

**Clippy 是 exact baseline**（`.github/clippy-baseline.json`，fingerprint＝code/file/line/message/count）：任何 warning 的新增、消失、搬移都會 fail。修掉或新增 warning 後要同步更新該 JSON（腳本只驗證、不重產）。

## Agent skills

### Issue tracker

工作項目與 PRD 追蹤於 GitHub `NakiriYuuzu/Yuzora` Issues。見 `docs/agents/issue-tracker.md`。

### Triage labels

使用五個 canonical triage roles：`needs-triage`、`needs-info`、`ready-for-agent`、`ready-for-human`、`wontfix`。見 `docs/agents/triage-labels.md`。

### Domain docs

本 repo 採 single-context domain layout，以 `.yuuzu/CONTEXT.html` 與 `.yuuzu/adr/` 為權威來源。見 `docs/agents/domain.md`。

## 慣例與路徑

- 語言：回覆繁體中文；技術詞英文、UI 文字中文。
- Git 寫入（add／commit／push 等）僅在使用者明確下令時執行。
- 規劃／執行制度：`.yuuzu/`——`specs/`（計畫）、`adr/`（架構決策）、`eval/`（驗收）、`memory/lessons.html`（教訓），配合 yuuzu-plan／yuuzu-dev skills。
- 結案報告：`docs/html/<題目>-<YYYY-MM-DD>.html`。
- MSSQL driver 是 vendored patch：`src-tauri/vendor/tiberius-0.12.3-yuzora`（Cargo.toml path dependency）。
