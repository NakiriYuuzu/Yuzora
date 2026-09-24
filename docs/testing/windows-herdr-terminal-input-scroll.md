# Windows Herdr terminal：捲動與 Alt 組合鍵量測／驗收

計畫：`specs/herdr-windows-terminal-and-sidebar-navigation-plan.html`（Phase 1、7、8）。

本文件用於收集兩項 Windows-only 問題的證據：

1. Herdr terminal 分頁捲動卡頓（macOS、WSL 正常）。
2. Herdr terminal 內 Alt+Q、Alt+T 等 Agent 快捷鍵無效。

Phase 7、8 的修正要依這裡的結果決定。請在 **Windows 原生 Herdr 0.9.1** 上執行。

## 不要做的事

- 不要停止或刪除既有的 Herdr Session。探針只會建立並清理自己的暫存 session（`yz-probe-*`）。
- 不要把紀錄檔貼到公開位置。匯出時請使用「已清理」的版本。

## A. 隔離探針（約 1–2 分鐘）

探針在暫存目錄啟動獨立的 Herdr server（獨立的 `APPDATA`／`LOCALAPPDATA`／session 名稱），量測 IPC 延遲並測試按鍵送達方式，結束時停止並刪除它。

1. 取得探針：
   - 單檔版本：`output/probe-herdr-terminal-io.exe`（在 macOS 用 `bun build --compile --target=bun-windows-x64 scripts/probe-herdr-terminal-io.ts --outfile output/probe-herdr-terminal-io.exe` 產生），或
   - 已安裝 bun 時：`bun scripts/probe-herdr-terminal-io.ts <herdr.exe>`。
2. 選擇 Herdr binary（二擇一，並記下用了哪一個）：
   - Yuzora 內附：`<Yuzora 安裝目錄>\resources\herdr\windows-x86_64\herdr.exe`
   - PATH 上的 0.9.1：`(Get-Command herdr).Source`
3. 在 PowerShell 執行：

   ```powershell
   .\probe-herdr-terminal-io.exe "C:\path\to\herdr.exe" --out "$env:USERPROFILE\Desktop\yz-probe-windows.json"
   ```

4. 預期最後一行是 `PROBE OK win32 herdr 0.9.1`，並寫出 JSON。若中途失敗，請回傳完整終端輸出。

### 判讀

- **Part A（延遲）**：`sessionListSpawnMs`（Yuzora 每次 API 會啟動兩次這個子程序）、`pingRoundTripMs`、`paneGetMs`、`paneScrollMs`、`terminalScrollToFrameMs`、`terminalScrollFrames.bytes`。
- **Part B（按鍵矩陣）**：每列是 `程式 送出方式 按鍵 結果`。
  - 程式：`P1` PowerShell `ReadKey`（console 按鍵語意）、`P2` Node raw 讀取器（Node Agent 實際收到的位元組）、`P3` 同 P2 但啟用 Kitty keyboard。
  - 送出方式：`D1` Yuzora 目前的原始 `ESC q`、`D2` win32-input-mode 記錄、`D3` Herdr API `pane.send_keys`。
  - 結果：`ok`（正確）、`split`（被拆成 Esc＋字元）、`raw-w32`（W32IM 序列原樣送達程式）、`none`（沒收到）、`other`（其他，看 JSON 的 `observed`）。

### macOS 參考結果（2026-09-24，herdr 0.9.1，隔離 session）

| 項目 | p50 | p95 |
|---|---|---|
| `session list` 子程序 | 4.6 ms | 5.1 ms |
| socket `ping` | 7.9 ms | 116 ms |
| `pane.get` | 108 ms | 113 ms |
| `pane.scroll` | 108 ms | 113 ms |
| connector `terminal.scroll` → 下一張 frame | 2.4 ms | 2.4 ms |

- 捲動 frame：30/30 為 delta frame，約 351 bytes。
- 按鍵：P2、P3 的 D1、D3 全部 `ok`；D2 為 `raw-w32`。macOS 的 pane 是 Unix PTY，沒有 ConPTY 轉換，所以這是預期結果，用來確認 harness 正確。
- 觀察：Herdr 的 `pane.get`／`pane.scroll` 在 macOS 本身就約需 100ms，connector 捲動則只要約 2ms。

## B. App 內除錯紀錄（約 2 分鐘）

1. 開啟 Yuzora → 設定 → Logs，開啟「詳細紀錄（debug）」。
2. 開啟一個 Herdr terminal 分頁，在其中執行 pi（或其他 Agent），讓畫面有足夠的歷史可以捲動。
3. 用滑鼠滾輪連續捲動約 30 秒；若有觸控板，再用觸控板捲動約 30 秒。
4. 在 pi 中依序按：Alt+Q、Alt+T、Alt+Enter、Esc，各按 2–3 次，並記下畫面上實際發生什麼。
5. 按 Ctrl+K，記下是否開啟命令面板（修正前預期無反應）。
6. 設定 → Logs → 匯出，選擇「已清理」版本。
7. 回到設定關閉詳細紀錄。

紀錄只包含數字與按鍵名稱（例如 `alt+q`、`ESC 71`），不含終端輸出或一般輸入文字。事件名稱為 `herdr.terminal.diagnostics`，每 5 秒一筆。

## 要回傳的資料

- `yz-probe-windows.json` 與探針的完整終端輸出
- 匯出的已清理紀錄檔
- Windows 版本（`winver`）、鍵盤配置與輸入法、滑鼠／觸控板型號或類型
- 使用的 `herdr.exe` 來源（內附或 PATH）
- 步驟 B.4 中每個按鍵的實際畫面反應

## 已實作、待 Windows 驗證的修正（2026-09-24）

以下兩項不需要量測就能確定是成本，已先實作；下一次 Windows 測試時請一併確認效果：

- **F1**：每次 Herdr API 呼叫原本會啟動兩次 `herdr session list --json` 子程序並 ping 一次；現在在 1 秒內重用上一次的 Session 清單與 ping 結果。任何請求失敗、手動重新整理或 Session 啟停後會立刻失效；socket 路徑仍然只來自 `session list`。
- **F2**：Windows named pipe 等待回應時，原本每次固定睡到 100ms；現在改為 1、1、2、4、8ms 逐步拉長。事件訂閱通道維持 100ms。

- **F3（改）**：macOS 實測（2026-09-24）確認，原生 connector `terminal.scroll` 的 ack 約 1ms 就回來，會讓捲動指令排隊（5 秒 270 次指令，只回來 141 個 frame）。現在原生 macOS／Windows 在 pane API 可用時改走和 WSL 相同的 `pane.scroll`（使用者 A/B 實測最順）；不可用時退回 connector，並改成等 frame 回來才送下一次。Windows 請重點確認：捲動停手後畫面是否立刻停住。debug log 裡 `wheel.strategy` 應該是 `pane`，`pane.scroll` 的 p95 也要一併記錄。

仍依 Windows 證據決定的項目：F4（scrollbar 手勢後同步）、F5（full frame 依繪製合併），以及 Alt 組合鍵的送出方式（計畫 Phase 8）。

## 結果紀錄

| 日期 | 版本／commit | 執行者 | 結果摘要 |
|---|---|---|---|
| 2026-09-24 | 規劃中（macOS 參考） | agent | 見上方 macOS 參考結果 |

修正後（Phase 7、8）請用同一組步驟重測，並把前後對照填在這裡。
