# Herdr 0.8.0：WSL 與 AI agents 的官方整合邊界

> 研究範圍：Herdr 官方網站/文件、`/tmp/pi-github-repos/herdrdev/herdr` checkout（`Cargo.toml` 明確為 `0.8.0`）。未使用第三方 WSL bridge 或非官方推測。

## 結論

Herdr 官方支援的是「在同一個 OS/環境內」執行 Herdr、agent 與本機 socket/API。Linux Herdr 在 WSL 內執行是完整支援路徑；Windows Herdr 以 pane 啟動 `wsl.exe`，或從 Windows 直接以 `wsl.exe ... agent` 啟動 Linux agent，官方目前沒有跨 Windows↔WSL 的 agent-state/socket bridge。這兩種跨界路徑仍可作為互動 terminal 使用，但不會自動把 Linux agent 的 hooks/plugin 狀態送回 Windows Herdr。

Herdr 的基本 screen/process detection 不依賴 hooks；因此 agent 若被 Herdr 的同一 OS PTY/process tree 看見，仍可能有一般 terminal multiplexer 與螢幕狀態偵測。可是 Windows 端的 `wsl.exe` 只是 Windows process，checkout 沒有 WSL-aware process detection、`wsl.exe` argument parser、WSL path translation 或 WSL socket transport；不能把這種可見 terminal 等同於完整 direct integration。

## 三個執行情境

### 1. Linux Herdr binary 在 WSL 內執行：官方完整路徑

- `website/install.sh` 只下載 `linux-{x86_64,aarch64}` 或 macOS binary；在 WSL 內以 Linux 身分執行即可。Windows installer 是另一條 `website/install.ps1` 路徑。
- Linux Herdr 建立/注入 `HERDR_ENV=1`、`HERDR_SOCKET_PATH`、`HERDR_BIN_PATH`，並在 managed pane 啟動 agent 時注入 `HERDR_WORKSPACE_ID`、`HERDR_TAB_ID`、`HERDR_PANE_ID`（`src/integration/env.rs`、`src/pane.rs`）。
- Linux hooks 以 Unix socket (`AF_UNIX`) 連到同一 WSL 內的 Herdr server；JS plugin 在 Linux 使用 `HERDR_SOCKET_PATH`，Windows 才轉成 named pipe endpoint（例如 `src/integration/assets/pi/herdr-agent-state.ts`、`.../opencode/herdr-agent-state.js`）。
- 因為 binary、agent、socket 都在 WSL Linux namespace/filesystem，direct integrations 可用；不需 WSLENV。

### 2. Windows-native Herdr pane 啟動 `wsl.exe`，再在 Linux 內互動啟動 agent：只有 terminal 層

- Windows build 有 native ConPTY/Windows PTY（`src/pty/backend.rs`、`src/pty/actor.rs`）及 Windows named-pipe API；官方文件/README 只宣稱 Windows beta binary，沒有「Windows Herdr 控制 WSL agent」的功能說明。
- `[terminal].default_shell` 是單一 `String`（`src/config/model.rs`）；source 沒有 `wsl.exe` 專用欄位、argv parser、distro/path translation 或 `--cd`/`--exec` builder。因而不能把 `wsl.exe -d Ubuntu -- ...` 當成官方解析過的 shell 設定。
- Windows Herdr 注入到 `wsl.exe` 的環境變數，不會因 Herdr 自己的程式碼而自動進入 Linux child：checkout 沒有 `WSLENV` 寫入/重建邏輯，也沒有 WSL-side relay。`WSLENV` 在 source 中找不到；`wsl.exe` 亦不是 Herdr 的 spawn/parse 特例。
- 即使手動讓某些環境變數進入 WSL，Windows socket 是 named pipe（JS 端 Windows endpoint 是 `\\.\pipe\...`），Linux `.sh`/Python hooks 使用 `AF_UNIX`；source 沒有把 Windows named pipe 暴露成 Linux Unix socket 或 TCP bridge 的實作。
- 因此：pane/PTY 與互動 agent **可以工作**；Linux direct hook/plugin 回報到 Windows Herdr **不會自動工作**。Windows Herdr 仍可做其 Windows 端 screen/process detection，但 source 沒有證明它能辨識 WSL 內 agent 的 Linux process identity。

