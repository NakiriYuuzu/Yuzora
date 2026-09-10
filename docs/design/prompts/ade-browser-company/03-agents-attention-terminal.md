# Prompt 03｜ADE 核心操作流程
你是了解 Herdr authority 與 terminal lifecycle 的產品設計師。請重新設計 Agents、Attention、Agent Inspector、terminal/pane 工作面。

## 必須設計的情境
1. 使用者同時查看多個 agent：working、blocked、idle、done（未查看）、unknown。
2. Agent 等待回覆：從 Attention 找到正確 Session/Space/pane，看到當下內容與適當的下一步。
3. 已回覆待查看：區分 seen/unseen；不能把 done 當工作正確或測試通過。
4. Observer 檢視 terminal：清楚表明唯讀、誰擁有控制權（僅在資料可得時）、要求控制與失敗後的狀態。
5. Pane 操作：split、focus、resize、zoom/還原、swap、close 的可發現入口與 keyboard alternative。
6. 斷線、stale snapshot、stopped server、unsupported capability、pane消失、agent被替換的處理。
7. 有能力時的 prompt/wait、attach/resume、explain；能力缺失時提供明確原因或可返回 terminal 的現有流程。

## 設計方式
- Agent row 先呈現可理解的名稱／目前工作上下文／狀態／下一步；cwd、revision、read source 等留在 Inspector 的進階詳情。
- Inspector 評估 docked panel 與 dialog 的取捨；持續對照 terminal 的任務避免被不必要的 modal 打斷。
- Terminal 仍是主要真實工作內容。不要把 raw terminal transcript 自動轉成聊天泡泡或編造思考過程。
- 狀態用文字＋形狀／圖示，不只用 color dot。模型／provider／Space 身份不能與狀態共用唯一訊號。
- Input 的 busy、pending、blocked、success、error、stale 和 focus restoration 要可追蹤。
- 不發明不存在的 agent.stop/restart API；任何中止／close 的提案須標出實際 backend target、ownership 與作用範圍。
- read-only Inspector、native agent session restore、terminal reconnect 分別設計，不混成同一個「繼續」按鈕。

## 輸出
- Agent row／Attention row／Inspector／pane header 的 anatomy 與狀態表。
- 每個 action 的 human label、可見條件、enabled條件、authority、回應、錯誤、可恢復方式。
- 狀態轉換與一條完整使用者故事：收到注意項→查看→回覆→等待→回到產物；沒有 backend 的步驟明標提案。
- 兩 Session、三 Spaces、八 agents、四 pane 的壓力情境 wireframe。
- 與現有 typed IPC/store 的對照表；只在規格中寫 API 名。
- 測試項目：prompt遇blocked不送入；agent替換不誤判完成；session切換不跨context輸入；使用者自行捲動時不強制跳到底部。

所有資料、輸出與事件若為 mock，都要清楚標示；不要操作 live Herdr。
