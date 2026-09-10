# Yuzora Mobile · 概念原型

此目錄僅由 `output/ui-rollout/harness.tsx` 的 `?device=phone` 模式載入，production App 不載入手機元件或樣式。

目前入口：`http://127.0.0.1:4321/output/ui-rollout/index.html?device=phone&theme=light&accent=violet`。

- 375×812、390×844、430×932 CSS 畫板；窄於 480px 時使用目前 viewport。
- 底部「工作 / Work、Agents、工具 / Tools、設定 / Settings」。保留 Space／branch／Herdr Session。
- 兩個 Herdr Session 範例、兩個 worktrees、八個 Agents；working／blocked／idle／done／unknown 各自呈現，Docs Session 顯示 stopped。
- 一次顯示一個 pane；切 pane 保留各 Agent 的輸入草稿；單欄 Diff、文件與 Preview 示意。
- 設定中的 Web Service 提供啟動、停止、模擬 App 開啟時自動啟動、配對與電腦授予／釋放控制權。
- 只有模擬服務啟動、裝置已配對、正常連線且有控制權才可輸入；unknown／stale／unsupported／error／stopped 阻擋輸入。
- 模擬輸入僅儲存在元件記憶體，顯示於所屬 pane。沒有 IPC、HTTP、WebSocket、PTY、SQL 或 Git mutation。
- 英文與繁體中文可於畫板上方切換；一般操作與正文 13–16px，metadata 11–12px，主要手機按鈕 40–44px。
- 沿用 shadcn Button、Tabs、ToggleGroup、Select、Switch、Field、Textarea、ScrollArea，以及現行 palette、字體、SpaceCharacter。

## 後續真實 Web Service 的介面邊界（未實作）

Desktop Settings 應提供服務啟動／停止、可選的隨 App 啟動設定及已配對裝置清單。啟動後顯示真實可連線位址與一次性配對方式；目前 `https://your-mac.local:port` 僅為示意，不可連線。

後續須實作服務身分驗證、電腦端批准與撤銷配對、TLS／信任建立、Origin 檢查、受驗證連線的 typed Bridge、帶有 runtime scope 的 IDs，以及與 Herdr 相容的 observer／controller 權限和斷線恢復。手機頁面關閉不停止 Herdr process；停止服務應撤銷該服務的控制連線。不可直接把 Tauri invoke 暴露成任意遠端 API。

## 驗證

本輪使用一般 Vitest＋jsdom 的 component tests 檢查配對／控制 gating、停止服務撤銷輸入、草稿按 Agent 隔離、自動啟動模擬。這些不是 browser E2E。

依使用者的 AGENTS.md，browser／原生 GUI 自動化需要明確同意；目前未取得同意，所以未做真實 viewport、觸控、鍵盤彈出或 Native double-click 驗收。