### 3. Windows 直接 wrapper：`wsl.exe ... agent`

這與情境 2 的 boundary 相同，只是省略互動 shell。`wsl.exe` 接受 command line 並在 Linux 啟動 agent，但 Herdr 官方沒有 wrapper、WSLENV allowlist、socket forwarding 或 pane-identity forwarding。結果仍是：agent 本身可在 WSL 執行；Windows Herdr 的 direct lifecycle/session integration 不會因為 command line wrapper 而自動出現。

## 官方 direct integration inventory

以下是 `src/api/schema`/`src/integration/registry.rs`/`src/integration/targets.rs` checkout 中的 17 個可安裝 target。`integration_target_supported()` 在 `cfg(not(windows))` 對全部 target 回傳 true；Windows 也列出這些 target，但這代表 Windows-native agent config/install 支援，不代表 Linux WSL child bridge。

| Target | 機制與狀態 | installer/assets（Linux / Windows） | runtime identity/socket 要求 |
|---|---|---|---|
| Pi | extension；rich lifecycle state + session | `src/integration/assets/pi/herdr-agent-state.ts`（同一 TS） | `HERDR_ENV=1`、`HERDR_PANE_ID`、`HERDR_SOCKET_PATH`；JS 在 Windows 轉 named pipe |
| OMP | extension；rich lifecycle state + session | `assets/omp/herdr-agent-state.ts`；由 `PI_CONFIG_DIR`/`.omp` extension dir 定位 | 同上；另需 Pi/OMP 使用不同 extension dir |
| Claude | lifecycle hook installer，但 0.8.0 hook 實際為 session-only（`SessionStart`；state 由 screen detection） | `assets/claude/herdr-agent-state.sh` / `.ps1`；寫入 `~/.claude/hooks` + `settings.json` | Linux `.sh` 需 Unix socket、`python3`；Windows `.ps1` 以 `HERDR_BIN_PATH` 或 `herdr` CLI 回報 |
| Codex | session-only hook（`SessionStart`）+ config/hooks wiring | `assets/codex/...sh` / `.ps1`；`~/.codex`、`hooks.json`、`config.toml` | Linux 需 `HERDR_SOCKET_PATH`、`HERDR_PANE_ID`、Python；檢查 `CODEX_THREAD_ID` 避免錯綁 |
| Kimi | lifecycle hooks：SessionStart、prompt/tool、permission、compact、stop 等 | `assets/kimi/...sh` / `.ps1`；`~/.kimi-code/hooks` + `config.toml`；要求 Kimi Code >= `0.14.0` | 同一環境的 pane id/socket |
| GitHub Copilot | session-only（`SessionStart`；舊 lifecycle hooks 會被移除） | `assets/copilot/...sh` / `.ps1`；`~/.copilot/hooks` + `settings.json` | 同一環境的 pane id/socket/CLI |
| Devin | session-only reporting（多個事件均報 session identity） | `assets/devin/...sh` / `.ps1`；XDG `devin` config | 同一環境的 pane id/socket |
| Droid | session-only（`SessionStart`；舊 lifecycle state hooks 移除） | `assets/droid/...sh` / `.ps1`；`~/.factory/hooks` + settings | 同一環境的 pane id/socket |
| OpenCode | plugin；rich lifecycle state + separate TUI session-selection plugin | `assets/opencode/herdr-agent-state.js` + `herdr-tui-session.js`；`~/.config/opencode/plugins`、TUI config | JS 使用 Unix socket；Windows JS 使用 `\\.\pipe\${HERDR_SOCKET_PATH}` |
| Kilo | plugin；rich lifecycle state/session | `assets/kilo/herdr-agent-state.js`；`~/.config/kilo/plugin` | 同一環境的 pane id/socket |
| Hermes | Python plugin；session identity hooks（startup/reset/resume） | `assets/hermes/plugin.yaml` + `__init__.py`；`~/.hermes/plugins/herdr-agent-state` | 使用 `HERDR_BIN_PATH`/`herdr pane report-agent-session`；Windows plugin可用，但仍無 WSL bridge |
| Qoder CLI | session-only（`SessionStart`） | `assets/qodercli/...sh` / `.ps1`；`QODER_CONFIG_DIR`/`~/.qoder` | 同一環境的 pane id/socket |
| Qwen | session-only（`SessionStart`） | `assets/qwen/herdr-agent-session.sh` / `.ps1`；`QWEN_HOME`/`~/.qwen` | 可用 `HERDR_BIN_PATH` 呼叫 CLI；同一環境 pane id/socket |
| Cursor agent | session-only hook | `assets/cursor/...sh` / `.ps1`；`~/.cursor` hooks | 同一環境的 pane id/socket |
| Mastracode（Mastra 相關的官方 target 名稱） | rich lifecycle hooks：SessionStart、prompt、AgentStart/End、tool、permission、subagent、interrupt、stop | `assets/mastracode/...sh` / `.ps1`；`~/.mastracode/hooks` | 同一環境的 pane id/socket |
| Antigravity CLI (`agy`) | session-only：只有 `PreInvocation`，官方註解明確說 screen detection 擁有 agent state | `assets/antigravity_cli/...sh` / `.ps1`；`~/.gemini/config/hooks.json` | 同一環境的 pane id/socket |
| Grok | session-only hook + `herdr.json` registration | `assets/grok/...sh` / `.ps1` + config asset；`GROK_HOME`/`~/.grok` | 同一環境的 pane id/socket |

