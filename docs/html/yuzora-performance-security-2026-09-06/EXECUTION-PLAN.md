# 55 項改善與多 database／schema：平行開發及整合相依

評估日期：2026-09-06。狀態：**派工設定已備妥；待使用者明確開始指令，尚未啟動開發代理**。來源 HEAD：`9d0634c44e314b3dcc1c39b371ed448481b2f16a`。

原稽核 55 項全部納入；新增 DBM01–DBM04 是功能能力標籤，不增加已確認漏洞數。此文件安排工程工作，不授權 Git 寫入、發布或外部遠端測試。

共 44 個工程工作包（含契約／整合職責）與 5 個聯合驗收 gate；涵蓋 55/55 原項及 4/4 新能力。所有 task 都是 planned。

機器可讀版本：[execution-plan.json](execution-plan.json)。主報告：[HTML](../yuzora-performance-security-2026-09-06.html#execution)。本檔由 render-report.ts 產生；修改 execution-strategy/database/runtime/frontend.json 後重新產生，避免手改衍生檔漂移。

## 啟動控制與指定模型

目前：**standby／待命**。使用者在本任務明確下達開始／開工指令後才 spawn 開發代理；本次準備訊息不是啟動授權。

開發：最多 3 個 `gpt-5.6-sol`／`medium` worker。最終 review：`gpt-6-astra`／`xhigh`。

主代理負責排程、ownership、共用檔案串行接線與驗收；不另行改變主代理模型。

### 收到指令後的派工前置

- 收到開始指令後重新讀取 CLAUDE.md、live worktree diff 與檔案 ownership；目前 host/remote 架構仍有大量他人變更，不直接沿用舊快照派工。
- 以 codebase graph／live source 重新對齊已搬移模組，更新 affected task paths 與依賴；他人變更若已涵蓋部分改善，先驗證再記錄處置，不重複實作，也不逕自標示完成。
- 保存本輪起始變更清單與測試基準，區分既有／他人變更和本輪實作；必要檔案交接依 ALIGN-HOST 執行。
- 建立新的 Sol medium worker，明帶工作包、唯一 ownership、契約、完成 gate 與不得還原他人變更；不把舊分析代理當作已符合指定模型的開發代理。

- 同時最多三名開發 worker（目前共四個 active slots，保留一個主代理）；依 requires／mockAfter 和目標 writer 動態補位。
- 先挑可獨立且有價值的安全／正確性工作，再推進契約與效能工作；共用檔案僅由指定 writer 接線。
- 每個 worker 必須回報修改路徑、測試證據、未完成項與風險；主代理驗證後才更新工作包狀態。
- 依既有規則排程重型 build／benchmark；不能因增加代理而同時污染效能量測。

### 最終 review 啟動條件（全部滿足）

- 44 個工作包均完成其交付與 gate；55 項原清單及 DBM01–DBM04 均有實作／量測處置證據，沒有 pending、blocked 或僅 mock 的未完成範圍。
- G-DB、G-SSH、G-LSP、G-ASSET、G-ALL 全部通過；缺少平台／環境驗證不能寫成已完成。
- 所有實作 worker 停止寫入，凍結待審改動與驗證結果快照；若他人同時變更相同範圍，先對齊並重驗受影響項，再進入 review。

最後才 spawn Astra xhigh，以唯讀方式審查本輪最終實作、相關未追蹤新檔及必要相鄰程式碼；檢查正確性、安全、ownership、並行／取消、記憶體回收、多 database/schema、相容性、遷移與測試盲點。不得把原先已存在的他人變更誤算成本輪改動。

每項附嚴重度、精確位置、觸發條件與修正建議。確認缺陷交回 Sol medium 修正；完成相關回歸後，再由 Astra xhigh 核對。不得提前宣稱全部完成或 review 通過。

仍不包含 Git stage／commit／push、建立 Git worktree、發布或使用真實外部帳密；本次只儲存待命設定。

以下是未執行的 spawn 參數模板；task_name 與完整 bounded 工作訊息須在實際派工時提供。

```json
{
  "development": {
    "model": "gpt-5.6-sol",
    "reasoning_effort": "medium",
    "agent_type": "worker",
    "fork_turns": "none"
  },
  "finalReview": {
    "model": "gpt-6-astra",
    "reasoning_effort": "xhigh",
    "agent_type": "default",
    "fork_turns": "none"
  }
}
```

## 如何閱讀相依關係

- **可立即準備／開發**：沒有其他工作包的程式交付前置；先確認 ownership。效能路徑變更前仍須留下 BASE 所定義的對照資料。安全修補不必等待全專案 benchmark。
- **契約後可平行**：先固定 typed DTO、owner、錯誤與 lifecycle fixtures，再用 mock 開發獨立演算法、adapter 或 UI；mock 通過不等於 production 整合完成。
- **交付硬相依 requires**：正式交付／啟用前必須通過的前置工作包。可以先寫測試或草擬介面；若有 mockAfter，可在該契約完成後提前開發。
- **檔案互斥**：同一檔案一次只有一位 writer；這是編輯與接線衝突，不自動構成功能先後依賴。其他人透過獨立模組、props、adapter 與測試 fixture 平行工作。
- **聯合 gate**：組合真正 backend、frontend 及故障情境驗證。多個前置以 AND 解讀，不是完成其中一個即可。

## 已接受的行為與預設

- 維持 PostgreSQL、MSSQL、SQLite。HostProfile→DatabaseDescriptor→ConnectionIdentity→schema；schema 不新增連線，PG/MSSQL database 各有獨立 actor。SQLite ATTACH 保持同 session，不自動重播。
- Schema 選取只影響瀏覽／補全，產生 SQL 使用限定名稱。手寫 SQL 的 context 變動只讀回並標示 fresh/stale/unknown，不暗中 SET search_path、USE、commit 或 rollback。
- RetainedResultOwner 與 LiveCursorOwner 分離：每 DB current result 即使離線或未選中仍 pin；已保留頁與 cell 可唯讀。下一頁讀 DB、cancel 必須有 live exact owner；舊 snapshot 不可操作新 actor。
- 記憶體壓力只淘汰 inactive cache；全部 pin 時暫停 producer 或拒絕／延後新 admission。淘汰回 pageEvicted/cellEvicted；明確清除回 resultReleased；禁止重跑 SQL、強制 disk spool 或靠強制 GC 回收仍被引用的物件。
- Preview 使用獨立、有界結果 bucket 與 captured descriptor/object/revision；busy、open cursor 或未提交交易時拒絕，保留 SQL 草稿與目前結果。
- 每個舊 network profile 遷為自己的 host＋DB child，保留 descriptor ID；不因 hostname 相同而合併，不重寫 vault 密碼，保留 credential recovery。原明文例外不延伸到新 database。
- SFTP 無 atomic-overwrite 能力時保留原檔並要求其他檔名；使用經驗證 explicit target leaf，final promote 再處理競態，不能先刪除舊檔。
- LSP 取得 byte credit 並入列才算 accepted；已 accepted 保持 FIFO，未 accepted 每文件只保留最新版本引用。壓力恢復後按協商模式補送，單純壓力不 restart。

| 項目 | 初始預設（待同條件量測） |
|---|---|
| DB page/cell | 500 rows／2 MiB retained 軟目標，合法大列獨立頁；保留 1 MiB field／8 MiB row。頁面儲存起始 ordinal；文字 preview 4 KiB、binary 256 bytes。 |
| DB cache/admission | 64 MiB/result session、256 MiB/app registry；metadata 8 MiB/DB、32 MiB/app、200 items／1 MiB/page、TTL 5 分鐘。4 個執行／取頁、2 metadata、2 connect、8 network helpers；每 DB 一執行一待執行，metadata queue 32。 |
| DB deadline | SQL wall-clock 預設關閉；connect 15 秒、metadata 30 秒。Heartbeat 每 5 秒、120 秒無 helper liveness 判失聯；assembly timeout 獨立。控制 frame 不受 data credits 堵塞，idle cursor／transaction 不自動取消。 |
| SSH/SFTP | input 1 MiB/session；output 4 MiB/session、32 MiB/app；transfer 4/session、8/app、queue 1000；chunk ≤128 KiB 且符合 negotiated cap，window 起始 8，progress 100 ms。 |
| LSP | inbound/outgoing frame 64 MiB；queued＋in-flight 72 MiB/server、128 MiB/app，前端待送資料也納入計量。 |
| Search/Preview/log | Search 起始 2 workers，每段 1 GiB 或 30 秒，保留現有結果配額；Preview 4 workers／queue 32／16 sessions；日誌 10 MiB/segment、100 MiB 合計含 active、14 天，writer queue 4 MiB。 |

## 本輪發現的進行中變更與交接

唯讀快照：2026-09-06T09:56:48.762Z。本輪文件生成期間的唯讀 git diff 與 live source 快照；不是持續監控。

其他工作者正在抽出 host crate 與 runtime identity。path_capability 原實作已移至 host/src，desktop 檔案成為 facade；sftp_edit 已提供 raw session／posix-rename 路徑，不能再按舊稽核位置寫第二套 adapter。

所有 ownership 表是交接後的目標分工；交接前維持現有工作者 writer。SH-C 與 RT-FSP 的正式交付需 ALIGN-HOST；SH-F 純 F10 queue 內部工作與 FE-Herdr 目前 runtime 內的最佳化可先行，接線再對齊。

受影響工作包：SH-C、SH-A、SH-B、SH-F、RT-FSP、INT、FE-Herdr。

- `src-tauri/Cargo.toml`
- `src-tauri/Cargo.lock`
- `src-tauri/src/lib.rs`
- `src-tauri/src/ssh_service.rs`
- `src-tauri/src/path_capability.rs`
- `src-tauri/src/file_content.rs`
- `src-tauri/src/host_service.rs`
- `src-tauri/src/sftp_edit.rs`
- `src-tauri/host/`
- `src/lib/herdrNormalize.ts`
- `src/lib/herdrNormalize.test.ts`
- `src/lib/runtimeIdentity.ts`
- `src/lib/runtimeIdentity.test.ts`

## 分工與派工

- 模型與啟動：等待明確開工指令；開發使用 gpt-5.6-sol / medium，全部工作與驗收完成後才 spawn gpt-6-astra / xhigh 做最終 review。
- 本輪已發現其他工作者修改 host/capability/SSH/Cargo/identity；派工前重新讀取 diff，依 ALIGN-HOST 完成交接。不得把稽核時的舊檔案位置當成目前實作位置。
- 預設一位整合者＋最多三位實作 worker；這是目前協作容量，不是 runtime concurrency 設定。
- 每次派工寫明 task ID、唯一檔案 ownership、允許修改的 adapter/module、所需契約版本、驗收 gate；先檢查 live worktree 與其他工作者 ownership。
- 有 requires 未完成且無 mockAfter 的工作，只做不依赖該實作的 fixture／設計準備；有 mockAfter 者依契約先行，不將 mock 狀態寫成已完成。
- shared-file writer 持短時間接線窗口；不要為了平行而做全專案無關模組拆分。所有人不得還原其他人的變更。
- 本評估不建立 Git worktree、不 stage／commit／push。若日後使用隔離 checkout，shared protocol／dependency 相容性 gate 仍存在。
- DAG 是技術與整合順序，未有工時數據，不能據此宣稱精確 critical path、完成日期或加速倍數。

### 可立即分流

DB-S TLS 修補、DB-A accounting、RT-GIT、獨立 editor/document/HERDR 工作可同時開發。

實際只挑 3 個 worker；其餘排隊。共用檔案有單一 writer，相關效能修改先保存對照資料。

### DB 契約完成後

DB-R 結果核心、DB-B 傳輸、DB-H profile/discovery 可平行；DB-UR／DB-UN 可先用 fixtures 做 UI。

db_service、db_query_worker、dbStore 的變更經指定 owner 接線；DB-R 等 DB-A，新 UI 正式啟用等各 backend AND gate。

### SSH 契約完成後

SH-A adapter、SH-B backend mock、SH-F frontend mock 可平行。

先通過 ALIGN-HOST，重用已出現的 raw／atomic-save 邊界；ssh_service.rs 只有 SH-B writer。真 transfer/promotion 仍需 SH-A，不另外並行建第二套 adapter。

### 跨子系統補位

RT-LOG、RT-FSP、RT-PROC、RT-PERF、前端 tree/search 等無衝突工作作為空閒 slot 候選。

LSP writer＋前端可依 LS-C 平行；logging writer→rotation→cursor、search quota→parallel enable 保留內部順序。

## 工作包與交付 DAG

requires 是正式交付前置；mockAfter 是可提前開發的契約門檻。表內存在前置不代表下游必須空等；也不代表共享檔案可以多人同寫。

| 工作包 | 範圍 | 方式 | 正式交付前置（AND） | 可先 mock 的契約 |
|---|---|---|---|---|
| BASE 固定對照資料與驗收矩陣 | — | 可獨立開始 | — | — |
| ALIGN-HOST 對齊進行中的 host／identity 邊界與檔案交接 | — | 可獨立開始 | — | — |
| INT 共用邊界與退出整合 | — | 可獨立開始 | — | — |
| DB-S PostgreSQL TLS 降級修補 | SEC01 | 可獨立開始 | — | — |
| DB-C DB 跨層契約與測試 fixtures | D03、D04、D06、D08、D09、D11、D12、DBM01、DBM02、DBM03、DBM04 | 可獨立開始 | — | — |
| DB-A 增量結果記憶體計帳 | D01 | 可獨立開始 | — | — |
| DB-R 有界頁面與可離線讀取的結果快照 | D04、D06、DBM04 | 契約後可平行 | DB-C、DB-A | DB-C |
| DB-B 有界資料流與獨立控制通道 | D02、D12 | 契約後可平行 | DB-C | DB-C |
| DB-BT Helper 批次傳輸量測與選型 | D10 | 契約後可平行 | DB-B、DB-A | DB-C |
| DB-T 連線交易與實際執行 context | D03、DBM03、DBM04 | 契約後可平行 | DB-C | DB-C |
| DB-H Host profile 與多 database 生命週期 | DBM01、DBM02 | 契約後可平行 | DB-C | DB-C |
| DB-N 有界多 schema metadata | D08、DBM03 | 契約後可平行 | DB-C、DB-H | DB-C |
| DB-Q 排隊、去重與保留工作的資源 admission | D09、D12、DBM02、DBM03 | 契約後可平行 | DB-C、DB-R、DB-B、DB-T | DB-C |
| DB-UR 結果表格、cell viewer 與独立 Preview | D05、D04、D06、DBM04 | 契約後可平行 | DB-C、DB-R、DB-T | DB-C |
| DB-UN Host、database、schema 導覽與獨立 console | D08、DBM01、DBM02、DBM03 | 契約後可平行 | DB-C、DB-H、DB-N、DB-Q | DB-C |
| DB-E 保持順序的 script 進度與提早結果 | D11 | 契約後可平行 | DB-C、DB-R、DB-B、DB-Q | DB-C |
| DB-SQ SQLite idle cursor 與 worker 策略 | D07 | 契約後可平行 | DB-C、DB-R、DB-Q | DB-C |
| SH-C SSH/SFTP session、credit、transfer 與覆寫契約 | S01、S02、S03、S04、S05、S06、S07、S08、S09、S10、S11、S12 | 交付有前置 | ALIGN-HOST | — |
| LS-C LSP accepted-version、byte admission 與停止契約 | R09 | 可獨立開始 | — | — |
| LG-C Log segment、cursor 與 trace privacy 契約 | R04、R05、R06、H02 | 可獨立開始 | — | — |
| SH-A SFTP extension 與有界 transport adapter | S02、S03、S04、S05、S06、S12 | 契約後可平行 | SH-C | SH-C |
| SH-B SSH/SFTP backend lifecycle、scheduler 與資料完整性 | S01、S02、S03、S04、S05、S06、S09、S10、S12 | 契約後可平行 | SH-C、SH-A | SH-C |
| SH-F SSH/SFTP frontend 狀態一致性、歷史與 terminal queue | S01、S02、S03、S07、S08、S11、F10 | 契約後可平行 | SH-C、SH-B | SH-C |
| RT-GIT Git 全生命週期期限、輸出預算與搜尋分頁 | R07、R08、R15 | 可獨立開始 | — | — |
| RT-LSP LSP bounded writer、停止與 trace producer | R09、H02 | 契約後可平行 | LS-C、LG-C | LS-C |
| RT-LOG Logging 單 writer、rotation、retention 與 cursor | R04、R05、R06、H02 | 契約後可平行 | LG-C | LG-C |
| RT-FSP 有界檔案讀取與 Preview 併行/回收 | R10、R11、R12 | 交付有前置 | ALIGN-HOST | — |
| RT-PROC Process 輸出有界 parser、tail 與事件 batching | R13 | 可獨立開始 | — | — |
| RT-PERF Perf 單次取樣與 terminal high-water 回收 | R14、F12 | 可獨立開始 | — | — |
| FE-Dirty 輸入 dirty 狀態 no-op 與 structural sharing | F01 | 可獨立開始 | — | — |
| FE-TREE-C 檔案樹 cache 與虛擬列契約 | — | 可獨立開始 | — | — |
| FE-TreeCache 檔案樹有界 cache、跨批次 single-flight 與取消失效 | F03、F04 | 契約後可平行 | FE-TREE-C | FE-TREE-C |
| FE-TreeView 檔案樹可見列 flatten 與虛擬化 | F02 | 契約後可平行 | FE-TREE-C | FE-TREE-C |
| FE-Document 文件讀取 single-flight 與 registry 生命週期 | F05 | 可獨立開始 | — | — |
| SEARCH-C Workspace search 取消、預算與增量事件契約 | — | 可獨立開始 | — | — |
| SEARCH-Core Workspace search 內層取消與資源預算 | R01、R03 | 契約後可平行 | SEARCH-C | SEARCH-C |
| SEARCH-Parallel Workspace search 有界平行 walker 與 worker | R02 | 契約後可平行 | SEARCH-C、SEARCH-Core | SEARCH-C |
| SEARCH-View Workspace search 增量 grouping 與虛擬化結果 | F06 | 契約後可平行 | SEARCH-C、SEARCH-Core | SEARCH-C |
| FE-LSP Semantic tokens 與 diagnostics 請求取消 | F07 | 契約後可平行 | LS-C | LS-C |
| FE-Herdr HERDR snapshot structural sharing 與 ID maps | F11 | 可獨立開始 | — | — |
| FE-Load 功能與語言 lazy chunks | F08 | 可獨立開始 | — | — |
| FE-Markdown Markdown worker、latest-wins 與預覽預算 | F09 | 可獨立開始 | — | — |
| FE-Build Tailwind 建置 profiling 與明確 source 範圍 | F13 | 可獨立開始 | — | — |
| FE-CSP Main WebView CSP 與 capability 邊界加固 | H01 | 可獨立開始 | — | — |
| G-DB 多 database／schema 與結果壓力聯合驗收 | SEC01、D01、D02、D03、D04、D05、D06、D07、D08、D09、D10、D11、D12、DBM01、DBM02、DBM03、DBM04 | 聯合驗收 | INT、DB-S、DB-C、DB-A、DB-R、DB-B、DB-BT、DB-T、DB-H、DB-N、DB-Q、DB-UR、DB-UN、DB-E、DB-SQ | — |
| G-SSH SSH 互動與 SFTP 真實傳輸聯合驗收 | S01、S02、S03、S04、S05、S06、S07、S08、S09、S10、S11、S12、F10 | 聯合驗收 | INT、SH-C、SH-A、SH-B、SH-F | — |
| G-LSP LSP 一致性、取消、隱私與退出聯合驗收 | R09、F07、H02 | 聯合驗收 | INT、LS-C、LG-C、RT-LSP、RT-LOG、FE-LSP | — |
| G-ASSET production chunks、worker、CSP 與 CSS 聯合驗收 | F08、F09、F13、H01 | 聯合驗收 | INT、FE-Load、FE-Markdown、FE-Build、FE-CSP | — |
| G-ALL 完整回歸、量測與交付 | — | 聯合驗收 | BASE、ALIGN-HOST、INT、DB-S、DB-C、DB-A、DB-R、DB-B、DB-BT、DB-T、DB-H、DB-N、DB-Q、DB-UR、DB-UN、DB-E、DB-SQ、SH-C、LS-C、LG-C、SH-A、SH-B、SH-F、RT-GIT、RT-LSP、RT-LOG、RT-FSP、RT-PROC、RT-PERF、FE-Dirty、FE-TREE-C、FE-TreeCache、FE-TreeView、FE-Document、SEARCH-C、SEARCH-Core、SEARCH-Parallel、SEARCH-View、FE-LSP、FE-Herdr、FE-Load、FE-Markdown、FE-Build、FE-CSP、G-DB、G-SSH、G-LSP、G-ASSET | — |

### 必须依序完成的重點

- DB-A 增量 accounting → DB-R page/pin/snapshot；DB-B 控制／背壓 → heartbeat timeout 啟用與 DB-BT 正式量測；DB-R＋DB-B＋DB-T → DB-Q 壓力處置啟用。
- SH-C → SH-A 真 adapter → SH-B 真 transfer/promotion；SH-F 與 SH-B 可以在 SH-C 後先 mock 平行，最後才共同通過 G-SSH。
- LG-C → writer ownership → rotation/retention → cursor 跨清理驗收；同 logging.rs 由一位 writer 完成，cursor fixture 可先行。
- Search quota/cancel/settlement → 平行 workers 正式啟用；tree generation 防晚到回填 → cache eviction 啟用。
- LS-C → LSP backend 與 frontend 平行 → G-LSP；production chunks／worker／CSS 與 CSP 各自可開發，最後 AND 驗收 G-ASSET，不互相建立循環相依。
- 各 service bounded stop → INT 全 app shutdown 整合 → 故障注入；log 最後 drain，不能為縮短工期跳過退出 gate。

## 每包交付與驗收

### BASE — 固定對照資料與驗收矩陣

- 負責角色：整合／量測負責人；狀態：planned。
- 交付：固定 source revision、fixture hash、指標定義、cold/warm 條件、55 項及新增能力追蹤；依受測子系統先留下修改前資料。
- Gate：每項有測試情境與對照條件；沒有量測的數字明確標示未量測，不把既有 92 tests 當成本次實作驗收。
- 相依：—；先 mock：—。
- 責任範圍：契約／驗收文件；不宣稱取得產品共享檔案寫入權。
- 協作注意：基準量測獨占測試主機；安全修補可先保留可重現 fixture，不等待全部效能量測。

### ALIGN-HOST — 對齊進行中的 host／identity 邊界與檔案交接

- 負責角色：INT 與現有 host／identity 工作負責人；狀態：planned。
- 交付：核對最新 diff、真實 capability／file-content 所在地、host protocol 與 SFTP raw／reservation 介面；記錄可接手的穩定契約與唯一 writer。重用 sftp_edit 的 raw/posix-rename 能力，抽成單一 adapter，不另建競爭實作。
- Gate：新 host source 與 desktop facade 對應完整；SFTP atomic-save／destination reservation、session generation、HostState shutdown 和 normalization identity 均有交接 fixture。未交接前不覆寫其他工作者檔案。
- 相依：—；先 mock：—。
- 責任範圍：契約／驗收文件；不宣稱取得產品共享檔案寫入權。
- 協作注意：只對齊契約與 ownership，不要求整個 host migration 完成；Git、logging、LSP、process、perf 等獨立工作不需等待。

### INT — 共用邊界與退出整合

- 負責角色：整合負責人（單一 writer）；狀態：planned。
- 交付：按子系統逐次接線 DTO、commands、dependency 與 capability；集中 owned-process 終止能力，最後整合全 app bounded shutdown。
- Gate：跨層型別一致；stop idempotent；控制操作可達；共同 10 秒 shutdown 期限內收斂並回報失敗，log 最後有界 drain；僅停止 owned processes/connectors，clean marker 不提前寫。
- 相依：—；先 mock：—。
- 責任範圍：`src-tauri/src/lib.rs`、`src/lib/ipc.ts`、`src/lib/types.ts`、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock`、`src-tauri/src/path_capability.rs`、`src-tauri/src/process_kill.rs`、`.github/clippy-baseline.json`
- 協作注意：持續性整合職責，不必等全部工作完成才第一次接線。各 service stop API 可先 mock；production shutdown 必須等對應 bounded stop 實作後驗收。

### DB-S — PostgreSQL TLS 降級修補

- 負責角色：DB安全修補者；狀態：planned。
- 交付：加密模式明確 Require、已授權明文模式明確 Disable，補完整協議負面回歸。
- Gate：加密模式收到 SSL 拒絕後不送 Startup/Password；正常 TLS、明文例外與憑證確認流程仍正確。全部 network 功能最終驗收須包含此 gate。
- 相依：—；先 mock：—。
- 責任範圍：`src-tauri/src/db_service.rs`、`src-tauri/src/db_query_worker.rs`、`src-tauri/tests/database_integration.rs`
- 協作注意：可立即獨立交付，不等待 DB 重構，也不讓每個工作包硬依賴它。既有稽核已復現漏洞；本規劃未實作修復或新增、執行修復後 loopback 回歸。

### DB-C — DB 跨層契約與測試 fixtures

- 負責角色：DB契約整合者；狀態：planned。
- 交付：凍結 Host/DB child identity、metadataRevision、snapshot/cursor owner 分離、有界 page/cell、progress/heartbeat 與 resourceWaiting 契約，提供可共享 fixtures。
- Gate：Rust/TS 契約一致；明列舊 descriptor 相容、離線 cell 讀取、過期 owner、pin-full、忙碌 Preview 與兩 DB 獨立狀態案例。
- 相依：—；先 mock：—。
- 責任範圍：`src/lib/types.ts`、`src/lib/ipc.ts`、`src-tauri/src/lib.rs`、`src-tauri/src/db_service.rs`、`src-tauri/src/db_query_worker.rs`
- 協作注意：只凍結必要介面，避免先做全面重構；契約通過後各包可用 mock 平行開發，production 相依仍須完成。

### DB-A — 增量結果記憶體計帳

- 負責角色：DB結果核心實作者；狀態：planned。
- 交付：以增量 page/session/registry retained bytes 取代每列重掃，保留原子預留與配置回滾。
- Gate：既有 15 項結果預算與 ownership 測試不退步；比較 5k/10k/20k 列、1/4 connections 的新增列與鎖等待成本。
- 相依：—；先 mock：—。
- 責任範圍：`src-tauri/src/db_result_session.rs`
- 協作注意：可與 DB-S、DB-C 立即平行；先保持公開行為，落地後再由 DB-R 改頁面與快取生命週期。效能改善須另量測。

### DB-R — 有界頁面與可離線讀取的結果快照

- 負責角色：DB結果核心實作者；狀態：planned。
- 交付：500 列與 2 MiB 軟目標頁面、不可變 cell handles、pin/LRU、明確 discard；分離 retained snapshot 與 live cursor ownership。
- Gate：合法單大列仍可讀；各 DB current result 切換或離線後仍保留全文；回收舊頁回 pageEvicted/cellEvicted，不重跑 SQL；新 actor 不操作舊 cursor。
- 相依：DB-C、DB-A；先 mock：DB-C。
- 責任範圍：`src-tauri/src/db_result_session.rs`、`src-tauri/src/db_service.rs`、`src/lib/types.ts`、`src/lib/ipc.ts`、`src/state/dbStore.ts`
- 協作注意：停止 cursor 不等於清除結果；pin-full 僅拒絕或延後新 admission。無強制 disk spool；先前固定每 500 列定位須改為保存 page ordinal。

### DB-B — 有界資料流與獨立控制通道

- 負責角色：DB傳輸實作者；狀態：planned。
- 交付：PG bounded row queue 與 byte permits，背壓傳到 driver；cancel/close/heartbeat 不被資料輸出堵住。
- Gate：大量結果停第一頁 30 秒時 retained queue 不持續膨脹；背壓滿載仍可取消並收到 heartbeat；取消不得影響下一 run。
- 相依：DB-C；先 mock：DB-C。
- 責任範圍：`src-tauri/src/db_query_worker.rs`、`src-tauri/src/db_service.rs`
- 協作注意：與 DB-A、DB-T、DB-H 平行。新 120 秒 liveness 語意須在 heartbeat/control path 通過後啟用，不能把合法長 SQL 無列輸出當失聯。

### DB-BT — Helper 批次傳輸量測與選型

- 負責角色：DB傳輸實作者；狀態：planned。
- 交付：比較 row/byte 雙界線的小批次，選擇具量測收益且保持簡單的 frame 策略。
- Gate：比較 32/128/500 列批次的吞吐、配置、首屏與取消延遲；保持 frame hard ceiling；未改善則保留簡單版本並交付證據。
- 相依：DB-B、DB-A；先 mock：DB-C。
- 責任範圍：`src-tauri/src/db_query_worker.rs`、`src-tauri/src/db_service.rs`
- 協作注意：原型可先做；正式比較等待 DB-A 避免把 accounting 成本誤歸因 serde，production 等 DB-B 的背壓與取消正確性。

### DB-T — 連線交易與實際執行 context

- 負責角色：DB執行狀態實作者；狀態：planned。
- 交付：connection-generation-scoped transaction/context snapshot，提供 fresh/stale/unknown 與实际 catalog，不靠本 run 重新初始化交易狀態。
- Gate：跨 run BEGIN/SELECT/UPDATE/COMMIT、error/cancel/reconnect 正確；unknown 保守處理；不得自動 commit/rollback；schema browse 不改 search_path。
- 相依：DB-C；先 mock：DB-C。
- 責任範圍：`src-tauri/src/db_connection_actor.rs`、`src-tauri/src/db_service.rs`、`src-tauri/src/db_query_worker.rs`、`src/state/dbStore.ts`、`src/app/panels/DatabasePanel.tsx`
- 協作注意：可獨立使用 engine fixture 開發；補充 context 讀取失敗不能覆蓋原 SQL outcome。Preview 啟用與交易保護須等待本包。

### DB-H — Host profile 與多 database 生命週期

- 負責角色：DB設定與連線實作者；狀態：planned。
- 交付：HostProfile 下建立每 DB 穩定 child descriptor，實作無秘密 migration、credential binding reuse、bounded database discovery 與 lazy open。
- Gate：舊 descriptorId 不變、migration 不讀 vault；DB A/B 獨立 actor/交易；刪 child 不刪 host credential；新 DB 不繼承其他 DB 的明文授權。
- 相依：DB-C；先 mock：DB-C。
- 責任範圍：`src-tauri/src/db_profiles.rs`、`src-tauri/src/db_credentials.rs`、`src-tauri/src/db_service.rs`、`src-tauri/src/db_query_worker.rs`、`src/state/dbStore.ts`
- 協作注意：同 host 的不同 DB 可平行開啟；同 descriptor 仍以 reservation 去重。不可每 schema 建連線，不用 USE 搬移既有 console，也不自動合併相似舊 profiles。

### DB-N — 有界多 schema metadata

- 負責角色：DB資料檢索實作者；狀態：planned。
- 交付：listNamespaces/listObjects/columns 有界分頁與搜尋，包含空 schema、權限、quoted identity、metadataRevision。
- Gate：PG/MSSQL 多 schema 同名物件、權限拒絕、空 schema，SQLite ATTACH/temp，過期 revision 與跨 DB 回應均不混入。
- 相依：DB-C、DB-H；先 mock：DB-C。
- 責任範圍：`src-tauri/src/db_service.rs`、`src-tauri/src/db_query_worker.rs`、`src/lib/databaseSql.ts`、`src/lib/types.ts`、`src/lib/ipc.ts`
- 協作注意：各引擎 adapter 可依契約 mock 與 DB-H 平行；production 跨 database owner 整合才等待 DB-H。保留 catalog/schema/kind/name 四元 identity。

### DB-Q — 排隊、去重與保留工作的資源 admission

- 負責角色：DB排程實作者；狀態：planned。
- 交付：每 connection metadata single-flight/queue，app query/metadata/connect/helper admission 與獨立控制優先權。
- Gate：快速展開 5 物件成功且去重；query 優先、取消可達；pin-full 不終止 active query、未提交交易或各 DB current result，只拒絕或延後新增工作。
- 相依：DB-C、DB-R、DB-B、DB-T；先 mock：DB-C。
- 責任範圍：`src-tauri/src/db_connection_actor.rs`、`src-tauri/src/db_query_worker.rs`、`src-tauri/src/db_service.rs`、`src/state/dbStore.ts`
- 協作注意：queue/counter 可先用 fake actor/clock/budget 平行開發；production 壓力處置須等 pin、背壓與交易狀態，不可只加 semaphore 後沿用隱藏 cancellation。

### DB-UR — 結果表格、cell viewer 與独立 Preview

- 負責角色：DB結果介面實作者；狀態：planned。
- 交付：虛擬化寬表、4 KiB cell preview 與全文 viewer、offline snapshot 呈現；Preview 使用獨立有界結果 bucket。
- Gate：500×20/200 欄 GUI 驗收與量測；離線全文可讀；Preview 不覆寫 SQL/runGroup，busy/open cursor/未提交交易時明確拒絕，不自動取消、提交或另開連線。
- 相依：DB-C、DB-R、DB-T；先 mock：DB-C。
- 責任範圍：`src/app/panels/DatabasePanel.tsx`、`src/lib/types.ts`、`src/state/dbStore.ts`、`src/lib/ipc.ts`
- 協作注意：可用 frozen page/cell/transaction fixtures 與 backend 平行；preview 截斷只是呈現，不得冒充 resultLimitReached。

### DB-UN — Host、database、schema 導覽與獨立 console

- 負責角色：DB導覽介面實作者；狀態：planned。
- 交付：Host→DB→schema/object 惰性樹、分頁搜尋、每 DB console bucket 與實際 context 顯示。 明確負責 schema-aware SQL completion provider：按 engine/descriptor/generation/metadataRevision 建候選，columns 按需讀取；使用 CodeMirror compartment 更新候選，不重建 EditorView/undo。
- Gate：DB A/B SQL、結果與取消目標不混用；切換 DB 不解除各自 current-result pin；schema filter 不 SET search_path；延遲/權限/空 schema 正確。 同名物件插入完整限定名稱；切換 DB/schema、revision 或 reconnect 時舊候選失效；無權限物件不出現；候選更新不清除草稿、游標或 undo。
- 相依：DB-C、DB-H、DB-N、DB-Q；先 mock：DB-C。
- 責任範圍：`src/app/workbench/DatabaseNavContent.tsx`、`src/app/panels/DatabasePanel.tsx`、`src/state/dbStore.ts`、`src/lib/databaseSql.ts`、`src/lib/databaseCompletion.ts`
- 協作注意：可先用多 DB fixtures 開發；database discovery 不代表自動開全部 connection。共用 dbStore 與 panel 接線交單一整合者。 databaseCompletion.ts 是規劃新增的 provider；DatabasePanel 的 CodeMirror 接線仍交既有 panel writer。DB-N 提供 metadata，不重複維護另一份無界全量 catalog。

### DB-E — 保持順序的 script 進度與提早結果

- 負責角色：DB執行進度實作者；狀態：planned。
- 交付：exact run/statement owner events、去重 reducer 與逐 statement 已完成結果，維持單 connection SQL 順序。
- Gate：快→慢→快 script 首結果提早可讀；事件重複/延遲、cancel/skipped/outcome 順序正確；資源壓力不丟目前結果、不重跑 statement。
- 相依：DB-C、DB-R、DB-B、DB-Q；先 mock：DB-C。
- 責任範圍：`src-tauri/src/db_service.rs`、`src-tauri/src/db_query_worker.rs`、`src/state/dbStore.ts`、`src/app/panels/DatabasePanel.tsx`、`src/lib/types.ts`、`src/lib/ipc.ts`
- 協作注意：前端 reducer/event fixture 可先平行；production emitter 等結果 ownership、背壓與 admission。不得 join_all 同一 script 的 statements。

### DB-SQ — SQLite idle cursor 與 worker 策略

- 負責角色：SQLite執行實作者；狀態：planned。
- 交付：量測多 idle cursor 的 thread、write wait、WAL 成本；依證據選擇有界 worker 策略並保持 live statement ownership。
- Gate：多 connection 501+ 列停留、next/release/cancel 與回收正確；不重跑 SQL、不丟 transaction/context，不以 idle TTL 終止工作。
- 相依：DB-C、DB-R、DB-Q；先 mock：DB-C。
- 責任範圍：`src-tauri/src/db_service.rs`、`src-tauri/src/db_connection_actor.rs`、`src-tauri/src/db_result_session.rs`
- 協作注意：基準量測可立即進行，worker 原型可用契約平行；production 接線等 DB-R/DB-Q。固定 worker 模型只在量測支持時採用。

### SH-C — SSH/SFTP session、credit、transfer 與覆寫契約

- 負責角色：SSH/SFTP backend owner；狀態：planned。
- 交付：固定 session generation、request generation、transfer ID 與狀態機；定義 byte credit、已確認進度、取消及 cleanup-incomplete、directory cursor、explicit target leaf、atomic-overwrite capability 與 conflict 錯誤。指定 scheduler 掌握 admission，adapter 不另開無界 queue 或連線。
- Gate：SH-A、SH-B、SH-F 共同確認 DTO、錯誤及 lifecycle 範例；active/pinned 達配額只阻擋新 admission，不自動停止工作。未知 host key 的人類確認期限與 auth timeout 分開。
- 相依：ALIGN-HOST；先 mock：—。
- 責任範圍：契約／驗收文件；不宣稱取得產品共享檔案寫入權。
- 協作注意：契約節點不授予產品檔案寫入權。不同檔案可有限平行傳輸；同 destination promotion、同 session lifecycle 與已接受 terminal input 順序仍需序列化。

### LS-C — LSP accepted-version、byte admission 與停止契約

- 負責角色：LSP backend owner；狀態：planned。
- 交付：定義取得 byte credit 並入列才是 accepted；accepted frames FIFO 且不可丟棄。每文件尚未 accepted 的變更只保留最新 document/version 引用，不累積 JSON 或 Promise；恢復後相對最後 accepted 版本補送合法 replacement。定義取消順序、writer failure 與 kill-before-stdin-lock。
- Gate：64 MiB outgoing JSON body、72 MiB/server queued+in-flight、128 MiB/app admission 起始值能容納 10 MiB 合法文件的 JSON expansion；壓力下持續編輯不產生無界未計費 payload，不因一般資源滿額 restart；超限文件保持編輯內容並明示 LSP 同步受限。
- 相依：—；先 mock：—。
- 責任範圍：契約／驗收文件；不宣稱取得產品共享檔案寫入權。
- 協作注意：前端 LSP 取消與補送由片段外的 frontend LSP owner 負責，這裡不列其檔案 ownership。其與 RT-LSP 的最終聯合 gate 由整合者接上；writer 不必等待整個 frontend 包完成才能使用契約 mock 開發。

### LG-C — Log segment、cursor 與 trace privacy 契約

- 負責角色：Logging owner；狀態：planned。
- 交付：由 RT-LOG owner 固定 segment identity/命名、writer-owned rotation、active bytes accounting、record truncation/degraded 格式、cursor 的 segment identity+offset，以及 retention 後 cursor 失效回應；定義 trace 預設關閉、敏感 body 的 opt-in/遮罩、落盤最小化及 export redaction 政策。
- Gate：以舊日日誌相容、跨 rotation cursor、retention 失效、超大 record 與假秘密 sentinel 範例確認契約；100 MiB 總預算包含 active segment，明確說明過载/磁碟錯誤時可觀測的 degraded 行為。
- 相依：—；先 mock：—。
- 責任範圍：契約／驗收文件；不宣稱取得產品共享檔案寫入權。
- 協作注意：契約不要求 RT-LSP 等 RT-LOG 整體實作完成；H02 producer 只依赖這個 privacy 契約。writer、rotation、query/export 都由同一 Logging owner 實作，避免同檔交錯修改。

### SH-A — SFTP extension 與有界 transport adapter

- 負責角色：SFTP adapter owner；狀態：planned。
- 交付：在 ALIGN-HOST 後，重用現有 sftp_edit/open_sftp_raw 的 raw session、posix-rename 與 reservation 邊界，抽出單一 sftp_transport adapter，提供 bounded request/read-ahead、directory cursor、cancel 與原子 promotion；不再預設建立新的 dependency patch。
- Gate：fake peer 驗證 capability 缺失、ACK 不回、short read/EOF、取消、late response 與 temp collision；in-flight bytes 有界，單個 transfer/channel ownership 明確，不自行建立第二套 scheduler。
- 相依：SH-C；先 mock：SH-C。
- 責任範圍：`src-tauri/src/sftp_transport.rs`、`src-tauri/src/sftp_edit.rs`
- 協作注意：sftp_transport.rs 是規劃新增路徑。若確需 dependency patch，patch 路徑由整合者確認後交此 owner；Cargo.toml/Cargo.lock 與 lib.rs 註冊仍由整合者寫。SH-B 可先使用契約 mock，但 production 整合依赖真 adapter。 本輪已出現可用 raw API 的實作位置；先和現有 owner 交接，避免再造一套 atomic-save。能力已出現不代表 S02／S12 驗收已完成。

### SH-B — SSH/SFTP backend lifecycle、scheduler 與資料完整性

- 負責角色：SSH/SFTP backend owner；狀態：planned。
- 交付：在單一 backend owner 下整合 byte backpressure、control deadline、SFTP singleflight、有限 transfer admission、read-ahead、listing budget、取消與 cleanup、confirmed progress、session generation 及同步 key I/O 隔離。目標存在且無 atomic overwrite 時保留原檔，回可另存的明確錯誤；新增安全 target leaf 並處理一次性 capability 的消耗/重試。
- Gate：同時測 stalled peer、滿 queue、1000 小檔、錯序 response、rename 失敗與取消；既有原檔不遺失，init 次數為 1，FD/task/slot 釋放，terminal 不因另一 transfer 取消而斷線；不以資源滿額取消 active transfer。
- 相依：SH-C、SH-A；先 mock：SH-C。
- 責任範圍：`src-tauri/src/ssh_service.rs`
- 協作注意：S01 的 shell 路徑與 SFTP 路徑仍共用 ssh_service.rs、session map 與 lifecycle，不能拆成兩名同時写檔的 worker。可以在契約後先 mock adapter 開發；若未先完成模組拆分，整檔必須維持此唯一 owner。

### SH-F — SSH/SFTP frontend 狀態一致性、歷史與 terminal queue

- 負責角色：SSH/SFTP frontend owner；狀態：planned。
- 交付：以固定 IPC/event 契約實作 credit 驅動輸入輸出、queued/running/cancelled UI、另存 target leaf、host/session/request generation fencing、coalesced progress、bounded completed history、inactive listing cache 及 refresh debounce；F10 由同 owner 改 head-index/deque，保留既有 terminal callback/backpressure/dispose 語意。
- Gate：mock 逆序 A/B、remove/reset 時晚到 connect、完成 fallback tick、部分 admission 與取消 cleanup-incomplete；真 backend 聯測另存及 transfer 狀態。固定總 bytes、不同 chunk 粒度的 F10 benchmark 及既有 queue tests 通過。
- 相依：SH-C、SH-B；先 mock：SH-C。
- 責任範圍：`src/state/sftpStore.ts`、`src/state/sshStore.ts`、`src/app/panels/SshPanel.tsx`、`src/terminal/SshTerminalSession.tsx`、`src/terminal/terminalOutputQueue.ts`
- 協作注意：可與 SH-A/SH-B 平行以 mock 開發。F10 是可獨立先交付的子變更，不需等 SSH backend 才量測；此 package 整體 production gate 仍需 SH-B。terminalOutputQueue.ts 不再交給其他 frontend worker 同時修改；清歷史不等於取消 active 工作。 ALIGN-HOST 僅限制 IPC／另存／remote editing 接線；F10 queue 內部且 API 相容的改變可獨立先行。

### RT-GIT — Git 全生命週期期限、輸出預算與搜尋分頁

- 負責角色：Git runtime owner；狀態：planned。
- 交付：在同一 owner 下重整 run_git_inner 的 stdin/stdout/stderr/wait/drain deadline 與 byte budgets，保留各 caller 既有 timeout、process-group cleanup 及 repo mutation 保護；Git log query 改有界 scan/cursor、batch OIDs 並只 hydrate 所需頁面。
- Gate：本機 fixture 驗證不讀 stdin、父程序退出而子程序持 pipe、output 超限與 cancellation 後 kill/reap；搜尋 pagination 保留固定 tips、OR filter、date-order，無漏重；不完整 status 不得當 clean。
- 相依：—；先 mock：—。
- 責任範圍：`src-tauri/src/git_service.rs`、`src-tauri/src/git_log.rs`
- 協作注意：R07/R08 都修改 run_git_inner，不能交兩個 owner 平行改同函式；R15 的 stdin batching 與 runner budget 需要同包驗收。可與 SSH/LSP/logging/preview 平行開發。process_kill.rs 如需新增共同 helper，由整合者寫入。

### RT-LSP — LSP bounded writer、停止與 trace producer

- 負責角色：LSP backend owner；狀態：planned。
- 交付：實作 byte-accounted writer、accepted frame FIFO、版本/取消契約、bounded stop report 與 kill-before-stdin-lock；依 LG-C 實作 trace producer 的敏感 body 最小化/遮罩。所有未獲 admission 的資料也納入端到端契約，不用無界 async promises 把壓力轉回 renderer。
- Gate：server 不讀 stdin、大型 didOpen/escaping、持續 typing/paste、cancel 與 exit fixture；accepted incremental 順序完整，資源壓力不 restart，真正 transport failure 可明確恢復；trace sentinel 不超出 LG-C 政策。前端 LSP 包的聯合 gate 由整合者另接。
- 相依：LS-C、LG-C；先 mock：LS-C。
- 責任範圍：`src-tauri/src/lsp_service.rs`
- 協作注意：writer 可在 LS-C 後先以 frontend mock 開發；H02 producer 的 production 行為必須等 LG-C 定案，但不依賴 RT-LOG 整包完成。F07 與其 frontend 檔案不屬本 owner；不可為取得 stdin lock 而阻止 owned child 被 kill。

### RT-LOG — Logging 單 writer、rotation、retention 與 cursor

- 負責角色：Logging owner；狀態：planned。
- 交付：實作有界可觀測 writer queue、segment rotation、active+closed 總量 accounting、record bytes 上限、legacy 格式相容、cursor/tail 查詢、retention invalidation 與 export 第二層 redaction；保留 audit/error loss 的明確 degraded 訊號。
- Gate：慢盤/滿 queue、不合法或超大 record、今日檔超配額、跨 rotation/retention cursor、fake-secret export 與 shutdown drain 通過；查詢不再假設單日檔最多 100 MiB，現有 legacy 非log檔不被刪除。
- 相依：LG-C；先 mock：LG-C。
- 責任範圍：`src-tauri/src/logging.rs`
- 協作注意：內部交付順序為 writer/segment ownership → rotation/retention → cursor 的真實整合。cursor 可先依 LG-C 用 fixture/mock 開發，但三部分同檔單 owner，不能把它們當無衝突的多人寫檔工作。

### RT-FSP — 有界檔案讀取與 Preview 併行/回收

- 負責角色：File/Preview runtime owner；狀態：planned。
- 交付：在同一 opened handle 實施 cap+1 讀取、縮短 preview sessions 鎖、bounded request workers/streaming、HEAD metadata 路徑、revoke/deadline，以及 expired/inactive session 清理與新 admission 上限。
- Gate：檔案 stat 後成長、慢 reader、並行 assets、revoke/stop 及 expired token fixtures；既有 discovery/serve cap、pinned capability、CSP/token/allowlist 保護不退化。pinned session 滿額時拒絕新工作，不強制回收仍在使用者前景中的資源。
- 相依：ALIGN-HOST；先 mock：—。
- 責任範圍：`src-tauri/src/fs_service.rs`、`src-tauri/src/preview_server.rs`
- 協作注意：可與其他 runtime 包平行。path_capability.rs 是共同邊界，不屬此包直接寫入範圍；所需 API 變更交整合者。Preview workers 可平行讀，但 revoke/generation authority 不能為吞吐放寬。 Live drift：底層讀寫／分類／capability 已搬入 host crate，先對齊後再修改，不能只修 desktop facade；不依賴該邊界的 fixture 可先準備。

### RT-PROC — Process 輸出有界 parser、tail 與事件 batching

- 負責角色：Process runtime owner；狀態：planned。
- 交付：將逐行無界累積改為 bounded chunk parser，tail 同時限制 bytes/行數，保留 continuation/truncated 訊號並batch IPC；維持 ready URL/port detection 的 rolling window 與 process ownership。
- Gate：100 MiB 無換行、十萬短行、跨chunk ready URL、stderr tail 與停止情境；parser/RSS/IPC有界、偵測正確、FD回收。診斷tail可明確截斷，不能誤把terminal完整輸出套用相同丟棄政策。
- 相依：—；先 mock：—。
- 責任範圍：`src-tauri/src/process_service.rs`
- 協作注意：可獨立開發。process_kill.rs 的共同期限/kill helper 由整合者修改，避免與 Git/LSP 形成三套不一致的終止策略；不得停止不屬此 manager 的程序。

### RT-PERF — Perf 單次取樣與 terminal high-water 回收

- 負責角色：Performance telemetry owner；狀態：planned。
- 交付：實作 sampling singleflight/generation、背景/idle排程與程序資訊快取；歷史 terminal high-water fold 入 cumulative scalar 後刪除inactive entry，保持重連/同ID不重複計數。
- Gate：取樣耗時超過interval仍最多一個 in-flight；focus切換/晚到結果正確；10k unique terminal建立關閉後map回落、cumulative值單調且無漏重，取樣自身CPU/延遲可量測。
- 相依：—；先 mock：—。
- 責任範圍：`src-tauri/src/perf_service.rs`、`src/workbench/PerfBridge.tsx`
- 協作注意：F12與R14共用 PerfBridge，應由同 owner 交付；不需要等待 SH-F 的queue內部最佳化。若修改共同metrics registry契約需協作，但不是 runtime 真相依；資源壓力不得令監控直接停止 active terminal。

### FE-Dirty — 輸入 dirty 狀態 no-op 與 structural sharing

- 負責角色：FE-EDITOR；狀態：planned。
- 交付：已 dirty 的文件不重建 state；只替換實際改變的 tab/group，維持同文件多個 view 的 dirty/save 行為。
- Gate：輸入、保存、重開與跨 group 同檔案狀態一致；量測 editor input p95、store notification 與非目標 pane render 次數。
- 相依：—；先 mock：—。
- 責任範圍：`src/state/workspaceStore.ts`、`src/editor/cmExtensions.ts`、`src/editor/EditorPane.tsx`、`src/workbench/EditorArea.tsx`
- 協作注意：可立即實作。與 FE-Load 共用 cmExtensions/editor hosts，由同一 owner 短暫串行接線；這是檔案 ownership，不是 FE-Load 的技術前置。

### FE-TREE-C — 檔案樹 cache 與虛擬列契約

- 負責角色：FE-TREE；狀態：planned。
- 交付：固定 workspace/path/generation 身分、loading/loaded/evicted/error、reload、stable row key、active/expanded pin 與 ScrollArea viewport/ref 契約，提供 stale completion 與 eviction fixtures。
- Gate：cache 與 view owner 對同一 fixtures 得到一致 row identity、loading/reload 行為；契約不要求先完成 cache 或 virtualizer。
- 相依：—；先 mock：—。
- 責任範圍：`src/state/fileTreeStore.ts`、`src/workbench/FileTree.tsx`
- 協作注意：小型契約工作，不先全面重構。契約凍結後 FE-TreeCache 與 FE-TreeView 可以平行開發。

### FE-TreeCache — 檔案樹有界 cache、跨批次 single-flight 與取消失效

- 負責角色：FE-TREE；狀態：planned。
- 交付：共用 semaphore、跨 batch single-flight、generation/latest apply、workspace/node/byte budget 與 LRU；active/expanded 節點受 pin 保護。
- Gate：先驗證 drop/evict/workspace change 會失效 pending listings，再啟用 eviction；大量並行 relist 不超過全域上限，晚回覆不復活 cache，evicted 節點可重新載入。
- 相依：FE-TREE-C；先 mock：FE-TREE-C。
- 責任範圍：`src/state/fileTreeStore.ts`
- 協作注意：F03/F04 同檔案且共享生命週期，合併 ownership。若 pinned 節點超出 budget，限制新 cache admission，不能破壞 active/expanded 狀態。與 RT-FSP 的實際 filesystem 壓力行為於整合 gate 驗證，不要求其先完成才能開發。

### FE-TreeView — 檔案樹可見列 flatten 與虛擬化

- 負責角色：FE-TREE-UI；狀態：planned。
- 交付：只建立可見的扁平 tree rows 與 viewport 附近 DOM，保留展開、選取、鍵盤、ARIA、scroll anchor 和載入狀態。
- Gate：大樹 DOM/訂閱量隨 viewport 受界限；真實 GUI 驗證展開、折疊、重載及捲動錨點；與 FE-TreeCache 整合後確認 eviction/reload 不改變 row identity。
- 相依：FE-TREE-C；先 mock：FE-TREE-C。
- 責任範圍：`src/workbench/FileTree.tsx`、`src/components/ui/scroll-area.tsx`
- 協作注意：不需要等待 FE-TreeCache 全包完成；可用 frozen fixtures 獨立交付相容 view。沿用 shadcn ScrollArea viewport；不建立包辦 DB/SFTP/Search 的通用巨大 list。

### FE-Document — 文件讀取 single-flight 與 registry 生命週期

- 負責角色：FE-DOCUMENT；狀態：planned。
- 交付：合併相同文件的 pending read；drop/clear 使用 generation invalidation，避免已移除文件被晚完成的讀取重新放回 cache。
- Gate：duplicate open 只讀一次；讀取失敗可重試；drop/clear 之後晚完成不復活資料；保存、未保存文件與目前使用中的 document 不遺失。
- 相依：—；先 mock：—。
- 責任範圍：`src/editor/documentRegistry.ts`
- 協作注意：保持 registry 公開 API，可立即開發。FE-Markdown 可依自己的 worker/document input 契約平行開發，不將兩者排成硬依賴。

### SEARCH-C — Workspace search 取消、預算與增量事件契約

- 負責角色：FE-PROTOCOL；狀態：planned。
- 交付：固定 run identity、stable result identity、batch/done/cancel/truncated/error、預算原因、排序及 settlement 語意；定義共享 quota/cancel interface，提供 TS/Rust wire fixtures。
- Gate：雙側 fixtures 一致；stale run 不能回填新搜尋；done/cancel/error 只 settle 一次；global quota 不是每 worker 複製一份。
- 相依：—；先 mock：—。
- 責任範圍：`src-tauri/src/search_service.rs`、`src/lib/types.ts`、`src/lib/ipc.ts`、`src/workbench/search/useWorkspaceSearch.ts`
- 協作注意：契約先交付後，Core/Parallel/View 各自使用 fake collector、clock、walker 與 channel 平行開發。R15 Git log 屬 RT-GIT，不與 workspace search 合包。

### SEARCH-Core — Workspace search 內層取消與資源預算

- 負責角色：SEARCH-BACKEND；狀態：planned。
- 交付：受控 executor、檔案/區塊內取消、scan/line/bytes/time budgets、parallel-safe global quota；IPC send 失敗立即停止；避免不必要的全文 lowercase 重掃。
- Gate：超大單檔、超長行、取消、IPC 關閉、Unicode 與 stale generation 案例通過；資源耗用有界且截斷原因可辨識；run settlement 前不釋放仍在工作中的 owner。
- 相依：SEARCH-C；先 mock：SEARCH-C。
- 責任範圍：`src-tauri/src/search_service.rs`
- 協作注意：先建立可供多 worker 共用的安全 collector/quota/cancellation。這是 SEARCH-Parallel 正式啟用的硬前置，worker 原型與前端不必等待 Core 完成才開發。

### SEARCH-Parallel — Workspace search 有界平行 walker 與 worker

- 負責角色：SEARCH-BACKEND；狀態：planned。
- 交付：bounded work queue、worker-local Searcher 與共享 result quota；並行度可由量測選定，取消能停止生產與消費。
- Gate：多 worker 不超配結果/bytes quota、不遺漏 settlement；取消能排空或丟棄待處理工作；cold/warm、大小檔混合語料比較吞吐、首批延遲、CPU 與 RSS。
- 相依：SEARCH-C、SEARCH-Core；先 mock：SEARCH-C。
- 責任範圍：`src-tauri/src/search_service.rs`
- 協作注意：可在 SEARCH-C 後 mock 開發；只有 production 啟用等待 SEARCH-Core。不能藉無上限平行度或每 worker 獨立上限製造表面速度提升。

### SEARCH-View — Workspace search 增量 grouping 與虛擬化結果

- 負責角色：FE-SEARCH-UI；狀態：planned。
- 交付：逐 batch 增量更新 groups/results，避免每次複製重組全部事件；只渲染可見結果，顯示取消/截斷原因且保持鍵盤選取。
- Gate：密集 batch、快速改 query、取消後新搜尋、截斷與 IPC error fixtures 通過；最終連接 Core 驗證真實狀態；大結果集 DOM 與 commit duration 受界限。
- 相依：SEARCH-C、SEARCH-Core；先 mock：SEARCH-C。
- 責任範圍：`src/workbench/search/useWorkspaceSearch.ts`、`src/workbench/search/WorkspaceSearchGroup.tsx`、`src/components/ui/scroll-area.tsx`
- 協作注意：元件與 reducer 可在契約後使用 mock 平行開發；既有事件的純 UI 優化可先交付。完整新截斷/取消行為以 Core 真實 emitter 為 production 前置，不依賴 Parallel 完工。

### FE-LSP — Semantic tokens 與 diagnostics 請求取消

- 負責角色：FE-LSP；狀態：planned。
- 交付：superseded request 與 dispose 取消，清理 timer/listener；即使 server 忽略取消仍以 generation 防止 stale apply。 同時承接 transport IPC admission、workspace didOpen/didChange producer、client recovery 與 extension adapter；latest-unsent document/version 在序列化前合併，不讓 sendChain 累積完整字串。
- Gate：快速輸入、切文件、關 pane、server 延遲/忽略取消時結果正確；與 RT-LSP 整合驗證 pending request、server CPU 與 shutdown，不能只測前端丟棄回覆。 didOpen→didChange→request/save barrier→didClose 保序，背景文件升為 editor 時版本一致；真 transport 失敗後 reinitialize／重新同步最新 buffer，替換 pane client/formatter/plugin 引用且不重複 append。純記憶體壓力不 restart；舊 generation IPC 與回覆失效。
- 相依：LS-C；先 mock：LS-C。
- 責任範圍：`src/lsp/semanticTokens.ts`、`src/lsp/diagnosticsPull.ts`、`src/lsp/transport.ts`、`src/lsp/workspace.ts`、`src/lsp/lspManager.ts`、`src/lsp/lspExtensions.ts`、`src/lsp/transport.test.ts`、`src/lsp/workspace.test.ts`、`src/lsp/lspManager.test.ts`、`src/lsp/lspExtensions.test.ts`、`src/editor/editorPane.lsp.test.tsx`、`src/editor/EditorPane.tsx`、`src/lib/ipc.ts`
- 協作注意：LS-C 凍結取消與 disposal owner 後即可 mock 開發，與 Rust writer/shutdown 實作平行；端到端 gate 等兩側整合。 transport.ts/workspace.ts/lspManager.ts/lspExtensions.ts 歸 FE-LSP；EditorPane 與其測試由 editor lifecycle writer 短暫接線，ipc.ts 仍歸 INT。只限制 Rust queue 不構成端到端改善。

### FE-Herdr — HERDR snapshot structural sharing 與 ID maps

- 負責角色：FE-HERDR；狀態：planned。
- 交付：保留未變 snapshot 實體參照，建立內部 ID lookup，移除 attention render 迴圈中的 agents.find；保留現有 public DTO 與 session isolation。
- Gate：相同/局部改變/刪除 snapshot fixtures 對應正確 references；Attention/Agents 選取不漂移；大量 agents 的更新 CPU 與無關 render 次數下降。 對齊現有 normalization／identity writer：合法 workspace path→null 不能保留舊值，cwd 不補作 root；entity maps 限正確 runtime scope，舊 connection generation 不回填。同 host access 升級 identity 穩定、跨 host 同 path 必須隔離。
- 相依：—；先 mock：—。
- 責任範圍：`src/state/herdrStore.ts`、`src/app/workbench/HerdrNavContent.tsx`
- 協作注意：可立即以既有 snapshot fixtures 開發。新內部 maps 透過 selector adapter 接 UI，不等待全部 HERDR backend 改善。 現有 normalization／runtimeIdentity 變更不代表 store 已 host-aware；不接管他人四個 source/test 檔。不需等完整 host migration，最終接線再對齊契約。

### FE-Load — 功能與語言 lazy chunks

- 負責角色：FE-EDITOR；狀態：planned。
- 交付：按實際使用載入功能與語言模組，補足 loading/error/dispose 行為；記錄 production dynamic import/asset 路徑供 CSP 整合。
- Gate：冷啟動與各功能首次開啟分開量測；lazy loading 前後功能、語言切換與錯誤恢复一致；最終 Tauri chunks/workers/CSP 聯合 gate 通過。
- 相依：—；先 mock：—。
- 責任範圍：`src/editor/cmExtensions.ts`、`src/editor/EditorPane.tsx`、`src/workbench/EditorArea.tsx`、`src/App.tsx`、`vite.config.ts`、`package.json`、`bun.lock`
- 協作注意：可立即開發；與 FE-Dirty 同 owner，cmExtensions/editor hosts 採短暫串行 handoff，不建立偽硬依賴。F08/F09/H01 由總計畫另設聯合 gate，不互相 requires。bundle/gzip bytes 不能替代啟動延遲證據。

### FE-Markdown — Markdown worker、latest-wins 與預覽預算

- 負責角色：FE-MARKDOWN；狀態：planned。
- 交付：定義 worker input/output/generation 契約，移出主執行緒 parsing，加入 bytes/nodes/time budgets、latest-wins 及 worker disposal；保留 sanitizer。
- Gate：快速切檔/輸入/關閉後 stale result 不回填；大文檔與病態 markdown 主執行緒可回應；預算拒絕有可理解狀態；真 Tauri module worker、資源 URL 與 CSP 聯合 gate 通過。
- 相依：—；先 mock：—。
- 責任範圍：`src/workbench/MarkdownPreview.tsx`、`vite.config.ts`、`package.json`、`bun.lock`
- 協作注意：可用 fake worker 與固定 document fixtures 立即開發，不要求 FE-Document 完工。worker 本身不提供 HTML 安全邊界；不得移除 sanitizer。與 FE-Load/FE-CSP 只共享早期資源契約及最終聯合 gate。

### FE-Build — Tailwind 建置 profiling 與明確 source 範圍

- 負責角色：FE-BUILD；狀態：planned。
- 交付：量測 Tailwind transform/source discovery 成本，以實際使用目錄限制 source 範圍，保留合法動態 classes。
- Gate：固定依賴與冷/暖條件重複量測；最終新增 UI/worker 目錄和 dynamic class coverage 不漏；完整 build 與 GUI 樣式回歸通過。
- 相依：—；先 mock：—。
- 責任範圍：`src/styles.css`、`vite.config.ts`、`package.json`、`bun.lock`
- 協作注意：profiling 可立即開始；整合後再確認所有實際來源。既有 27.8 秒 Tailwind transform、28.51 秒 Vite 為單次觀察，不當作穩定基準或 CPU time。效能量測期間不與其他 build/壓测同跑。

### FE-CSP — Main WebView CSP 與 capability 邊界加固

- 負責角色：FE-SECURITY；狀態：planned。
- 交付：盤點 script/style/img/font/connect/worker 實際需求，建立主 WebView CSP；保持 preview 與 main 的 capability 分離及必要 dev/production 資源載入。
- Gate：真 Tauri 驗證 main/child webview IPC 權限、外部 navigation、CodeMirror、worker、fonts/assets 與 CSP reports；最終 F08/F09 production chunks/workers 聯合 gate 通過後正式啟用。
- 相依：—；先 mock：—。
- 責任範圍：`src-tauri/tauri.conf.json`、`src-tauri/capabilities/default.json`
- 協作注意：inventory/policy/fixtures 可立即進行，不等待全部改善完成。與 FE-Load/FE-Markdown 不互相 requires，總計畫負責聯合 gate。此項是防禦縱深，不能改寫為已證明的 main XSS/RCE。

### G-DB — 多 database／schema 與結果壓力聯合驗收

- 負責角色：整合／驗收負責人；狀態：planned。
- 交付：使用真正實作取代 mock，完成跨層回歸與故障注入。
- Gate：兩 DB 分別保留未提交交易與 current result；另一 DB 大結果引發 cache 壓力時，metadata、取消、disconnect/reconnect 不破壞原工作。驗證三引擎、migration、同名／空 schema、temp objects、離線 cell 與 exact owner。
- 相依：INT、DB-S、DB-C、DB-A、DB-R、DB-B、DB-BT、DB-T、DB-H、DB-N、DB-Q、DB-UR、DB-UN、DB-E、DB-SQ；先 mock：—。
- 責任範圍：契約／驗收文件；不宣稱取得產品共享檔案寫入權。
- 協作注意：多個 requires 是 AND gate；前置各包可依自己的契約平行開發。

### G-SSH — SSH 互動與 SFTP 真實傳輸聯合驗收

- 負責角色：整合／驗收負責人；狀態：planned。
- 交付：使用真正實作取代 mock，完成跨層回歸與故障注入。
- Gate：真 adapter＋backend＋frontend 在高 RTT、多檔、慢 renderer、取消／黑洞、無 atomic overwrite、目的地競態及大目錄下驗證 bytes、echo 延遲與原檔完整性。
- 相依：INT、SH-C、SH-A、SH-B、SH-F；先 mock：—。
- 責任範圍：契約／驗收文件；不宣稱取得產品共享檔案寫入權。
- 協作注意：多個 requires 是 AND gate；前置各包可依自己的契約平行開發。

### G-LSP — LSP 一致性、取消、隱私與退出聯合驗收

- 負責角色：整合／驗收負責人；狀態：planned。
- 交付：使用真正實作取代 mock，完成跨層回歸與故障注入。
- Gate：10 MiB escaping 密集文件、不讀 stdin、持續編輯與過期 request；accepted 版本有序、pending 有界、停機不等 stdin 鎖，trace producer 與落盤／匯出政策一致。
- 相依：INT、LS-C、LG-C、RT-LSP、RT-LOG、FE-LSP；先 mock：—。
- 責任範圍：契約／驗收文件；不宣稱取得產品共享檔案寫入權。
- 協作注意：多個 requires 是 AND gate；前置各包可依自己的契約平行開發。

### G-ASSET — production chunks、worker、CSP 與 CSS 聯合驗收

- 負責角色：整合／驗收負責人；狀態：planned。
- 交付：使用真正實作取代 mock，完成跨層回歸與故障注入。
- Gate：真 Tauri 驗证 lazy feature/language chunks、Markdown worker、CodeMirror、字型、IPC、資產及動態 class；不需等待其他 DB／SFTP 效能工作才開始此 gate。
- 相依：INT、FE-Load、FE-Markdown、FE-Build、FE-CSP；先 mock：—。
- 責任範圍：契約／驗收文件；不宣稱取得產品共享檔案寫入權。
- 協作注意：多個 requires 是 AND gate；前置各包可依自己的契約平行開發。

### G-ALL — 完整回歸、量測與交付

- 負責角色：整合／驗收負責人；狀態：planned。
- 交付：55 項與四個新能力完整處置、before/after 證據、跨平台回歸與更新報告。
- Gate：完整 frontend／Rust／三引擎 integration 與真 Tauri 驗收；50 次 lifecycle 循環、2 小時混合負載；量測候選需有明確採用或保留原實作的數據理由，不宣稱未實測改善。
- 相依：BASE、ALIGN-HOST、INT、DB-S、DB-C、DB-A、DB-R、DB-B、DB-BT、DB-T、DB-H、DB-N、DB-Q、DB-UR、DB-UN、DB-E、DB-SQ、SH-C、LS-C、LG-C、SH-A、SH-B、SH-F、RT-GIT、RT-LSP、RT-LOG、RT-FSP、RT-PROC、RT-PERF、FE-Dirty、FE-TREE-C、FE-TreeCache、FE-TreeView、FE-Document、SEARCH-C、SEARCH-Core、SEARCH-Parallel、SEARCH-View、FE-LSP、FE-Herdr、FE-Load、FE-Markdown、FE-Build、FE-CSP、G-DB、G-SSH、G-LSP、G-ASSET；先 mock：—。
- 責任範圍：契約／驗收文件；不宣稱取得產品共享檔案寫入權。
- 協作注意：重型量測獨占主機。此 gate 不包含 Git 寫入、PR、發布或使用真實遠端帳密。

## 共享檔案交接後的目標 writer

下表是計畫的最終 writer 指派，優先於子系統片段內的角色名稱；交接前仍維持現有工作者 ownership。task.files 是參與範圍，不是允許每個參與者直接修改。INT 集中持有全域檔案；domain owner 提交介面需求／adapter，由指定 writer 接線。新檔明確標示為預計新增。

| 路徑 | 有效 writer | 相關包 | 理由 |
|---|---|---|---|
| `src-tauri/src/lib.rs` | INT | INT、DB-C、DB-R、DB-N、DB-UR、DB-E、SH-A、SH-B、RT-GIT、RT-LSP、RT-LOG、RT-FSP、RT-PROC、RT-PERF | 其他包提交 signature／DTO／dependency 需求，由 INT 串行接線；exact clippy baseline 在 final diagnostics 確定後更新，不讓多 worker 各改一份。 |
| `src/lib/ipc.ts` | INT | INT、DB-C、DB-R、DB-N、DB-UR、DB-E、SEARCH-C、FE-LSP、SH-B、SH-F、RT-GIT、RT-LSP、RT-LOG、RT-FSP、RT-PROC、RT-PERF、SEARCH-Core、SEARCH-View | 其他包提交 signature／DTO／dependency 需求，由 INT 串行接線；exact clippy baseline 在 final diagnostics 確定後更新，不讓多 worker 各改一份。 |
| `src/lib/types.ts` | INT | INT、DB-C、DB-R、DB-N、DB-UR、DB-E、SEARCH-C、SH-B、SH-F、RT-GIT、RT-LSP、RT-LOG、RT-FSP、RT-PROC、RT-PERF、SEARCH-Core、SEARCH-View | 其他包提交 signature／DTO／dependency 需求，由 INT 串行接線；exact clippy baseline 在 final diagnostics 確定後更新，不讓多 worker 各改一份。 |
| `src-tauri/Cargo.toml` | INT | INT、SH-A、RT-GIT、RT-LSP、RT-LOG、RT-FSP、RT-PROC、RT-PERF | 其他包提交 signature／DTO／dependency 需求，由 INT 串行接線；exact clippy baseline 在 final diagnostics 確定後更新，不讓多 worker 各改一份。 |
| `src-tauri/Cargo.lock` | INT | INT、SH-A、RT-GIT、RT-LSP、RT-LOG、RT-FSP、RT-PROC、RT-PERF | 其他包提交 signature／DTO／dependency 需求，由 INT 串行接線；exact clippy baseline 在 final diagnostics 確定後更新，不讓多 worker 各改一份。 |
| `src-tauri/src/path_capability.rs` | INT | INT、SH-B、RT-FSP | 其他包提交 signature／DTO／dependency 需求，由 INT 串行接線；exact clippy baseline 在 final diagnostics 確定後更新，不讓多 worker 各改一份。 |
| `src-tauri/src/process_kill.rs` | INT | INT、RT-GIT、RT-LSP、RT-PROC | 其他包提交 signature／DTO／dependency 需求，由 INT 串行接線；exact clippy baseline 在 final diagnostics 確定後更新，不讓多 worker 各改一份。 |
| `.github/clippy-baseline.json` | INT | INT、DB-S、DB-A、DB-R、DB-B、DB-BT、DB-T、DB-H、DB-N、DB-Q、DB-E、DB-SQ | 其他包提交 signature／DTO／dependency 需求，由 INT 串行接線；exact clippy baseline 在 final diagnostics 確定後更新，不讓多 worker 各改一份。 |
| `src/components/ui/scroll-area.tsx` | INT 指定的共用 UI owner | FE-TreeView、SEARCH-View | Tree、Search、DB、SFTP 共享 viewport/ref 接入方式；領域列表可平行寫，不同時修改 primitive，也不額外發明通用 list 框架。 |
| `package.json` | Build／asset 整合 owner | FE-Load、FE-Markdown、FE-Build | 集中 dependency、worker/chunk 及 Tailwind source 接線。先固定 asset contract，最後驗證 lazy components 與動態 classes 都有 CSS。 |
| `bun.lock` | Build／asset 整合 owner | FE-Load、FE-Markdown、FE-Build | 集中 dependency、worker/chunk 及 Tailwind source 接線。先固定 asset contract，最後驗證 lazy components 與動態 classes 都有 CSS。 |
| `vite.config.ts` | Build／asset 整合 owner | FE-Load、FE-Markdown、FE-Build | 集中 dependency、worker/chunk 及 Tailwind source 接線。先固定 asset contract，最後驗證 lazy components 與動態 classes 都有 CSS。 |
| `src/styles.css` | Build／asset 整合 owner | FE-Build | 集中 dependency、worker/chunk 及 Tailwind source 接線。先固定 asset contract，最後驗證 lazy components 與動態 classes 都有 CSS。 |
| `src-tauri/host/src/path_capability.rs` | Host/core 邊界 owner（沿用現有工作者） | SH-A、SH-B、RT-FSP、INT | 真 capability／分類／讀寫實作已移到 host；desktop facade 不再是常數與底層檢查所在地。先 ALIGN-HOST，再由單一 owner 接線安全邊界。 |
| `src-tauri/host/src/file_content.rs` | Host/core 邊界 owner（沿用現有工作者） | SH-A、SH-B、RT-FSP、INT | 真 capability／分類／讀寫實作已移到 host；desktop facade 不再是常數與底層檢查所在地。先 ALIGN-HOST，再由單一 owner 接線安全邊界。 |
| `src-tauri/host/src/content.rs` | Host/core 邊界 owner（沿用現有工作者） | SH-A、SH-B、RT-FSP、INT | 真 capability／分類／讀寫實作已移到 host；desktop facade 不再是常數與底層檢查所在地。先 ALIGN-HOST，再由單一 owner 接線安全邊界。 |
| `src-tauri/host/src/files.rs` | Host/core 邊界 owner（沿用現有工作者） | SH-A、SH-B、RT-FSP、INT | 真 capability／分類／讀寫實作已移到 host；desktop facade 不再是常數與底層檢查所在地。先 ALIGN-HOST，再由單一 owner 接線安全邊界。 |
| `src-tauri/src/file_content.rs` | Host/core 邊界 owner（沿用現有工作者） | SH-A、SH-B、RT-FSP、INT | 真 capability／分類／讀寫實作已移到 host；desktop facade 不再是常數與底層檢查所在地。先 ALIGN-HOST，再由單一 owner 接線安全邊界。 |
| `src-tauri/host/src/protocol.rs` | Host protocol/runtime owner（沿用現有工作者） | SH-C、SH-A、SH-B、RT-FSP、INT | Host transport 是 SSH 的新消費者，protocol limits 與 LSP limits 分開。保留每 connection request 順序；HostState disconnect_all 納入退出整合。 |
| `src-tauri/host/src/wire.rs` | Host protocol/runtime owner（沿用現有工作者） | SH-C、SH-A、SH-B、RT-FSP、INT | Host transport 是 SSH 的新消費者，protocol limits 與 LSP limits 分開。保留每 connection request 順序；HostState disconnect_all 納入退出整合。 |
| `src-tauri/host/src/server.rs` | Host protocol/runtime owner（沿用現有工作者） | SH-C、SH-A、SH-B、RT-FSP、INT | Host transport 是 SSH 的新消費者，protocol limits 與 LSP limits 分開。保留每 connection request 順序；HostState disconnect_all 納入退出整合。 |
| `src-tauri/host/src/main.rs` | Host protocol/runtime owner（沿用現有工作者） | SH-C、SH-A、SH-B、RT-FSP、INT | Host transport 是 SSH 的新消費者，protocol limits 與 LSP limits 分開。保留每 connection request 順序；HostState disconnect_all 納入退出整合。 |
| `src-tauri/host/src/lib.rs` | Host protocol/runtime owner（沿用現有工作者） | SH-C、SH-A、SH-B、RT-FSP、INT | Host transport 是 SSH 的新消費者，protocol limits 與 LSP limits 分開。保留每 connection request 順序；HostState disconnect_all 納入退出整合。 |
| `src-tauri/src/host_service.rs` | Host protocol/runtime owner（沿用現有工作者） | SH-C、SH-A、SH-B、RT-FSP、INT | Host transport 是 SSH 的新消費者，protocol limits 與 LSP limits 分開。保留每 connection request 順序；HostState disconnect_all 納入退出整合。 |
| `src-tauri/src/sftp_edit.rs` | SH-A adapter owner（完成現有 writer 交接後） | SH-A、SH-B、SH-F | 已有 raw session、posix-rename／remote-write reservation 使用者；先交接，再抽出共用 adapter，禁止第二套 promotion／save ownership。 |
| `src-tauri/host/Cargo.toml` | INT dependency/build owner | SH-A、RT-FSP、INT | 新 host crate 的 dependency／lockfile 也集中排程；host/target 是產物，不列 source ownership。 |
| `src-tauri/host/Cargo.lock` | INT dependency/build owner | SH-A、RT-FSP、INT | 新 host crate 的 dependency／lockfile 也集中排程；host/target 是產物，不列 source ownership。 |
| `src/lib/herdrNormalize.ts` | 現有 normalization／identity 工作負責人 | FE-Herdr、INT | FE-Herdr 唯讀消費最終契約，不接管這四檔。合法 workspace path→null、host-scoped tuple keys、access 升級及 stale connection generation 均需整合回歸。 |
| `src/lib/herdrNormalize.test.ts` | 現有 normalization／identity 工作負責人 | FE-Herdr、INT | FE-Herdr 唯讀消費最終契約，不接管這四檔。合法 workspace path→null、host-scoped tuple keys、access 升級及 stale connection generation 均需整合回歸。 |
| `src/lib/runtimeIdentity.ts` | 現有 normalization／identity 工作負責人 | FE-Herdr、INT | FE-Herdr 唯讀消費最終契約，不接管這四檔。合法 workspace path→null、host-scoped tuple keys、access 升級及 stale connection generation 均需整合回歸。 |
| `src/lib/runtimeIdentity.test.ts` | 現有 normalization／identity 工作負責人 | FE-Herdr、INT | FE-Herdr 唯讀消費最終契約，不接管這四檔。合法 workspace path→null、host-scoped tuple keys、access 升級及 stale connection generation 均需整合回歸。 |
| `src-tauri/tests/database_integration.rs` | DB integration 整合者 | DB-S、DB-A、DB-R、DB-B、DB-BT、DB-T、DB-H、DB-N、DB-Q、DB-UR、DB-UN、DB-E、DB-SQ | TLS fixture 優先交付；交易、多 DB、namespace、取消、資源壓力共用 integration fixtures，由單一 writer 接線。DB-S 不永久獨占，也不因此成為每個 DB 包的硬前置。 |
| `tests/database` | DB integration 整合者 | DB-S、DB-A、DB-R、DB-B、DB-BT、DB-T、DB-H、DB-N、DB-Q、DB-UR、DB-UN、DB-E、DB-SQ | TLS fixture 優先交付；交易、多 DB、namespace、取消、資源壓力共用 integration fixtures，由單一 writer 接線。DB-S 不永久獨占，也不因此成為每個 DB 包的硬前置。 |
| `src-tauri/src/db_service.rs` | DB後端整合者 | DB-S、DB-C、DB-R、DB-B、DB-BT、DB-T、DB-H、DB-N、DB-Q、DB-E、DB-SQ | 共用 commands、engine worker 與 run 接線由單一 writer 合併。DB-S 小修先落地；各線的獨立模組、fixtures 與演算法仍可平行，不以同檔名製造功能硬相依。 |
| `src-tauri/src/db_result_session.rs` | DB結果核心實作者 | DB-A、DB-R、DB-SQ | DB-A 先固定增量計帳，再由 DB-R 改 page/pin/snapshot；DB-SQ 透過介面使用。預算預留、pin 與 eviction 判斷不可競態。 |
| `src-tauri/src/db_query_worker.rs` | DB傳輸與協議整合者 | DB-S、DB-C、DB-B、DB-BT、DB-T、DB-H、DB-N、DB-Q、DB-E | message enum、frame/read loop、heartbeat 與 cancellation 單一 writer；engine adapters 可平行。控制通道不可被資料背壓阻塞。 |
| `src-tauri/src/db_connection_actor.rs` | DB actor 整合者 | DB-T、DB-Q、DB-SQ | lease、交易與 settlement 狀態須統一；同 connection query/metadata/context 保持序列化，exact cancel 完成前不能讓新 run 接管。 |
| `src-tauri/src/db_profiles.rs` | DB設定與連線實作者 | DB-H | migration、credential binding、recovery ledger 與同 descriptor open reservation 單一 owner；不同 DB child 可平行連線。 |
| `src-tauri/src/db_credentials.rs` | DB設定與連線實作者 | DB-H | migration、credential binding、recovery ledger 與同 descriptor open reservation 單一 owner；不同 DB child 可平行連線。 |
| `src/state/dbStore.ts` | DB store 整合者 | DB-R、DB-T、DB-H、DB-Q、DB-UR、DB-UN、DB-E | 單一 writer 接線各包 reducers/adapters；每 DB current result、離線 owner、metadataRevision 與新連線身份不可互相覆蓋。 |
| `src/app/panels/DatabasePanel.tsx` | DB結果介面整合者 | DB-T、DB-UR、DB-UN、DB-E | 表格/viewer 抽取與 panel 接線集中合併；交易、進度與導覽透過凍結 props/store 契約平行開發。 |
| `src/app/workbench/DatabaseNavContent.tsx` | DB導覽介面實作者 | DB-UN、DB-N | 導覽與 qualified object helper 單一 writer；backend metadata adapter 不直接修改 tree，逐段 identifier quoting 不可退化。 |
| `src/lib/databaseSql.ts` | DB導覽介面實作者 | DB-N、DB-UN | 導覽與 qualified object helper 單一 writer；backend metadata adapter 不直接修改 tree，逐段 identifier quoting 不可退化。 |
| `src/lib/databaseCompletion.ts`（預計新增／尚不存在） | DB導覽介面實作者 | DB-UN | 規劃新增 completion provider；只消費 owner-bound metadata，UI compartment 接線由 panel writer 合作。 |
| `src-tauri/src/ssh_service.rs` | SSH/SFTP backend owner | SH-B | S01–S06/S09/S10/S12修改同一檔案及session lifecycle。這是同檔ownership限制，不代表所有SSH/SFTP runtime I/O需全域序列化；未先完成模組拆分時只允許一個writer。 |
| `src-tauri/src/sftp_transport.rs`（預計新增／尚不存在） | SFTP adapter owner | SH-A | 規劃新增adapter由獨立owner寫，讓其能與ssh_service backend平行開發。adapter真交付是SH-B production硬相依；共享contract可先mock。 |
| `src/state/sftpStore.ts` | SSH/SFTP frontend owner | SH-F | S01/S03/S07/S08/S11與F10共享store/terminal生命週期，集中ownership避免其他frontend包同時改queue、listeners與清理語意。 |
| `src/state/sshStore.ts` | SSH/SFTP frontend owner | SH-F | S01/S03/S07/S08/S11與F10共享store/terminal生命週期，集中ownership避免其他frontend包同時改queue、listeners與清理語意。 |
| `src/app/panels/SshPanel.tsx` | SSH/SFTP frontend owner | SH-F | S01/S03/S07/S08/S11與F10共享store/terminal生命週期，集中ownership避免其他frontend包同時改queue、listeners與清理語意。 |
| `src/terminal/SshTerminalSession.tsx` | SSH/SFTP frontend owner | SH-F | S01/S03/S07/S08/S11與F10共享store/terminal生命週期，集中ownership避免其他frontend包同時改queue、listeners與清理語意。 |
| `src/terminal/terminalOutputQueue.ts` | SSH/SFTP frontend owner | SH-F | S01/S03/S07/S08/S11與F10共享store/terminal生命週期，集中ownership避免其他frontend包同時改queue、listeners與清理語意。 |
| `src-tauri/src/git_service.rs` | Git runtime owner | RT-GIT | R07/R08都重寫run_git_inner；R15 batch stdin與其byte/deadline契約需一起驗證。檔案衝突不能誤畫成獨立的前後功能依賴。 |
| `src-tauri/src/git_log.rs` | Git runtime owner | RT-GIT | R07/R08都重寫run_git_inner；R15 batch stdin與其byte/deadline契約需一起驗證。檔案衝突不能誤畫成獨立的前後功能依賴。 |
| `src-tauri/src/lsp_service.rs` | LSP backend owner | RT-LSP | writer、stop、cancel和H02 trace producer共用檔案与server ownership；frontend LSP work可依LS-C平行mock，但不取得這份Rust檔案ownership。 |
| `src-tauri/src/logging.rs` | Logging owner | RT-LOG | writer、rotation、retention、cursor/export共享檔案與segment狀態，單一owner。LG-C是可先交付的契約，不要求RT-LSP等整個logging實作。 |
| `src-tauri/src/fs_service.rs` | File/Preview runtime owner | RT-FSP | read cap、request concurrency、revoke及TTL共用opened-handle/session語意；同包整合，其他runtime包可獨立平行。 |
| `src-tauri/src/preview_server.rs` | File/Preview runtime owner | RT-FSP | read cap、request concurrency、revoke及TTL共用opened-handle/session語意；同包整合，其他runtime包可獨立平行。 |
| `src-tauri/src/process_service.rs` | Process runtime owner | RT-PROC | bounded parser、tail、ready detection與event batching需一起保留語意；共同process-kill helper仍由整合者維護。 |
| `src-tauri/src/perf_service.rs` | Performance telemetry owner | RT-PERF | R14/F12在相同取樣/報告路徑，避免其他frontend worker同時改PerfBridge；不因此把telemetry綁成SSH或DB交付硬相依。 |
| `src/workbench/PerfBridge.tsx` | Performance telemetry owner | RT-PERF | R14/F12在相同取樣/報告路徑，避免其他frontend worker同時改PerfBridge；不因此把telemetry綁成SSH或DB交付硬相依。 |
| `src/editor/cmExtensions.ts` | FE-EDITOR | FE-Dirty、FE-Load | 同一 editor owner 管理 extension 與 host 接線，採短暫串行 handoff；store/模組開發可平行，共用檔案不是技術硬依賴。 |
| `src/editor/EditorPane.tsx` | FE-EDITOR | FE-Dirty、FE-LSP、FE-Load | 同一 editor owner 管理 extension 與 host 接線，採短暫串行 handoff；store/模組開發可平行，共用檔案不是技術硬依賴。 |
| `src/workbench/EditorArea.tsx` | FE-EDITOR | FE-Dirty、FE-Load | 同一 editor owner 管理 extension 與 host 接線，採短暫串行 handoff；store/模組開發可平行，共用檔案不是技術硬依賴。 |
| `src/state/fileTreeStore.ts` | FE-TREE | FE-TREE-C、FE-TreeCache、FE-TreeView | 先固定 row/cache 契約，再由 cache owner 與 view owner 分檔實作；涉及公開 selector/row model 的接線由 FE-TREE 整合，防止兩側各自發明 loading/evicted 語意。 |
| `src/workbench/FileTree.tsx` | FE-TREE | FE-TREE-C、FE-TreeView、FE-TreeCache | 先固定 row/cache 契約，再由 cache owner 與 view owner 分檔實作；涉及公開 selector/row model 的接線由 FE-TREE 整合，防止兩側各自發明 loading/evicted 語意。 |
| `src-tauri/src/search_service.rs` | SEARCH-BACKEND | SEARCH-C、SEARCH-Core、SEARCH-Parallel | 單一 owner 接 wire enum、coordinator、collector 與 worker；parallel worker 可依 frozen quota/cancel interface 開發，避免多人同改 run_search 的控制流程。 |
| `src/workbench/search/useWorkspaceSearch.ts` | FE-SEARCH-UI | SEARCH-C、SEARCH-View | 契約工作提供 fixtures 與 reducer 語意，UI owner 接入真實 channel，避免 event/state schema 與 grouping/virtualization 同時改動相同區塊。 |
| `src/workbench/search/WorkspaceSearchGroup.tsx` | FE-SEARCH-UI | SEARCH-View、SEARCH-C | 契約工作提供 fixtures 與 reducer 語意，UI owner 接入真實 channel，避免 event/state schema 與 grouping/virtualization 同時改動相同區塊。 |
| `src-tauri/tauri.conf.json` | FE-SECURITY | FE-CSP | 集中維護 main/preview capability 與 CSP；worker/chunk owner 提供資源需求，不各自放寬安全設定。 |
| `src-tauri/capabilities/default.json` | FE-SECURITY | FE-CSP | 集中維護 main/preview capability 與 CSP；worker/chunk owner 提供資源需求，不各自放寬安全設定。 |
| `src/editor/editorPane.lsp.test.tsx` | FE-EDITOR | FE-LSP、FE-Dirty、FE-Load | LSP recovery、lazy editor 與原 pane lifecycle 整合測試由單一 writer 接線；其餘 LSP transport/workspace/client tests 歸 FE-LSP。 |
| `src/state/workspaceStore.ts` | FE-EDITOR | FE-Dirty | 單一工作包直接負責；其他包只透過既定介面使用。 |
| `src/editor/documentRegistry.ts` | FE-DOCUMENT | FE-Document | 單一工作包直接負責；其他包只透過既定介面使用。 |
| `src/lsp/semanticTokens.ts` | FE-LSP | FE-LSP | 單一工作包直接負責；其他包只透過既定介面使用。 |
| `src/lsp/diagnosticsPull.ts` | FE-LSP | FE-LSP | 單一工作包直接負責；其他包只透過既定介面使用。 |
| `src/lsp/transport.ts` | FE-LSP | FE-LSP | 單一工作包直接負責；其他包只透過既定介面使用。 |
| `src/lsp/workspace.ts` | FE-LSP | FE-LSP | 單一工作包直接負責；其他包只透過既定介面使用。 |
| `src/lsp/lspManager.ts` | FE-LSP | FE-LSP | 單一工作包直接負責；其他包只透過既定介面使用。 |
| `src/lsp/lspExtensions.ts` | FE-LSP | FE-LSP | 單一工作包直接負責；其他包只透過既定介面使用。 |
| `src/lsp/transport.test.ts` | FE-LSP | FE-LSP | 單一工作包直接負責；其他包只透過既定介面使用。 |
| `src/lsp/workspace.test.ts` | FE-LSP | FE-LSP | 單一工作包直接負責；其他包只透過既定介面使用。 |
| `src/lsp/lspManager.test.ts` | FE-LSP | FE-LSP | 單一工作包直接負責；其他包只透過既定介面使用。 |
| `src/lsp/lspExtensions.test.ts` | FE-LSP | FE-LSP | 單一工作包直接負責；其他包只透過既定介面使用。 |
| `src/state/herdrStore.ts` | FE-HERDR | FE-Herdr | 單一工作包直接負責；其他包只透過既定介面使用。 |
| `src/app/workbench/HerdrNavContent.tsx` | FE-HERDR | FE-Herdr | 單一工作包直接負責；其他包只透過既定介面使用。 |
| `src/App.tsx` | FE-EDITOR | FE-Load | 單一工作包直接負責；其他包只透過既定介面使用。 |
| `src/workbench/MarkdownPreview.tsx` | FE-MARKDOWN | FE-Markdown | 單一工作包直接負責；其他包只透過既定介面使用。 |

## Runtime 平行與序列化

| 子系統 | 可受限平行 | 必須保留的序列化／原子性 |
|---|---|---|
| DB | 不同 DB actors 與 retained snapshot 唯讀可受限平行。 | 同 connection 的 query、metadata、context 操作服從 lease；同 script statements 有序，cancel settlement 完成才交接下一 run。 |
| Profile／結果預算 | 不同 database child 可獨立 connect；不可變結果可多讀。 | 同 profile credential ledger／generation 更新，以及 budget reservation、pin／evict 判斷必須原子。SQLite statement 留在所屬 worker。 |
| SSH／SFTP | 不同檔案有限傳輸、單檔有限 read-ahead、terminal 資料與控制分流。 | 同 session lifecycle／SFTP 初始化 single-flight；同目的地單 promotion owner，ACK 完整後才 close/promote。 |
| LSP／Git | 不同 LSP server 與不同 repo 的独立工作可有界平行。 | 同 server accepted frames FIFO；同 repo mutation 保留既有互斥，不因 runner 改善而平行 checkout／merge 等操作。 |
| Logging／Preview | log closed segments 的受控讀取、Preview requests 可平行。 | append／rotation 單 writer；retention cursor identity 與 capability revoke/generation 一致，不能以吞吐放寬權限。 |
| Shutdown | 獨立 service cleanup 在共同 deadline 內平行。 | 先停止 admission；各自遵守 owned-resource 清理順序，log 最後 drain，完成後才標 clean。 |

## 測試與量測排程

- 聚焦功能測試可平行，但 fixtures、temp dirs、ports、資料庫 namespace 與假時鐘必須隔離。共用 Docker DB 的 migration／破壞性 fixture 不同時跑。
- 同工作目錄的 dependency resolution、完整 Cargo／Vite build 與寫同產物的測試由 INT 排程；不以多 process 等同更快，避免 build lock 與 RAM 競爭。
- 同一主機的 CPU、input p95、RSS／GC、磁碟吞吐、cold start、build 或 terminal soak benchmark 一次只跑一個；暫停其他 build、壓測及 GUI 操作。
- cold／warm cache 分開，固定 fixture hash、app mode、viewport、instrumentation、worker 數與回收等待時間。bundle bytes 不代替 startup latency，peak RSS 不代替 retained heap。
- 不同主機可同時量測，但每台以自己的 baseline 做前後比較；不可直接跨硬體混合得出改善百分比。
- 最終執行 frontend lint/typecheck/test/build、三平台 cargo check、fmt／exact clippy baseline／Rust tests、三引擎 integration、真 Tauri GUI；至少 50 次 lifecycle 循環與 2 小時混合負載。原稽核的 8 小時 soak 保留為延伸穩定性測試，不宣稱本次已跑。

## 完整範圍對照

同一 finding 可以映射多個交付包，代表各部分都需驗收，不是重複計算改善數。單靠契約包完成不能關閉原 finding。

| 原稽核／新能力 | 工作包 |
|---|---|
| R01 | SEARCH-Core |
| R02 | SEARCH-Parallel |
| R03 | SEARCH-Core |
| R04 | LG-C、RT-LOG |
| R05 | LG-C、RT-LOG |
| R06 | LG-C、RT-LOG |
| R07 | RT-GIT |
| R08 | RT-GIT |
| R09 | LS-C、RT-LSP |
| R10 | RT-FSP |
| R11 | RT-FSP |
| R12 | RT-FSP |
| R13 | RT-PROC |
| R14 | RT-PERF |
| H01 | FE-CSP |
| H02 | LG-C、RT-LSP、RT-LOG |
| S01 | SH-C、SH-B、SH-F |
| S02 | SH-C、SH-A、SH-B、SH-F |
| S03 | SH-C、SH-A、SH-B、SH-F |
| S04 | SH-C、SH-A、SH-B |
| S05 | SH-C、SH-A、SH-B |
| S06 | SH-C、SH-A、SH-B |
| S07 | SH-C、SH-F |
| S08 | SH-C、SH-F |
| S09 | SH-C、SH-B |
| S10 | SH-C、SH-B |
| S11 | SH-C、SH-F |
| S12 | SH-C、SH-A、SH-B |
| F01 | FE-Dirty |
| F02 | FE-TreeView |
| F03 | FE-TreeCache |
| F04 | FE-TreeCache |
| F05 | FE-Document |
| F06 | SEARCH-View |
| F07 | FE-LSP |
| F08 | FE-Load |
| F09 | FE-Markdown |
| F10 | SH-F |
| F11 | FE-Herdr |
| F12 | RT-PERF |
| F13 | FE-Build |
| D01 | DB-A |
| D02 | DB-B |
| D03 | DB-C、DB-T |
| D04 | DB-C、DB-R、DB-UR |
| D05 | DB-UR |
| D06 | DB-C、DB-R、DB-UR |
| D07 | DB-SQ |
| D08 | DB-C、DB-N、DB-UN |
| D09 | DB-C、DB-Q |
| D10 | DB-BT |
| D11 | DB-C、DB-E |
| D12 | DB-C、DB-B、DB-Q |
| SEC01 | DB-S |
| R15 | RT-GIT |
| DBM01 | DB-C、DB-H、DB-UN |
| DBM02 | DB-C、DB-H、DB-Q、DB-UN |
| DBM03 | DB-C、DB-T、DB-N、DB-Q、DB-UN |
| DBM04 | DB-C、DB-R、DB-T、DB-UR |

## 證據與界限

- 工作包來源為 observations.json 的 55 項、已接受的多 database/schema 計畫與三條獨立唯讀相依評估；不重新聲稱做過全部產品測試。
- Codebase graph 優先用於定位，對 metadata drift 及 graph 未覆蓋處以 live source 為準。已核對 lib.rs 的 exit 清理與 clean marker 順序；共享檔案清單是 ownership 風險，不是窮盡所有可能的 diff。
- 本次交付僅文件及其 renderer／驗證資料；任務均是 planned，不代表改善已實作或已測得加速。
- 原稽核程式碼連結與行號以固定 commit 驗證；新 host／identity 變更只用來更新派工風險與路徑，未宣稱已驗收或已修復原 finding。
