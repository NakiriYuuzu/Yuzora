# Prompt 06｜重設計實作交接
你是熟悉Yuzora的frontend architect。把共用Brief、設計規格與原型整理成可review的實作計畫。本prompt本身只產出計畫；不修改產品程式碼、不建立commit/PR。

## 調查
重讀live CLAUDE/AGENTS/CONTEXT/ADR、HEAD與dirty state。Code discovery先用codebase-memory，未索引先index；coverage不夠的source與design docs直接補讀。
只讀與設計slice相關的AppShell/workbench/panels/styles、typed IPC、Bridge、store、i18n。不要做無關重構。

## 切片方式
1. 現有tokens和共享chrome的樣式調整。
2. Shell/導航與視覺層級，保存現有selection和pane/process身份。
3. Agents/Attention/Inspector與pane入口的現有能力重組。
4. Editor/Git/Database/SSH/Preview的外殼一致性。
5. 需要新能力的跨session聚合、agent actions、worktree、通用plugins，分開列為後續工程，不能藏在CSS重設計裡。

## 每個slice必填
- 使用者行為的before/after，對應設計spec與reference。
- 受影響檔案、元件、store選擇器、IPC與i18n keys；新抽象必須有具體需求。
- 重用的shadcn元件。缺primitive時先registry/docs調查；domain chrome需說明理由。
- 明確不變的runtime/ownership/焦點/捲動邊界。
- 相依項、風險、可回退方式、必要的meaningful tests與GUI check。
- 完成後如何證明既有功能與資料沒有遺失。

## 驗證策略
依slice選lint/typecheck/build與相關existing tests；避免寫只鏡像實作的測試。行為改動需有失敗情境及recovery驗證。
真實GUI項包括terminal不因切頁remount丟失、observer/controller、webview overlay、CodeMirror/xterm焦點與shortcut、light/dark/zoom、ScrollArea和keyboard。
只改前端樣式不自動要求完整DB／Rust測試；若觸碰Rust再依CLAUDE執行對應checks與exact Clippy baseline。
有ADR衝突就明確說明需要決策，不自行覆寫。後續使用者選定並授權slice後才實作，屆時直接完成已授權範圍，不對每個可逆小改動反覆請示。

## 交付
分階段工程計畫、檔案責任表、現有vs新增backend能力矩陣、驗收清單。不要把新設計提案寫成已完成或已批准的產品事實。