### Screen/process detection（不是 direct integration）

Herdr 0.8.0 仍有不需安裝的 screen/process path。`src/detect/mod.rs`：

- `Agent::ALL` 有 22 個可識別 label：Pi、Claude、Codex、Gemini、Cursor、Devin、Antigravity、Cline、OMP、Mastracode、OpenCode、Copilot、Kimi、Kiro、Droid、Amp、Grok、Hermes、Kilo、Qodercli、Qwen、Maki。
- `SCREEN_MANIFEST_AGENTS` 有 20 個 screen-manifest agent；排除 OMP 與 Mastracode。manifest/state 是 Herdr 自己的 terminal-tail heuristic，不是 agent plugin 安裝。
- 官方 agents 文件說 automatic detection 不需 hooks；direct integrations 只是較強的 semantic reporting。這種 detection 的前提仍是 Herdr 能觀察到正確的 PTY/process/screen；source 沒有 WSL-aware Windows→Linux process bridge。

## Installer / binary assets 與 OS 邊界

1. **Linux/macOS installer**：`website/install.sh` 只接受 `Linux`/`Darwin`，下載對應 binary 至 `$HOME/.local/bin/herdr`。
2. **Windows installer**：`website/install.ps1` 讀官方 manifest、下載 Windows package 並寫 Windows PATH；它不安裝 WSL Linux binary、Linux-side hooks 或 socket bridge。
3. **Hook asset selection**：多數 hook 在 Rust compile-time 以 `cfg!(windows)` 選 `.ps1`，非 Windows 選 `.sh`；Pi/OMP/OpenCode/Kilo 等 JS/TS plugin 共用 asset，但其 socket endpoint 仍按 `process.platform` 分成 Unix socket/named pipe。
4. **`HERDR_BIN_PATH`**：`src/integration/env.rs::apply_pane_base_env` 設成 `current_exe()`。Windows PowerShell/Python-style session hooks及 Hermes/Qwen 等會優先使用它；Linux child 若是另一個 WSL environment，不會自動取得 Windows executable 的可執行 Linux path。
5. **`HERDR_PANE_ID`**：由 Herdr 在 managed pane launch 時注入；所有 direct assets 都把它當作 report target。沒有 pane id，plugin/hook 會安靜退出或不回報。
6. **`HERDR_SOCKET_PATH`**：`src/api/mod.rs` 定義；同一 OS 內為 active Herdr API socket。Linux shell hooks用 `AF_UNIX`；Windows JS用 named pipe endpoint。source 沒有 WSL path translation 或 cross-OS endpoint abstraction。

