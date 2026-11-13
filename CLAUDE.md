# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Yuzora：給 CLI coding agents 的原生工作區（ADE）。**v0.1.0-beta.2 開發中**：以 herdr-app（Rust＋GPUI＋libghostty＋QuickJS mods）為基底，放在 `app/`，把 Git、資料庫、SSH／SFTP、Files 與編輯器、設定、mods／templates 內建進去。v0.1.0-beta.1 以前的 Tauri 2＋React 19 版已從 repo 移除，原始碼在 git 歷史（tag `v0.1.2`）。GitHub：`NakiriYuuzu/Yuzora`。

各 agent 共用的規則（beta.2 架構與目錄地圖、Release 平台支援、Rust 建置目錄重用、Agent skills）只寫在 AGENTS.md，這裡直接匯入；本檔只放 Claude 補充內容。

@AGENTS.md

## 指令

以下 shell snippets 使用 Bash／Git Bash／WSL 語法。Windows PowerShell 請拆成多行，並用 `$env:NAME = ...` 設定環境變數。在 worktree／lane 執行 cargo 前，先依 AGENTS.md「Rust 建置目錄重用」設定 `CARGO_TARGET_DIR`。

beta.2 app（於 `app/`，需 Zig 0.16.0 與 ghostty submodule）：

```bash
export PATH="$HOME/.local/zig-0.16.0:$PATH"
git submodule update --init app/vendor/ghostty
cd app
cargo build                          # target/debug/herdr-app（package／binary 暫用上游名稱）
cargo test --locked                  # 全 workspace；單一 crate：cargo test --locked -p yuzora-core
scripts/yuzora/check-mods            # 在複本上對每個 yuzora-* mod 跑 herdr-app check
scripts/yuzora/bundle-macos /tmp/yz  # 包成 /tmp/yz/Yuzora.app（本機用，非正式打包）
scripts/yuzora/package               # 發布用安裝檔到 dist/（細節見 L7 補充）
scripts/yuzora/sync-check            # subtree pull 之後的上游同步檢查（--quick 不跑 cargo test）
```

上游 herdr-app 的其他工具（`scripts/sandbox`、`bun scripts/docs.ts check`、`justfile`）見 `app/AGENTS.md`。

官網與根目錄（bun）：

```bash
bun install
bun run lint             # eslint：site/*.js、scripts/*.mjs、tests/*.js
bun run site:seo         # 由 site/index.html 與 site/i18n.js 產生 site/en/ 與 site/sitemap.xml
bun run test             # bun test：tests/ 底下的官網、CHANGELOG 與 site-remotion 版本測試
```

`app/scripts/*.test.ts` 也是 bun test，由 app 自己跑（`cd app && bun test scripts`）。

## beta.2 架構

先讀 `docs/beta2/architecture.md`（ADR）與 `docs/beta2/lanes.md`。重點：

- **三層**：herdr 殼（上游 `app/src`、上游 mods）→ Yuzora Rust 原生層（`app/crates/yuzora-*`，各提供 gpui-shell `HostModule`：async 函式與原生元件）→ Yuzora JS mods（`app/defaults/mods/yuzora-*`，組合側欄模式與 tool pane）。
- **整合點**：`app/src/yuzora/mod.rs` 掛上 `yuzora:core` 與 7 個 lane 模組（shell、git、db、ssh、files、settings、perf）；`app/Cargo.toml` 已依賴全部 crate。lane 只在自己 crate 的 `host_module()` 加函式，`DECLARATIONS` 要同步加 `export function …`（不一致會在 `validate` 失敗）。
- **herdr 模型**：herdr-core（`hc`）執行終端機與 machines，app 只畫畫面；space 屬於一台機器，tab 屬於一個 space，pane 可以是終端機或 mod pane（`pane.split {mod, props}`）。layout 節點 `row`／`column`／`stack`／`block`，`layout.select {stack, child}` 切換。mod API 在 `app/defaults/herdr.d.ts`，Yuzora 的在 app home 的 `yuzora.d.ts`。
- **外連**：遙測、feedback 送 herdr.dev、update notes 媒體都關閉（`app/src/yuzora/mod.rs` 的常數與 `nothing_goes_to_herdr_dev` 測試）；app 自我更新改走本 repo 的 GitHub Releases（`app/src/yuzora/release/`，manifest 必須以 release 金鑰簽章，只有安裝好的 release build 會檢查）；herdr-core 的安裝與更新保留上游官方通道。app home `~/.config/yuzora`（`YUZORA_APP_HOME`；不讀 `HERDR_APP_HOME`），core layout 文件名 `yuzora`。
- **探索**：mod 在 `mod.json` 宣告 `"yuzora": {"pane": …}`／`{"settings": …}`，由 `yuzora:core` 的 `panes()`／`settingsPages()` 列出。
- **i18n**：介面只有英文（beta.3 起）。`app/defaults/mods/<mod>/i18n/en.json`，JS `t(key, vars)`，Rust `yuzora_core::i18n`；沒有語言設定，不要新增 `zh-TW.json`（`app/scripts/yuzora-i18n.test.ts` 會擋）。
- **與官方 Herdr 共用 core**：終端機、machines、templates 共用；收養來的終端機在任一邊關閉都會在 core 結束。細節與已決定的取捨見 `docs/beta2/herdr-compat.md`。
- **mod 限制**：mod 的 `exec` 上限 30 秒／8 MiB、`readTextFile` 1 MiB、`writeTextFile` 只能寫 app home。DB 連線、SSH session、大量資料都要在 Rust 層做；遠端用 `yuzora_core::remote`（`hc exec --on`，可自訂 timeout）。

