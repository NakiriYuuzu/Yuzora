# Changelog

## [0.0.1] - 2026-07-13

首個公開版本：整合 ACP agent、SSH、資料庫與 terminal 的 local-first 桌面開發工作台（Tauri + React）。

### 🚀 Features

**AgentZone（ACP agent）**
- 前端 hosted 的 ACP client，遷移至 `@agentclientprotocol/sdk` 1.2（session config、usage/cost、agent 端 session title 通道）
- curated agent presets（pi／claude／codex／custom）與 AgentRouter 依 command 指紋路由
- Session Index 持久化與跨重啟續聊、三欄位 session title、agent 品牌徽章
- composer 補全：`/slash` 指令、`@skill`、`@file`（workspace path index 模糊搜尋）
- usage/cost chip、session config 選單、tool 詳情展開、startupInfo banner

**資料庫**
- exact-owner connection actor：單連線單 in-flight、query 取消、generation 防 stale
- keyring 憑證保管庫（不落盤）＋ connection profile 持久化（write-ahead ledger）
- 有界結果分頁 session（500 rows/頁）；PG NUMERIC／MSSQL MONEY 無損精確解碼（vendored tiberius fork）
- profile CRUD、憑證缺失提示與復原、dialect-aware SQL helpers；SQLite／PostgreSQL／MSSQL

**編輯器**
- CodeMirror 6 編輯器＋LSP（診斷、補全、格式化）；右分割視窗與手動 Format document
- Markdown 分割預覽與雙向捲動同步

**Terminal**
- xterm 終端抽屜（pty_service）；copy/paste/clear/split 面板命令與拖曳比例記憶

**Git**
- 狀態、staging、commit、branch、log graph（全 branch lane 圖）、commit 詳情 cherry-pick
- 變更列多選、右鍵選單與 rollback 對話框（staleness／conflict 防護）

**其他**
- Preview 面板：dev-server 偵測、原生 webview 導覽狀態機
- SSH 連線管理；logs 查詢／匯出／verbose 開關；工作區最近清單與 agent 數量徽章
- 產品頁（雙語＋六支 Remotion 功能影片）與 GitHub Pages 部署

### 🐛 Bug Fixes

- GUI 啟動（Finder/Dock）補齊 `~/.zshrc` export 的 env——修正 agent 子行程拿不到憑證導致的 Authentication required
- Windows 路徑正規化（最近工作區反斜線、LSP UNC／磁碟機 URI）
- SSH 連線失敗寫入 log；disconnect 錯誤不再被吞

### ♻️ Refactoring

- Context menu 系統重構：typed request、per-command availability 與反灰提示

### 🔧 CI / 建置

- CI：前端品質門檻＋三平台 Rust compile matrix＋clippy 指紋基線＋真實資料庫整合測試（docker Postgres/MSSQL）
- Release：tag 驅動三平台建置（macOS universal `.dmg`、Windows `.msi`/`.exe`、Linux `.AppImage`/`.deb`/`.rpm`）與固定檔名下載別名

### 已知限制

- macOS 安裝檔未簽章：首次開啟請右鍵 →「打開」，或執行 `xattr -cr /Applications/Yuzora.app`
- Windows 安裝檔未簽章，SmartScreen 會出現警告
