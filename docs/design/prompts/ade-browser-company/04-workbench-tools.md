# Prompt 04｜整個 Workbench 的一致性
你是 Yuzora 的產品設計師。請讓 Editor、Git、Database、SSH/SFTP、Preview、Worktree、Settings 延續同一個 ADE 工作面和 The Browser Company 視覺語言。

## 每個子系統都要覆蓋
- Editor/LSP：files/search/symbols、tab與split、dirty/save/conflict、diagnostics；editor內容維持CodeMirror。
- Git：status、diff、history、staging/commit、branch與conflict；保留既有操作與安全提示，不增加自動提交。
- Database：profile/connection、schema、query/results、cancel、錯誤；保留DB權限與安全流程，不把SQL操作接成自動agent action。
- SSH/SFTP：host identity、session、remote path、transfer progress/error；明確區分本機與遠端。
- Preview：與對應Space/文件/URL的關聯、導航、載入/錯誤、native webview bounds與focus。
- Worktree：inventory/provenance之外，評估create/open/remove提案；列出backend依賴、canonical path、branch/base與dirty處理。
- Settings：Appearance、Editor、LSP、Terminal、Herdr、Plugins/Integrations、Preview、Safety、Git、About；沿用現有頁面與入口，新增分類要解釋。
- Onboarding/empty：無Space、無agent、未連線、缺capability；每個情境只有清楚且可完成的主action。

## 一致性要求
保留暖色與霧面chrome；code、diff、logs、grid的內容層有穩定可讀性。共用header/action/toolbar/error/pending模式，但不要把editor、terminal、DB grid硬塞成相同卡片。
從agent轉到diff／file／preview時僅使用真正可用的關聯；沒有artifact metadata就顯示可選的相關檢視，不猜檔案清單。
Inspector與side panel是否共用欄位要依上下文定義，切換後能回到先前內容與位置。不同工具保留獨立selection和scroll ownership。

## 輸出
1. 子系統×主要任務×入口×主action×狀態×返回目的地矩陣。
2. 至少Editor/Git/Database/SSH/Preview五張主要畫面規格；不只交一張首頁。
3. Worktree與Plugins/Integrations專用提案：現有支援、視覺整合、新capability分開。
4. 共用元件清單與domain-specific元件理由；對照shadcn registry/現有UI。
5. 新舊入口對照與功能保留清單，明確指出任何建議移動或合併的控制項。

不連線真實DB／SSH、不修改檔案、不執行Git或worktree mutation；本輪只設計。