## Tauri 版（已移除）

v0.1.0-beta.1 以前的 Tauri 2＋React 版不在 repo 裡。要對照舊實作時讀 git 歷史（例如 `git show v0.1.2:src-tauri/src/lib.rs`）；React UI 不可重用，也不參考既往 GRILL-ME tickets。`app/vendor/` 的 russh／russh-sftp／tiberius patches 是從那一版複製過來的。

## 測試慣例

- app：Rust 測試 inline 或 `<module>/tests.rs`、整合測試 `app/tests/*.rs`；每個 `yuzora-*` crate 自帶單元測試。`tests/notices.rs` 守 `THIRD-PARTY-NOTICES` 與 `Cargo.lock` 一致；`update_notes` 測試要求 `app/CHANGELOG.md` 第一筆版本等於 `0.1.0-beta.2`。
- `mods::lifecycle_tests::close_starts_native_work_before_releasing_the_copy` 在平行執行時偶爾失敗（上游既有），單獨重跑可過。
- 本機 `scripts/test_distribution.py` 若裝有 `/Applications/Herdr.app`，installer 測試會判成 reinstall 而失敗；CI 不受影響。
- 官網與根目錄：`bun run test`。`tests/site-*.test.js` 先 import `tests/site-dom.js` 取得 jsdom 的 `DOMParser` 與 `document`，並讀 `site:seo` 產生的 `site/en/`、`site/sitemap.xml`，所以先跑 `bun run site:seo`；`tests/changelog.test.js` 守兩份根目錄 CHANGELOG 的版本與項目對應；`tests/site-remotion.test.js` 守 `site-remotion/package.json` 的 Remotion 套件同版本。`bun run lint` 是官網 JS 唯一的靜態檢查（測試只把 `site/app.js` 當文字讀）。

## CI 守門

- `app-package.yml`：改到打包相關路徑的 PR／push 與手動觸發；macOS、Windows、Linux x64／arm64 四平台打包並以使用者的方式安裝檢查，固定狀態名稱 `App (package)`。不建 tag、不發布、不讀 secret。
- `app-ci.yml`：base 為 `beta2/integration`、改到 `app/**` 的 PR；ubuntu-22.04＋macos-15 建置與 `cargo test --locked`；macOS 另跑 yuzora crates、`check-mods`（check 需要視窗伺服器）與 shot_pixels，Linux 另跑 packaging 與文件檢查；另有一個 windows-2022 job 只跑 `cfg(windows)` 的 console 視窗測試（`yuzora-core spawn::`、`yuzora-git exec::`、`yuzora-files fsops::tests::run_starts`；整個 workspace 的測試在 Windows 上有既有的失敗，不跑），也納入 aggregator。固定狀態名稱 `App (build · test)`。只有 push 到 `beta2/integration` 會存 cache。
- `site.yml`：改到 `site/**`、`tests/*.js`、`scripts/generate-site-pages.mjs`、`site-remotion/package.json`、`eslint.config.js`、根目錄兩份 CHANGELOG、`package.json`、`bun.lock` 的 PR 與 push 到 `main`。`Site (test)` 跑 `bun run lint`、`site:seo` 與 `bun run test`；push 到 `main`（或從 `main` 手動 dispatch）時，同一次建置的 `site/` 由 `Deploy Pages` job 部署到 GitHub Pages。
- `app-ci.yml`、`app-package.yml`、`site.yml` 都有路徑條件，`app-release.yml` 只能手動 dispatch：沒有任何一個 check 會在每個 PR 上出現，設定 required check 前先確認這點。

## 慣例與路徑

- 語言：回覆繁體中文；技術詞英文、UI 文字只有英文。
- Git 寫入（add／commit／push 等）僅在使用者明確下令時執行。beta.2 lane 的授權與 PR 流程見 `docs/beta2/lanes.md`；任何 session 都不碰 `main`、tag、release。
- commit trailer：`Co-Authored-By: Claude Opus 5.5 <yuuzu@yuuzu.net>`。
- 規劃／執行制度：`.yuuzu/`——`specs/`、`adr/`、`eval/`、`memory/lessons.html`。beta.2 決策以 `docs/beta2/` 為準。
- 結案報告：`docs/html/<題目>-<YYYY-MM-DD>.html`。

