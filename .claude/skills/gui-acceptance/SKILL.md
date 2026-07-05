---
name: gui-acceptance
description: 對 yuzora app 執行實機 GUI 驗收／復驗——用 computer-use MCP 建立可控的 app、驅動 GUI 驗收 checklist、留存證據截圖並記錄結果。適用於 milestone 驗收、缺陷修復後復驗、watcher/tab/dialog 等 WebView 行為驗證（jsdom 測不到的部分）。
---

# yuzora GUI 驗收

用 computer-use MCP 實機操作 yuzora，執行 milestone 驗收 checklist。全程程序於 2026-07-02 M1 驗收（round 1 手動＋round 2 computer-use，8/8 PASS）實測驗證。

## 0. 前置檢查

- 驗收 checklist：讀 `docs/superpowers/checklists/` 下當前 milestone 的 checklist 檔，逐項執行。
- 測試 fixtures（gitignored，`fixtures/out/`）：若缺檔重新產生：
  ```bash
  bun run fixtures/gen-fixtures.ts
  bun run fixtures/gen-encoding-fixtures.ts
  cp src-tauri/icons/32x32.png fixtures/out/pic.png   # 二進位拒開測試用
  ```
- **絕對不要**用 repo 內被 Vite/Tailwind 掃描的檔案（README.md、src/**）做外部修改測試——會觸發前端 full-reload 毀掉 app 狀態。一律用 `fixtures/out/` 的檔案。

## 1. 建立可被 computer-use 控制的 app

`tauri dev` 跑的是裸 binary，computer-use 的 `request_access` 對它**永遠回 `not_installed`**。必須走 bundle：

1. `bun run tauri build --debug --bundles app`（Rust 已編譯時 ~30 秒）
2. `ditto src-tauri/target/debug/bundle/macos/yuzora.app /Applications/yuzora.app && open /Applications/yuzora.app`
3. `request_access(["yuzora"])`——若回 `not_installed`：**MCP server 的已安裝 app 清單是啟動時快取的**，LaunchServices/Spotlight 註冊都救不了，唯一解法是請使用者 `/mcp` reconnect computer-use 後重試（agent 無法自行觸發，直接開口請使用者做，不要反覆重試）。
4. 授權成功後 `open_application("dev.yuuzu.yuzora")` 帶到前景，screenshot 確認視窗可見。
5. 驗收結束提醒使用者刪除 `/Applications/yuzora.app`（debug 副本）。

dev server（port 1420）與 dev app 不需要動，兩個 instance 並存無衝突（log 用時間戳分辨）。

## 2. 若派 subagent 執行

- brief 內載明：授權已完成不要再 request_access、用 ToolSearch `computer-use` max_results 30 一次載入全部工具、禁止 git 寫入、不殺 process、結束不關 app。
- 螢幕有 native 過濾：非 allowlist app 顯示為黑色區塊——正常現象，寫進 brief 避免 agent 困惑，並告誡不要點擊黑色區域（可能誤觸其他 session 的視窗）。

## 3. GUI 驅動要領（實測驗證）

### 開 workspace（NSOpenPanel）
點「Open workspace」→ `key("cmd+shift+g")` 呼出前往檔案夾（不要逐層點側欄）→ `type("<絕對路徑>")`（預設全選會整段取代）→ `key("Return")` → 確認麵包屑 → **點右下角 Open 按鈕**（不要按 Return，焦點可能已跳走）→ 截圖核對 FileTree 與 `ls` 一致。

### Tab 操作
- FileTree 單擊即開持久 tab（非 VS Code preview 模式）。
- Tab bar 會水平溢出捲動——點 tab 的 × 之前**每次都重新截圖量座標**，不要沿用舊座標。
- dirty ● 是橘色小點（檔名右側、× 左側），用 `zoom` 窄範圍判讀。
- 對 tab bar `scroll` 後立即截圖確認視窗沒被移動，必要時 `osascript` 復位。

### 原生對話框（plugin-dialog confirm 等）
視窗層級原生 sheet：標題＋訊息＋按鈕（Cancel 左、OK 右，OK 藍色預設）。按鈕寬度隨語言變化，每次截圖＋zoom 確認文字後再點，不要憑座標猜。

### 等待時間
- 外部修改 → auto-reload：等 2 秒驗證生效，**再等 3 秒驗證不被蓋回**（stale-flush regression 是延遲出現的）。
- tooLarge/binary 特殊視圖：等 1 秒再截圖避免過渡態。
- watcher debounce 300ms、batch 最壞 600ms、saveSuppress 窗口 750ms——外部修改後 2 秒是安全下限。

### 焦點救援
1. menu bar 不是 yuzora → `open_application("dev.yuuzu.yuzora")`。
2. 重新截圖：確認 menu bar 名稱＋紅綠燈為彩色（彩色＝focused；灰階＝未 focus，是 macOS 標準行為非 bug——判斷燈號顏色前先點視窗）。
3. 視窗位置不對 → `osascript` `set position of front window to {x, y}` 復位。
4. 任何懷疑焦點跑掉的操作後：先截圖再繼續，不要盲目連點。

## 4. 證據截圖

**不要依賴 MCP `screenshot`/`zoom` 的 `save_to_disk`**——實測呼叫成功但檔案找不到。用本 skill 附的 `cap.sh`（osascript 查即時視窗 frame → `screencapture -x -R` 裁切）：

```bash
EVIDENCE_DIR=<scratchpad>/evidence bash .claude/skills/gui-acceptance/cap.sh vv-<round>-<item>-<state>
```

優點：真實色彩（合成層過濾會讓未授權區域變黑、未 focus 燈號灰階）、精準裁到 yuzora 視窗（不外洩桌面其他內容）、Bash 可直接讀。

**座標系警告**：osascript 回傳 points、computer-use 用截圖像素、`screencapture -R` 吃 points 輸出 Retina 2x 像素——三套不同，**不要互相換算**，每次讓 osascript 即時查 frame。

## 5. 結果記錄

1. 報告寫 `.superpowers/sdd/acceptance-verify-<n>-report.md`：每項 PASS/FAIL＋證據截圖路徑；FAIL 附精確重現步驟。
2. checklist 檔末尾追加「## 復驗紀錄（computer-use，日期時間）」節，逐項一行；既有內容一字不動。
3. ledger（`.superpowers/sdd/progress.md`）記一筆總結。

## 6. 已知陷阱速查

| 陷阱 | 對策 |
|---|---|
| `request_access` 回 not_installed | 見 §1——bundle→/Applications→請使用者 reconnect MCP |
| `save_to_disk` 不落地 | 用 cap.sh |
| 外部修改觸發 Vite full-reload | 只改 `fixtures/out/` 檔案 |
| 未 focus 紅綠燈灰階誤判 | 先點視窗再截圖 |
| tab 座標漂移 | 每次點 × 前重新截圖 |
| 共用桌面搶焦點（其他 session/WebStorm） | 焦點救援 checklist，操作間隨手截圖 |
| osascript `keystroke` 部分組合鍵失效 | 改用 `key code`（如 cmd+shift+z 用 key code 6） |
| GUI 背景啟動被 App Nap 節流 | Bash 用 run_in_background，不要 nohup |
| 切 tab 後 undo 歷史消失 | 設計取捨（write-through），不是 FAIL |
