# yuzora-loop LEARNINGS

規則：只增不刪、一條一行；與 SKILL.md 衝突時以**較嚴**者為準。放寬既有條目需使用者在對話中明確批准，批准後在該條末尾標註（放寬批准：YYYY-MM-DD）。

格式：`- YYYY-MM-DD [tighten|trap|convention] 一句話（來源：runs/<file> 或出處）`

## 條目

- 2026-07-03 [convention] jsdom 測不到 watcher／tab／dialog／視覺行為——這類證據一律排 gui-acceptance 實機驗收（來源：.superpowers/sdd M1/M2 歷史）
- 2026-07-03 [trap] 外部修改測試只能用 `fixtures/out/`，動到 Vite/Tailwind 掃描範圍（README、src/**）會觸發 full-reload 毀掉 app 狀態（來源：gui-acceptance skill §0）
- 2026-07-03 [trap] `html { font-size: 13px }` 把全域 rem 基準縮小——任何用 rem 的 shadcn／第三方元件尺寸都會偏小，改 UI 尺寸前先想到這條（來源：.superpowers/sdd/progress.md Settings Switch 事故）
- 2026-07-03 [convention] tauri dev 跑的是裸 binary，computer-use 授權不到；實機驗收必走 debug bundle → /Applications（來源：gui-acceptance skill §1）
- 2026-07-03 [convention] 鐵律 v1.1 修訂：subagent 允許且僅限 opus（撰寫＋對抗 review，controller/worker 分離）、git 開放唯讀、worktree 可於 P1 詢問（預設不用）（放寬批准：2026-07-03，來源：runs/2026-07-03-loop-rules-v1-1.md）
- 2026-07-03 [tighten] run file 與 LEDGER 行必須在 TRIAGE 進閉環當下建立（狀態 RUNNING）、邊做邊填；事後補寫會產生斷鏈——v1.1 修訂即被獨立 reviewer 抓到引用尚不存在的 run file（來源：runs/2026-07-03-loop-rules-v1-1.md F1）
- 2026-07-03 [tighten] 每輪 P4 review 派全新 reviewer、run 內角色固定（reviewer 與撰寫者不得互換）；fallback 退自寫模式需附派工失敗證據，宣稱無證據＝違規（來源：runs/2026-07-03-loop-rules-v1-1.md F3/F4）
- 2026-07-03 [tighten] 凡涉及使用者原話語義邊界的決定（誰撰寫、範圍多大）一律列 P1 BLOCKING 問使用者，不得降級為非 BLOCKING 假設自行放行（來源：runs/2026-07-03-loop-rules-v1-1.md N2）
- 2026-07-03 [convention] Q1 裁決定案：controller 親手只寫記帳文件（run file／LEDGER／LEARNINGS／brief），其餘撰寫含閉環協定修訂一律派 opus subagent；2026-07-03 Fable 親筆＋親審為使用者核可的一次性例外，不得援引為先例（來源：runs/2026-07-03-loop-rules-v1-1.md Q1）
- 2026-07-03 [tighten] plan 型 P4 review rubric 必含「UI 觸發機制現況可達性」：凡宣稱「A 元件觸發/開啟 B 介面」，須核實觸發通道現況（state 在哪、有無全域 API、props 怎麼到）——區域 state 無全域入口是 wave 並行宣稱的盲區（來源：runs/2026-07-03-m3-lsp-kickoff.md F10）
- 2026-07-03 [trap] /clear 會使 in-flight subagent 的回報永久遺失，該輪 review/派工必須整輪重做；長輪派工前讓 subagent 中間結果落盤，或於 run file 標明「回報待收」以便續接 session 辨識斷點（來源：runs/2026-07-03-m3-lsp-kickoff.md Round 2 重做）
- 2026-07-03 [trap] 派工可能「空轉完成」——subagent 0 個 tool use、數秒即回報且內容與 brief 無關（樣板文字）；判準：tool_uses=0 或回報離題。處置：SendMessage 續推同一 agent 即復原，記派工失敗一次；這不構成退自寫的 fallback 條件（來源：runs/2026-07-03-m3-wave0.md T2 首輪）
- 2026-07-03 [convention] P2 baseline 一律以實測為準——plan／prompts 記載的基線數字會過時（本次 plan 寫 vitest 172/cargo 51，實測 305/78 且 fmt 紅）；「不可退化」以當下實測起算，baseline 紅燈記計畫修正處理（來源：runs/2026-07-03-m3-wave0.md）
- 2026-07-03 [trap] touch 建的 0-byte .rs stub 會被 cargo fmt --check 報「缺換行」diff——預註冊 stub 至少寫入一個換行 byte（來源：runs/2026-07-03-m3-wave0.md）
- 2026-07-04 [tighten] 不可信 HTML（markdown preview／LSP tooltip）注入到有全域 CSS 框架（Tailwind）的 body portal 時，sanitizer 注入面窮舉**必須含應用層屬性 class/id**，不只 DOMPurify 標籤/協定放行面——攻擊者可用已編譯的 utility class（fixed/inset-0/z-50）重建 overlay。且逐屬性黑名單是在跟全域 CSS 賽跑，應優先做**CSS 定位模型根因防禦**：對注入容器套 contain:paint（或 transform）建立 containing block，一次斷所有 position:fixed 逃逸（來源：runs/2026-07-03-m3-wave2.md R9→R10→R11 三輪才收斂）
- 2026-07-04 [trap] opus 額度/串流退化期（接近 session limit reset）派工會連續失敗：session limit／connection closed mid-response／stall 600s 三種模式。處置：單一 Agent 比 workflow 易過（輕量、可 SendMessage 續推）；lens/brief 加「優先讀碼推理、node repro 只單發短指令勿跑會 hang 的 watch」可避 stall；連 2-3 次基礎設施失敗即停硬重試，靠 cron 心跳＋額度 reset 自動重試，別亂槍打鳥（來源：runs/2026-07-03-m3-wave2.md Round 12b）
- 2026-07-04 [convention] 多輪 P4 dry-pass 判定須核「本輪所有 lens 是否都完成」——workflow 回報 dryPass:true 可能只代表「已完成 agents 中 0 findings」，若有 lens 因 API/額度失敗（見 failures 欄），關鍵 lens 未跑完就不算 dry pass，須補跑（來源：runs/2026-07-03-m3-wave2.md Round 12 containing-block-attack lens 失敗）
- 2026-07-05 [trap] subagent Write 可能夾入 NUL byte（0x00）——檔案被 grep/rg/file 判 binary 而對程式碼搜尋隱形，tsc/vitest 照綠、git 因 NUL 落在 8000-byte 偵測窗外仍當 text diff；每輪修復後掃 `find src -type f -print0 | xargs -0 perl -le '...\x00...'` 全 src（來源：runs/2026-07-04-m3-wave4.md R2-1）
- 2026-07-05 [tighten] 修復批次的單檔 vitest 驗證不夠——controller 整合驗證必跑**全量** vitest 並檢查「Errors／Unhandled Rejection」行：N passed 但 Errors 1 不算綠（跨測試檔交互如「新 production 依賴未在別的測試檔 mock」只在全量跑才現形）（來源：runs/2026-07-04-m3-wave4.md 批次 C follow-up）
- 2026-07-05 [tighten] async IIFE 的 unhandled-rejection 修復必須**窮舉該 IIFE 全部 reject 源**（await 鏈逐個列：getDocument/request/initializing…），只補單一 .catch 會留下「保證不完整」——review 時對 `void (async () => {})()` 逐一核每個 await 是否可 reject、catch 是否覆蓋（來源：runs/2026-07-04-m3-wave4.md R3-1）
- 2026-07-05 [convention] teammate subagent 完成後常只發 idle_notification 而未回傳內容——SendMessage 請其「to: main 回傳完整回報」即可取回，勿當派工失敗重派（來源：runs/2026-07-04-m3-wave4.md 多次）
- 2026-07-04 [trap] ultracode workflow 內層 parallel() 必須傳 thunk（`() => agent(...)`）非 agent(...) 呼叫（promise）；傳 promise 會令 parallel 同步拋錯、pipeline 把該 item 丟成 null、filter(Boolean) 全濾掉 → 回報**假 `survives:0`**（reviewer 其實有 findings、refuter 也真跑了，只是聚合崩）。防呆：dry pass 前必看 `<failures>` 欄＋讀 journal.jsonl 核 reviewer findings 與 refuter verdicts 真實數；本次靠此撈回 25 findings（來源：runs/2026-07-04-m3-wave3.md Round 1）
- 2026-07-04 [convention] 派修 brief 應明確授權 implementer「controller 的指示若在型別/執行期不安全，須停下回報並改採安全等價」——本次 C2 指示 lspLintSource `return null`，但已安裝 @codemirror/lint@6 的 LintSource 型別不含 null 且執行期會 concat(null) 崩潰；Agent A 正確攔截改用 forEachDiagnostic 重發現有診斷（語意等同保留前次、型別/執行期皆安全），避免 controller 拍腦袋指令釀 runtime crash（來源：runs/2026-07-04-m3-wave3.md C2）
- 2026-07-05 [trap] keyed-remount 生態（key=path:generation）中「先 bump 再 fetch」的 eager bump 是資料遺失 time bomb——世代號只可在新內容真正到位後遞增，否則 fetch 失敗後任何 re-render 都會 remount 掉活 pane、cleanup 的 write-back 被 gen guard no-op 而永久丟失未存 buffer；review promise-settle 類修復必追完整副作用鏈：store 變更→訂閱者 re-render→keyed remount→cleanup guard（來源：runs/2026-07-05-m3-wave5.md R2B-F1→R3-F1）
- 2026-07-05 [tighten] 裁決修法時修根因不修引信——R3-F1 reviewer 首選方案「不清 flag 避開 re-render」只拆近端引信（污染的 generation 仍在，任何他源 re-render 照樣引爆）；controller 必問「此修之後，原危險狀態本身還存在嗎」，答存在即改修根因（來源：runs/2026-07-05-m3-wave5.md R3-F1 裁決）
- 2026-07-05 [convention] 測試要斷言某模組的內部行為（如 generation bump 時序）時不得 mock 該模組本身，改 mock 其 IO/IPC 邊界（真 reloadDocument＋mock ipc.openFile）——否則斷言測的是 mock 而非實作，綠燈無證據力（來源：runs/2026-07-05-m3-wave5.md R3-F1 bridge 測重構）
- 2026-07-05 [trap] Tauri v2 非 async 的 `#[tauri::command]` 跑在 main thread——含阻塞 IO（下載/子行程/網路 git）的 command 會餓死全部 IPC；長阻塞 command 必 `async fn`＋`tauri::async_runtime::spawn_blocking`。本 codebase 其他 sync command（git_push/git_fetch 等）同屬此暗雷，未修（來源：runs/2026-07-05-m3-wave6.md W6A-F2）
- 2026-07-05 [tighten] 「動作端」與「生效端」的解析邏輯必須同源——install 用自寫 resolve_active（只讀 global default）而啟動用 lsp_config::resolve_server（含 workspace override），兩鏈分岔即「裝的≠啟動會用的」死路；review rubric 對這類雙端功能必問「兩端是否呼叫同一 resolve 函數＋同一 canonical 化」（來源：runs/2026-07-05-m3-wave6.md W6A-F1）