## Lane 補充

<!-- yuzora:lane:L1:start -->
### L1 實機驗證

- `shot` 只拍最後畫出的畫面；視窗被遮住時是舊畫面。用同一組隔離環境變數再執行一次 app 執行檔，會把既有視窗帶到前面（`raise`），再 `shot`。
- 隔離 home 第一次啟動會開 welcome；先在 `<home>/shared.json` 寫入 `"frame.welcomed": "skipped"` 可略過。
- 熱載入只在檔案內容改變時觸發；只改 `i18n/*.json` 不會重讀字串，要連同 `main.js` 一起改或重啟。
<!-- yuzora:lane:L1:end -->

<!-- yuzora:lane:L2:start -->
- L2 Git：pane 之間用 shared key（`yuzora.git.diff`／`merge`／`rebase`）交棒、`changed(repo, since)` 即時更新、`yuzora.badge.git`／`yuzora.status.git` 給 L1，細節在 `docs/beta2/git.md`〈pane 之間的約定〉。
<!-- yuzora:lane:L2:end -->

<!-- yuzora:lane:L3:start -->
- L3（資料庫）：已存的密碼只送給存它時的伺服器（`ConnectionConfig::password_may_follow`：engine、host、port、user、instance、tunnel 相同，TLS 不變弱、verify 模式不換 CA）。改連線相關程式時不要放寬這條規則。
- 原生元件在 JS 用 `DataGrid.new(id, props)`、`SqlEditor.new(id, props)`，不是 `new X(...)`（`herdr-app check` 會報 not a constructor）。
<!-- yuzora:lane:L3:end -->

<!-- yuzora:lane:L4:start -->
- L4 SSH 的 russh 走 crate 自己的 tokio runtime（`yuzora_ssh::runtime`）；HostModule 的 async 函式用 `runtime::bridge` 交回 GPUI executor，不要在 mod 的 host 函式裡直接 `tokio::spawn`。
- 改 `yuzora:ssh` 函式時同步改 `js.rs` 的 `DECLARATIONS`；錯誤以 `"[code] message"` 拋給 JS。
<!-- yuzora:lane:L4:end -->

<!-- yuzora:lane:L5:start -->
### L5 檔案與編輯器

- 一個檔案只有一個 `DocView`，同一時間只有一個 pane 畫它的編輯器（owner）；不要讓同一個 `EditorState` entity 在同一個 frame 被畫兩次。
- mod pane 不知道自己的 pane id：編輯器取得焦點時用 `focused().pane` 記到文件（`notePane`），開檔時靠它 `pane.focus` 回去。
<!-- yuzora:lane:L5:end -->

<!-- yuzora:lane:L6:start -->
- L6 實機看設定頁：`<app> do app.settings '{"section":"templates"}'`（section 可以是 Yuzora 頁的 mod 名或 section key）；直接開模板表單：`<app> do tab.new '{"mod":"yuzora-settings-templates","props":{"edit":"<name>"}}'`；再 `shot` 留證。
- 隔離 home 第一次啟動會跳 Herdr 的 welcome 視窗擋住 shot：app 停止時在隔離 home 的 `shared.json` 寫入 `"frame.welcomed": "skipped"`。
<!-- yuzora:lane:L6:end -->

<!-- yuzora:lane:L7:start -->
L7（發布與更新，`docs/beta2/release.md`）：

```bash
cd app
YUZORA_PACKAGE_BIN=$CARGO_TARGET_DIR/debug/herdr-app scripts/yuzora/package   # 只檢查打包，不做 release build
scripts/yuzora/package                                                       # release build 並打包到 dist/
bun test scripts/yuzora/ && bun scripts/yuzora/manifest.ts yuzora dist
cargo test --locked -p herdr-app yuzora::release
cargo test --locked -p yuzora-core migrate
```

本機的 `makensis`（`brew install makensis`）沒設 `LC_ALL=en_US.UTF-8` 會以 `std::bad_alloc` 結束。
<!-- yuzora:lane:L7:end -->

<!-- yuzora:lane:L8:start -->
### L8 實機驗證

- popover 截圖：用同一組隔離環境變數執行 `herdr-app do yuzora-perf.toggle` 打開（再執行一次關閉），等下一次重畫後 `shot`。
- 取樣只在視窗位於前景時進行；`shot` 前先把視窗帶到前面（再執行一次 app），否則 chip 會停在上一個樣本，並記為 `skipped_no_focus`。
<!-- yuzora:lane:L8:end -->
