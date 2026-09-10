# Prompt 02｜ADE Shell 與資訊架構
你是桌面生產力工具的 interaction designer。請重整 Yuzora shell，保留 The Browser Company 風格，讓使用者自然理解「在哪裡工作、誰需要我、接下來能做什麼」。

## 設計範圍
- Spaces rail、Space header、Named Session selector、Command Palette、ADE navigation、main canvas、contextual inspector、按需 terminal drawer、status bar。
- 以既有 rail/側欄為預設探索起點；決定哪個層級需要常駐，哪些可收合／漸進揭露。
- Space 名稱、branch/worktree、Named Session／runtime 的關係要清楚。長路徑與 protocol/version 放在可查閱的位置，不霸占日常工作區。
- 支援「專注單一 agent」與「掃描整個 Space」兩種工作需要；多 agent 監督使用清楚列表與 attention，不加沒有資料來源的 KPI。
- 跨 Named Session 的待處理數與 stale 標記若需新資料流，列為依賴；不可把別的 Session 的 cached state 當即時。
- 切 Space、Named Session、工具頁、close panel、返回 terminal 的行為、保存內容與 focus destination 都要明定。
- Command Palette 按可用 context 呈現操作與原因，沿用已存在快捷鍵；新增鍵位先確認 OS、editor、terminal 衝突。

## 輸出
- 區域圖：1440×900 default、1024×768 compact、sidebar/inspector collapsed。
- Navigation tree 與 route/selection/state 邊界對照；不任意把 UI tab 當 Herdr Tab。
- 至少六條操作流程：開啟 Space、切 Named Session、查看 blocked、切 Git diff、回到原 terminal、處理連線中斷。
- 每條 flow 的 before/action/after、focus、back/escape、context 保留與失敗狀態。
- Resize 與折疊優先順序；200% zoom 下核心 action 的保留方式。
- 與現行 AppShell/WorkspaceRail/HerdrNavContent/CommandPalette 的映射和變更範圍。

## 完成條件
工具入口可找到，主要工作畫布不被多層 chrome 擠壓；鼠標與鍵盤都能完成常用流程；不要引入新的持久化任務引擎來解決純導航問題。
