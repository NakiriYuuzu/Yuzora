<div align="center">

<img src="src-tauri/icons/128x128@2x.png" width="112" alt="Yuzora icon" />

# Yuzora

**讓 agent 開發，直接運轉在 HERDR。**

<samp>融合 Agent Development Environment 與 HERDR runtime 的開源桌面環境</samp>

<br />

[![CI](https://img.shields.io/github/actions/workflow/status/NakiriYuuzu/Yuzora/ci.yml?style=flat-square&label=CI&labelColor=1b1a17)](https://github.com/NakiriYuuzu/Yuzora/actions/workflows/ci.yml)
[![Pages](https://img.shields.io/github/actions/workflow/status/NakiriYuuzu/Yuzora/deploy-pages.yml?style=flat-square&label=pages&labelColor=1b1a17)](https://nakiriyuuzu.github.io/Yuzora/)
![Version](https://img.shields.io/badge/version-0.0.9--beta.3-86b81f?style=flat-square&labelColor=1b1a17)
![Platform](https://img.shields.io/badge/platform-macOS%20·%20Windows-57534b?style=flat-square&labelColor=1b1a17)
![Tauri](https://img.shields.io/badge/Tauri-2-24c8db?style=flat-square&logo=tauri&logoColor=white&labelColor=1b1a17)

<samp><a href="README.md">English</a> · 繁體中文 · <a href="https://nakiriyuuzu.github.io/Yuzora/">官方網站</a></samp>

<br />
<br />

<img src="docs/readme/hero-zh.gif" width="880" alt="Yuzora 產品導覽：ADE 與 HERDR Spaces、Agents、終端頁面、SSH、資料庫、terminal 與 git" />

</div>

<br />

> Yuzora 是以 HERDR 作為執行與終端 runtime 的 **Agent Development Environment（ADE）**。
> Spaces、named Sessions、Attention 與 Agents 投影在同一個桌面表面；編輯器、git、SSH/SFTP、
> 資料庫與本機 terminal 仍可並用。以 Tauri 打造，預設在地執行。

<br />

## 功能

<table>
<tr>
<td valign="middle" width="38%">

<sub><samp>01 · ADE × HERDR</samp></sub>

### 從 Space 到 agent 終端

Workspace rail 投影 HERDR Spaces；ADE sidebar 整理 named Sessions、Attention 與 Agents。選擇 agent 時，Yuzora 會聚焦其 Session 與 Space，再開啟對應的 HERDR terminal page。每個 Yuzora page 對應一個 HERDR tab，並遞迴呈現 BSP panes。所有 mutation 依 capability 開放，Agent Inspector 維持唯讀。

<code>Spaces</code> <code>named Sessions</code> <code>BSP terminal</code> <code>唯讀 Inspector</code>

</td>
<td valign="middle" width="62%">

<img src="docs/readme/ade-herdr-zh.png" alt="Yuzora ADE：HERDR Spaces rail、named Sessions、agent 狀態、BSP 終端 panes 與唯讀 Agent Inspector" />

</td>
</tr>
</table>

<table>
<tr>
<td valign="middle" width="62%">

<img src="docs/readme/remote-db-zh.png" alt="資料庫面板：查表、下 query、看結構" />

</td>
<td valign="middle" width="38%">

<sub><samp>02 · SSH ＆ 資料庫</samp></sub>

### 遠端即在地

SSH 連上遠端主機瀏覽與編輯檔案、SFTP 傳輸；資料庫面板直接查表、下 query、看結構。連線設定集中管理，known hosts 與憑證都留在本機。

<code>SSH / SFTP</code> <code>PostgreSQL</code> <code>MySQL</code> <code>SQLite</code>

</td>
</tr>
</table>

<table>
<tr>
<td valign="middle" width="38%">

<sub><samp>03 · TERMINAL ＆ GIT</samp></sub>

### 內建 terminal 與 git 工具

xterm 驅動的本機 terminal drawer 就在編輯器下方；git 面板看歷史、看 diff、從 commit 細節直接 cherry-pick。log 查詢與匯出讓除錯不用離開工作台。

<code>xterm + pty</code> <code>git log / cherry-pick</code> <code>log 查詢</code>

</td>
<td valign="middle" width="62%">

<img src="docs/readme/terminal-git-zh.png" alt="本機 terminal drawer 與 git 面板：log、diff、cherry-pick" />

</td>
</tr>
</table>

<br />

## 下載

所有版本皆由 GitHub Actions 建置並發佈於 [GitHub Releases](https://github.com/NakiriYuuzu/Yuzora/releases)，原始碼公開可查。

| 平台 | 格式 | 下載 |
|:--|:--|:--|
| **macOS** | `.dmg` — universal（Apple Silicon / Intel） | [Yuzora-macos-universal.dmg](https://github.com/NakiriYuuzu/Yuzora/releases/latest/download/Yuzora-macos-universal.dmg) |
| **Windows** | `.exe`（NSIS）— x64 | [Yuzora-windows-x64-setup.exe](https://github.com/NakiriYuuzu/Yuzora/releases/latest/download/Yuzora-windows-x64-setup.exe) |

Windows `.msi` 安裝檔與歷史版本見 [GitHub Releases](https://github.com/NakiriYuuzu/Yuzora/releases)。Linux 僅作為 CI／測試 host，不是 Yuzora 支援的桌面發佈平台。

## 技術架構

| 層 | 技術 |
|:--|:--|
| 桌面框架 | [Tauri 2](https://tauri.app)（Rust） |
| 前端 | React + TypeScript + Vite |
| Agent runtime | HERDR public API ＋官方 terminal session connector |
| Terminal | xterm.js ＋本機 pty ＋ HERDR terminal pages |
| 工具鏈 | Bun · Vitest · Cargo |

Yuzora 會優先使用 PATH 安裝的 HERDR binary；偵測不到時，會自動改用 macOS／Windows 安裝檔內附且固定版本的 Yuzora-managed binary。關閉頁面或 App 時，Yuzora 只釋放自己建立的 connector child，不會終止 HERDR server、panes 或 agents。

### Experimental Windows WSL Pi Plugin

Windows `0.0.9-beta.3` 安裝檔會在 App resource 內附 Pi-only 的 `Yuzora WSL Agents` Plugin，但**不會自動啟用**。使用者必須明確 link 封裝路徑、設定目標 WSL distro，並透過 HERDR Plugin actions 安裝 adapter。只支援 Plugin-managed panes。Yuzora 只消費 HERDR snapshot／events 的 live identity 與 state；不解析 terminal、不推斷 Linux process、不投影 Pi native session id，也不保證 resume／control。HERDR `v0.8.2` Runtime 為 Stable，但 Windows Plugin surface 與本整合仍為 Experimental。既有 running protocol-19 server 必須由使用者明確停止並重啟；Yuzora 升級時不會 kill。詳見 [`herdr-plugins/yuzora-wsl-agents/README.md`](herdr-plugins/yuzora-wsl-agents/README.md)。

## 開發

```bash
bun install          # 安裝依賴
bun run tauri:dev    # 啟動桌面 app（dev server :1420）
bun run test         # vitest
bun run build        # 前端建置（含 typecheck）
cd src-tauri
cargo check          # Rust 檢查
```

從原始碼建置安裝檔：

```bash
bun install
bun run tauri:build
```

本機建置會刻意停用 updater 產物與 release 簽章，因此不需要 production secrets。
官方 macOS 安裝檔只會在受保護的 release workflow 中完成 Developer ID 簽章與 notarization；各平台 gate 詳見 `docs/operations.md`。

> README 與[官方網站](https://nakiriyuuzu.github.io/Yuzora/)中的產品動畫與截圖，
> 均由 [`site-remotion/`](site-remotion/) 內的 [Remotion](https://www.remotion.dev) 專案程式化渲染，
> 與 app 本體的 design tokens 1:1 對齊。

<br />

---

<div align="center">

**ADE 與 HERDR，融合成一個工作面。**

<samp>夕空下的 agent development environment</samp>

<sub>

[原始碼](https://github.com/NakiriYuuzu/Yuzora) · [回報問題](https://github.com/NakiriYuuzu/Yuzora/issues) · [所有版本](https://github.com/NakiriYuuzu/Yuzora/releases) · [官方網站](https://nakiriyuuzu.github.io/Yuzora/)

</sub>

</div>