## WSLENV、wsl.exe parsing、socket：已核對的 source 結果

- checkout 全文相關 integration/env、PTY/platform、installer、asset code 中沒有 Herdr 自己建立或修改 `WSLENV` 的程式碼；`WSLENV` 不是 `src/integration/env.rs` 的 contract variable。
- 沒有 Herdr `wsl.exe` 專用 spawn helper、distro discovery、`--cd`/`--exec` argument parser 或 Windows↔Linux socket proxy。Windows PTY 直接使用 portable-pty/ConPTY；Linux PTY 使用 Unix backend。
- Windows direct hook 的 socket contract 是 named pipe；Linux direct hook 的 socket contract 是 Unix domain socket。此差異是實際 transport boundary，不是僅路徑字串差異。
- 因此官方 source 能證明的自動化只有「同 OS/同 socket namespace」。不能證明、也沒有實作「Windows Herdr pane 自動將 `HERDR_*`/socket/pane identity 經 WSLENV 傳入 Linux agent」。

## Works automatically / does not work automatically

### Automatically works

- Herdr 與 agent 都在 WSL Linux 內：PTY、screen/process detection、已安裝的 Linux direct integration、Linux Unix socket reports。
- Herdr 與 agent 都在 Windows native：Windows binary、Windows `.ps1` hooks，以及 JavaScript plugin 對 Windows named pipe 的 endpoint handling。
- 任一未安裝 direct integration 的 agent，只要在 Herdr 同一 OS PTY 內執行：Herdr 的一般 pane/workspace/screen detection 仍是設計上的 fallback。

### Does not automatically work / severity

- **High（功能缺口）**：Windows-native Herdr → `wsl.exe` → Linux agent 的 direct lifecycle/session reports；無官方 WSLENV/socket bridge。
- **High（身份缺口）**：Windows `HERDR_PANE_ID`/`HERDR_SOCKET_PATH`/`HERDR_BIN_PATH` 不會由 Herdr source 自動跨越 `wsl.exe` boundary；Linux hooks 不能假定能連 Windows named pipe。
- **Medium（啟動/解析缺口）**：把 `wsl.exe -d <distro> ...` 填入 Windows Herdr shell；`default_shell` 是單字串且 source 沒有 wsl.exe argv parsing/path translation，不能視為官方支援的 structured WSL launcher。
- **Not provided**：官方沒有宣稱或提供 Windows Herdr 控制 WSL agent 的 TCP/Unix-socket relay、Linux Herdr sidecar、remote daemon 或 WSL-specific installer。

## Primary sources kept

- Herdr official docs: [agents](https://herdr.dev/docs/agents/), [integrations](https://herdr.dev/docs/integrations/), [quick start](https://herdr.dev/docs/quick-start/), [plugins](https://herdr.dev/docs/plugins/).
- Upstream repository: [herdrdev/herdr](https://github.com/herdrdev/herdr), checkout `/tmp/pi-github-repos/herdrdev/herdr`, version proof `Cargo.toml` = `0.8.0`.
- Relevant checked-out files: `website/install.sh`, `website/install.ps1`, `src/config/model.rs`, `src/pane.rs`, `src/pty/backend.rs`, `src/pty/actor.rs`, `src/platform/windows.rs`, `src/api/mod.rs`, `src/session.rs`, `src/detect/mod.rs`, `src/integration/env.rs`, `src/integration/registry.rs`, `src/integration/targets.rs`, `src/integration/actions.rs`, and `src/integration/assets/**`.

## Gaps

Herdr official docs do not describe a WSL mode/bridge, and the source has no WSL-specific implementation to test. A live Windows+WSL machine would be required to experimentally characterize incidental screen/process behavior; that would not change the source-level conclusion that cross-boundary direct integration is not officially provided.
