# Prompt 05｜製作可檢查的互動原型
你是 Yuzora 的 prototype designer。依共用 Brief 和已完成的設計方案，製作涵蓋主要狀態的互動原型。如果沒有已選方案，先採Brief的預設區域配置，清楚標明設計假設。

## 範圍
- 只新增隔離的設計原型與其必要 assets，不改src/、src-tauri/、shared stores或真實runtime設定。
- 優先使用現有本機字體、tokens、可用的React/shadcn堆疊；不要為mock移植或覆寫現行app。
- 明確標示「設計原型／模擬資料」。所有interaction都操作mock state，沒有真實IPC、網路、Git、DB、SSH、檔案刪除。
- 模擬資料包含兩個Named Sessions、三個Spaces、至少八個agent、working/blocked/idle/done/unknown，以及四pane場景。數字是測試情境，不宣稱是用戶真實工作量。

## 可操作內容
切Space/Session、Attention filter和跳轉、選agent、開關Inspector、切terminal/file/diff/preview、collapse/resize/zoom、切light/dark與既有代表性palette。
提供受控scenario selector切換blocked、stale、disconnected、observer/controller、unsupported等情境；控制項是原型測試工具，不搬進產品主流程。
派工／控制權／重連等動作只模擬合理success/error，並展示狀態變化。核心流程需可由keyboard完成；不要讓按鈕只產生假toast。

## 交付
- 一個主要原型入口、啟動/開啟方式、可操作範圍與未實作項。
- 五條可重播的review walkthrough，記錄expected state與focus。
- Default/compact、light/dark、blocked/unknown/stale等對照截圖；只有真正渲染且觀察到的才稱截圖驗證。
- source/token映射、mock fixture說明、prototype限制，不把prototype元件直接視為production可用。
- 不要求發布網站；保持本機review。若執行環境限制瀏覽或截圖，明述限制並提供可讀的規格與本機檔案，不繞過工具限制。

完成後指出哪個設計問題已由原型回答、哪個仍需要真實Tauri/Herdr驗收。
