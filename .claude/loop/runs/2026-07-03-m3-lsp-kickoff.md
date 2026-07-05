# LOOP RUN — m3-lsp-kickoff

- 日期：2026-07-03
- 狀態：RUNNING
- 任務原文：
  > m2 已完成，繼續下一個

## P1 提問

**需求（可驗證陳述）**
- R1: M3 範圍與關鍵取捨定案（semantic tokens 排程、rust-analyzer download spike 取捨、backlog 安排），決策記錄於 run file 與 plan。
- R2: 產出 M3 implementation plan 至 `docs/superpowers/plans/`（派 opus subagent 撰寫；格式對齊 M2 plan 前例），內容涵蓋 spec M3 全項＋spike「需自寫 extension 清單」＋M2 移交 backlog。
- R3: plan 通過獨立 opus reviewer 對抗 review（dry pass）。
- R4: 文件可解析、文內引用路徑全部存在（P5 純文件 battery）。
- （若使用者選「一路做完」：追加 R5+ 實作需求，屆時回 P1 補寫並重走 G1。）

**已查資料**（LEDGER／LEARNINGS／specs／plans／sdd／design——寫「查了哪裡、得到什麼」）
- `.claude/loop/LEDGER.md`：三筆 run 皆制度建設，無里程碑 run 前例。
- `.claude/skills/yuzora-loop/LEARNINGS.md`：已讀全部 9 條；關鍵——語義邊界決定列 BLOCKING、run file 當下建立、gui-acceptance 用於 jsdom 測不到的行為。
- `.superpowers/sdd/m2-progress.md` 尾段：**M2 判定 Ready**（GUI 手動驗收待使用者，見 m2-manual-checklist）；Minor ledger 全數 defer 至 M3 backlog，fable final review 新發現 m1-m7 亦移交 M3。
- `docs/superpowers/specs/2026-07-02-yuzora-mvp-workbench-design.md` Milestones：下一個為 **M3 — LSP**（adapter manager、vtsls/Pyright/rust-analyzer/marksman 四語言、diagnostics/hover/definition/references/rename/completion/code actions/symbols/semantic tokens/format on save、Settings LSP 區＋`~/.yuzora/lsp.json`＋guided install＋status bar、Markdown preview、非 blocking spike：rust-analyzer managed download）。
- `docs/superpowers/plans/`：尚無 M3 plan；M2 前例為 design spec（specs/）＋ implementation plan（plans/）→ SDD waves 執行。
- `docs/superpowers/spikes/2026-07-02-cm6-lsp-capability.md`（M1 blocking spike）：Gate＝**CONDITIONAL-GO**。選用 `@codemirror/lsp-client`（官方）；5 基礎項（diagnostics/completion/hover/rename/definition）零保留通過；需自寫清單 6 項——(1) code actions UI ~100-200 行（必要）、(2) semantic tokens highlighting ~150-250 行（可選建議排）、(3) rust-analyzer diagnostics pull-loop ~60-100 行、(4) rename documentChanges 相容層 ~20-30 行、(5) 自訂 Workspace 子類別（留後續里程碑）、(6) 跳轉後聚焦分頁（UX 小補）。另有 6 條「遇到的坑」。
- `docs/recap/yuzora-m1-closeout-m2-git-kickoff-*.html`：M2 啟動前例＝「先 brainstorming 確認範圍與取捨 → 產出 plan 至 docs/superpowers/plans/ → SDD waves 執行」。
- `.superpowers/sdd/m2-progress.md`：M2 Minor ledger 全數 defer 至 M3 backlog；fable final review 新發現 m1-m7 亦移交 M3。

**假設（非 BLOCKING，已選定預設值）**
- A1: 使用者宣告「m2 已完成」＝M2 手動 GUI checklist 已由使用者驗收（或自擔），本 run 不回頭重驗 M2。（依據：使用者原話；m2-progress 記載 Ready＋手動項待使用者。）
- A2: M2 移交 backlog（Minor ledger＋m1-m7）排入 M3 plan 的開頭清債 wave 或由 plan 撰寫者依相依性安排，不獨立開 run。（依據：M2 已裁決「移交 M3」；安排屬 plan 內部事務。）
- A3: 語言支援四種全做（vtsls／Pyright／rust-analyzer／marksman），依 spec M3 原文，不縮減。（依據：spec 未有變更訊號。）

**BLOCKING 待問**（無則寫「無」；有則先問使用者，得到回覆前不進 P2）
- Q1: 「繼續下一個」的範圍語義——kickoff（plan 定稿後停，等核准再實作）或一路做完 M3？（LEARNINGS：語義邊界必問）
  → **裁決（2026-07-03 使用者）：先 kickoff，plan 核准後再實作**。本 run 範圍＝取捨定案＋M3 plan＋對抗 review，交使用者核准即收尾。
- Q2: semantic tokens highlighting（spike 判非 blocking、建議排、~150-250 行）排入 M3 還是延後 polish？（產品取向）
  → **裁決：排入 M3**。
- Q3: rust-analyzer managed download spike（spec 原案非 blocking prototype）照做還是直接 guided manual install？（產品取向）
  → **裁決：照 spec 原案做 spike**（失敗 fallback guided manual install）。
- Q4: 若進實作：M3 動 src＋src-tauri、遠超 10 檔，是否啟用 worktree？（鐵律 4：大範圍應問）
  → **裁決：不啟用**，直接在工作目錄（沿 M1/M2 做法）。

**Gate G1**：✅ BLOCKING 全獲回覆（2026-07-03）。

## P2 規劃

**Baseline battery**（本 run 為純文件變更面；`git status --short` 快照＋plans 目錄現況，唯讀）

```
$ git status --short | head -30 && git status --short | wc -l
D  .gitignore
?? .claude/ ?? .design/ ?? .editorconfig ?? .gitignore ?? .vscode/ ?? CLAUDE.md ?? README.md
?? app-icon.png ?? bun.lock ?? components.json ?? docs/html/ ?? docs/recap/ ?? fixtures/
?? index.html ?? package.json ?? public/ ?? spikes/ ?? src-tauri/ ?? src/ ?? tsconfig.json
?? tsconfig.node.json ?? vite.config.ts
（共 23 行；全部未 commit，與 M2 收尾狀態一致）

$ ls docs/superpowers/plans/
2026-07-02-yuzora-m1-agent-loop-prompts.md  2026-07-02-yuzora-m1-editor-core.md
2026-07-02-yuzora-mvp-foundation-logs-shell.md  2026-07-03-context-menu-ui.md
2026-07-03-yuzora-m2-agent-loop-prompts.md  2026-07-03-yuzora-m2-git-search.md
（尚無 M3 plan）
```

**步驟**
1. Controller 親寫 plan brief（記帳文件）至 `.superpowers/sdd/m3-plan-brief.md` → verify: 檔案存在、含目標/白名單/non-goals/鐵律/verify/回報格式 → 預期: 三段式 brief 完整。
2. 派 opus implementer subagent 撰寫 `docs/superpowers/plans/2026-07-03-yuzora-m3-lsp.md` → verify: 檔案存在＋結構 grep（Goal/Global Constraints/File Structure/任務相依/Tasks/Verification）→ 預期: 對齊 M2 plan 前例結構。
3. Controller 復驗 coverage：spec M3 條目、spike 自寫清單 6 項處置、m1-m7 逐項處置、Minor ledger 安排、Q2/Q3 裁決反映 → verify: 逐項 grep plan → 預期: 全數命中。
4. P4：派獨立 opus reviewer 對抗 review plan → findings 裁決、回派修復、全新 reviewer 重跑 → 預期: dry pass。
5. P5：純文件 battery——plan 文內引用路徑存在檢查 → 預期: 零斷鏈。
6. P6：收尾、LEDGER、回報使用者（plan 待核准，實作另開 run）。

**預計變更檔案**
- `docs/superpowers/plans/2026-07-03-yuzora-m3-lsp.md`（新增，opus subagent）
- `.superpowers/sdd/m3-plan-brief.md`（新增，controller 記帳）
- `.claude/loop/runs/2026-07-03-m3-lsp-kickoff.md`、`.claude/loop/LEDGER.md`（記帳）

**不碰範圍（non-goals）**
- 不動 `src/**`、`src-tauri/**`、`spikes/**`、既有 specs/plans（唯讀）；不開始 M3 實作；不產獨立 design spec（plan 內含設計對稿策略與假設待裁決節）；不寫 agent-loop-prompts（M3 執行改走 yuzora-loop 閉環）；git 唯讀。

**風險**
- 可能弄壞：無既有行為（純新增文件）／方向錯誤風險 → 守門：P4 對抗 review＋使用者核准 gate（kickoff 裁決）。
- coverage 遺漏（spec 條目或 backlog 漏掉）→ 守門：步驟 3 controller 逐項 grep 檢核清單。

**驗收證據清單（實作前固定，只可加嚴）**
- E1: plan 檔存在且結構完整（結構 grep 輸出：Goal/Global Constraints/File Structure/任務相依/Tasks/假設待裁決/Verification 各節命中）。
- E2: coverage 檢核輸出——(a) spec M3 全條目（adapter manager/四語言/十項 LSP features/Settings＋lsp.json＋guided install＋status bar/Markdown preview/RA download spike）；(b) spike 自寫清單 6 項各有處置；(c) m1-m7 逐項有處置；(d) Minor ledger 有安排；(e) Q2 semantic tokens 排入、Q3 RA spike 照做——皆可在 plan 中 grep 到。
- E3: P4 dry pass＋反證紀錄 ≥3＋每條 finding 有裁決（run file P4 段）。
- E4: plan 文內引用路徑存在檢查輸出零缺。

**Gate G2**：✅ 每步有 verify；證據清單固定；baseline 已貼。

## P3 執行

**派工紀錄**（每個 brief：目標／變更範圍 → opus subagent 回報摘要 → controller 復驗 verify 結果 綠／紅）
- 步驟 1（controller 記帳）：brief 已寫至 `.superpowers/sdd/m3-plan-brief.md`（三段式：Goal／R-a..R-f／必讀清單／內容範圍／自寫清單／backlog／constraints／白名單／verify／report 格式）。verify：Write 成功、內容如上。綠。
- 步驟 2（opus implementer，2026-07-03 派出）：撰寫 `docs/superpowers/plans/2026-07-03-yuzora-m3-lsp.md`，白名單單檔、git 唯讀、假設待裁決節必備。→ 回報：完成，72KB、15 tasks、6+1 waves、假設待裁決 A1–A9、Coverage 對照節；宣稱僅動白名單一檔。
- 步驟 2-3 controller 復驗（親自重跑）：
  - 檔案存在 72292 bytes；`grep "^## "` 命中 Global Constraints／File Structure／任務相依／自動已驗 vs 待手動／Plan 級風險／假設待裁決／Coverage 對照／Verification；`grep -c "^### Task"`＝15（≥10）✅
  - coverage greps 全部非零：semantic tokens 14、rust-analyzer 25、vtsls 13、pyright 12、marksman 8、markdown-oxide 3、typescript-language-server 4、pylsp 3、lsp.json 10、stale 24、guided 19、Syntax only 10、code actions 11、documentChanges 9、pull-loop 5、Markdown preview 11、quarantine 10、退避/backoff 12、format on save 13、workspace symbols 3、didOpen 4 ✅
  - m1–m7 逐項存在（T1 M2 清債，含 find_col 修法細節、markDirty、generation、viewRegistry 比對等實質內容）✅
  - 使用者裁決反映：T8 標「使用者裁決排入」、T14 RA download spike＋A2 fallback、Global Constraints「worktree 不使用（使用者裁決）」、頂部聲明「Plan 核准後才實作（使用者裁決）」✅
  - plans 目錄僅新增 1 檔 ✅
  - 判定：**綠**。

**變更檔案**（subagent 回報、controller 彙整：路徑 — 為什麼）
- `.superpowers/sdd/m3-plan-brief.md` — plan 撰寫派工 brief（controller 記帳）。
- `docs/superpowers/plans/2026-07-03-yuzora-m3-lsp.md` — M3 implementation plan（opus implementer 撰寫）。

**計畫修正**（如有：原因＋新增步驟；沒有寫「無」）
- 無

## P4 對抗 review

**Reviewer 派工**（獨立 opus subagent，未參與撰寫；brief 給 R 清單＋變更檔案＋rubric，不給作者自辯）
- Round 1 reviewer（opus，2026-07-03 派出）：受審檔＝M3 plan；規格＝m3-plan-brief.md R-a..R-f；對照 spec／spike／m2-progress／實際程式碼抽查 ≥5 處；文件型 rubric 六項；≥3 反證。→ 回報：5 Important＋4 Minor，反證嘗試 9 條（通過 9 項含 R-a 五基礎 feature、R-b 六項處置、R-d 裁決、m1/m2/m3/m5 修法定位與基線引用皆核實）。

**Round 1（reviewer 回報摘要）**
- 反證通過（附證）：R-b 六項處置與 spike 一致；R-d 三裁決正確反映（plan:3/28/475/644）；m1 find_col panic 分析對照 search_service.rs:37-44 正確；基線數字逐字等於 m2-progress:209。
- Rubric：邊界(coverage)✗F5/F7 時序(內部一致)✗F1/F3/F4 state(可執行)✗F8 現況✗F2/F9 scope✓無越權 i18n✓

**Findings 裁決**（controller 親驗後逐條裁決）
- F1（Important）wave 0 T2/T3 同 wave 各改 lib.rs，與「零交集」「T4 唯一動 lib.rs」矛盾 → controller 親驗 plan:92/93/149/195 屬實 → **真**，派修：序列化 T2→T3（或 lead 預註冊 mod 協定），修正兩處錯誤宣稱（T14 亦動 lib.rs 需一併措辭）。
- F2（Important）m7 指錯檔：discardAll/gitDiscard 在 GitNavContent.tsx:177-183，LocalChangesTab 無 → controller 親驗 grep 屬實 → **真**，派修：m7 目標檔與測試檔改 GitNavContent。
- F3（Important）m4 修 unregisterView 簽名必改 EditorPane.tsx:78，T1 Files 未列；相依表過度宣稱 T1 動 StatusBar → controller 親驗屬實 → **真**，派修：T1 Files 補 EditorPane.tsx、修 wave 註記。
- F4（Important）wave 2 T6/T13 同 wave 並發 bun add 撞 package.json/bun.lock → controller 親驗 plan:376/432/625/634 屬實 → **真**，派修：bun add 序列化（wave 註記明確排序）。
- F5（Important）document/workspace symbols 無 owning task/steps/驗收，A3 且提供 defer 選項，弱化 spec 硬需求 → controller 親驗 grep 僅 A3 一處 → **真**，派修：symbols 給 owning steps＋verify＋驗收（預設在 M3 範圍內），A3 僅保留 UI 型態選擇。
- F6（Minor）「11 個 lsp_* handler」與實際 8 個不符 → 親驗 plan:237/307 vs 271-287 屬實 → **真**，派修：改 8（＋T14 再加 1 說明）。
- F7（Minor）brief 指名代表 Minor T15/T18/T4-T7 未個別處置 → **真**，派修：T15/T18 入 T1（T18 revealLine clamp 連動 EditorPane 與 F3 同批）；T4/T7 linked worktree 明確 defer 附理由。
- F8（Minor）T1 Step 2 RED 指令未涵蓋 m5/m6/m7 新測試 → **真**，派修：擴指令路徑。
- F9（Minor）File Structure 總表漏 editor.css → 親驗屬實 → **真**，派修：補列。
- 誤判：0 條。九條全真，回 P3 派修（原 implementer 續任撰寫者，角色不變）。

**修復（原 implementer，2026-07-03）＋controller 復驗（親自重跑 grep）**
- F1: `grep -c "唯一動 lib.rs"`＝0；lead 預註冊 4 mod 行＋空 stub 協定寫入 :46/:94/:95/:160/:206（M2 D2 前例）✅
- F2: m7 全面改指 GitNavContent.tsx（:85/:110/:121），LocalChangesTab 僅剩澄清句 ✅
- F3: T1 Files 含 EditorPane.tsx（m4 呼叫端＋T18 clamp）；相依表明標「T1 不動 StatusBar.tsx」✅
- F4: wave 2 註記 bun add 序列化（lead 統一裝齊或 T6 先行；:96/:445/:645）✅
- F5: symbols owning＝T10（symbols.ts＋SymbolPicker.tsx＋測試＋Step1/2＋驗收＋gui-acceptance；:562-587）；A3 改僅裁 UI 型態 ✅
- F6: `grep -c "11 個"`＝0，統一 8 handler＋T14 第 9 個分開措辭 ✅
- F7: T15/T18 入 T1（:122）；linked worktree 明確 defer＋理由（:112/:123）✅
- F8: T1 Step 2 RED 指令擴至全部 T1 測試路徑（:138-140）✅
- F9: File Structure 補 editor.css（:67）✅
- 結構重驗：15 tasks、必要節全在。判定：**全綠**，進 Round 2（全新 reviewer 整輪重跑）。

**Round 2 reviewer 派工**（全新 opus subagent，未參與撰寫與前輪 review；整輪從頭審，抽查與前輪不同的現況宣稱 ≥5 處）
- 2026-07-03 派出。→ 回報待收。
- **計畫修正（2026-07-03 session 續接）**：前 session 被 /clear，Round 2 subagent 回報遺失。使用者於新 session 明確指示「不使用 subagent、不使用 git add/commit、不使用 worktree」——對話內明確指示優先於鐵律 1/2（一次性使用者覆寫，不援引為先例）。Round 2 改由 Fable controller 親自對抗 review（fresh context 成立：本 session 未參與 plan 撰寫），rubric 與抽查要求不變（抽查與 Round 1 不同的現況宣稱 ≥5 處、反證 ≥3 條）。如有 findings，修復亦由 controller 親寫（同一使用者覆寫）。**限制註記**：本輪起 reviewer／撰寫者／裁決者為同一人（使用者指示所致），對抗性弱於雙 subagent 模式——已以「先抽查實際程式碼再對照宣稱」的順序減輕自我說服。

**Round 2（Fable 親審，2026-07-03）— 現況抽查 8 處（與 Round 1 不重疊）**
- ✅ EditorPane.tsx 確為 98 行；`unregisterView(path)` 單參數確在 :78；`revealLine` :28 僅 `Math.min` 上界、無下界 clamp（`doc.line(0)` 會 throw）——T1 m4/T18 宣稱屬實。
- ✅ StatusBar.tsx:124-126 右側確為 `{languageFromPath(activePath)} · Syntax only` : `未開啟檔案`——T11 現況宣稱屬實。
- ✅ lib.rs 模組清單 10 個與 plan:38 逐字一致；`logging::LogSink` 存在（lib.rs:14）。
- ✅ package.json：cmdk 有、`@codemirror/lsp-client` 無、lang-{js,py,rust,md} 齊——File Structure 現況宣稱屬實。
- ✅ 基線 vitest 172/172（29 檔）／cargo 51/51＋1 ignored 與 m2-progress:209 逐字一致；「m1–m7 移交」確在 m2-progress:203。
- ✅ types.ts OpenFileResult kinds＝full/limited/tooLarge/binary/nonUtf8Readonly；workspaceStore 有 workspacePath/openTab/requestReveal；ipc.ts 有 openFile＋Channel（searchWorkspace 前例）——T5/T6/T10 依賴的既有 API 全部存在。
- ✅ T12「既有 Settings 測試檔」屬實（初查 grep SettingsDialog 零命中疑造假，深查後 panels.test.tsx:89 有 "Settings dialog content" describe、appShell.test.tsx:164 有開啟測試——經 AppShell 渲染而非直接 import，宣稱成立）。
- ✅ T1 引用之既有測試檔全部存在（gitStore/externalChangeResolver/gitNavContent/branchPopover/statusBar/searchResults）；viewRegistry.test.ts 不存在但 plan 已標「若無則新增」。
- 反證通過另兩條：8 handler＋T14 第 9 個的計數在 plan:46/248/318 三處一致；spike 自寫清單 6 項處置與 spike「M3 整合建議」1–6 逐項吻合、坑 6 條在 T4/T6/T10/風險表各有著落。

**Round 2 Findings 裁決**（controller 親驗程式碼後裁決，全部為真）
- F10（Important）T11「Missing/Failed 點擊進 Settings 對應語言」與 Files 清單矛盾：Settings 開啟為 AppShell 區域 state（AppShell.tsx:47 `settingsOpen`、:137-143 `handleOpenSettings`、:247 `<StatusBar />` 無 props），SettingsDialog section 為內部 state（:271），無任何全域 API 可從 StatusBar 開啟＋指定 section；T11 勢必動未列檔（AppShell/uiStore/SettingsDialog），且 SettingsDialog 是 T12 同 wave 5 檔案——「不同檔，併行」宣稱不成立 → **真**，修：openSettings API 劃給 T12（含 AppShell.tsx＋initialSection prop），wave 5 序列化 T12→T11。
- F11（Minor）T10 SymbolPicker 入口宣稱「既有 command palette」屬實（CommandPalette.tsx，⌘K，AppShell:249 掛載），但入口檔 CommandPalette.tsx 未列 T10 Files／File Structure → **真**，修：補列。
- F12（Minor）T13 Markdown preview 掛載點（TabBar/group-actions/EditorPane 旁）未列 File Structure 總表（與 Round 1 F9 同類）→ **真**，修：預設 TabBar.tsx（M3 無他人動）並補列，明確不動共用檔 EditorPane。
- F13（Minor）spec「diagnostics debounced didChange、save 時 flush」的 **save flush** 只出現在 T6 內容範圍句與 Coverage 表，無 owning step／verify／測試 → **真**，修：入 T10 save 閉包（與 format on save 同位）＋Step 1 測試。
- F14（Minor）lspManager/lspStore 介面引用 `FileGrade` 型別但 plan 未定義（types.ts 現況無此型別，T5 型別清單亦無）→ **真**，修：T5 補 FileGrade 型別＋推導函式。
- nit（順手修）：T1 Step 2 RED 指令 `src/app/workbench/FilesNavContent` 大小寫與實際測試檔 `filesNavContent.test.tsx` 不符（同指令 `src/app` filter 已涵蓋故無實害）。
- 誤判：0 條。五條全真＋1 nit，由 controller 親修（使用者覆寫）。

**修復（controller 親寫，2026-07-03）＋驗證（grep 重跑）**
- F10: wave 5 改「序列：T12 先、T11 後」＋現況行號證據；openSettings API 劃 T12（Files 補 AppShell.tsx＋uiStore.ts＋SettingsDialog initialSection prop＋API 合約測試）；T11 明確只動 StatusBar.tsx。`grep openSettings`＝8 處、「不同檔，併行」＝0 ✅
- F11: T10 Files 補 CommandPalette.tsx（＋可選 AppShell 快捷鍵）；入口句改「已核實存在（AppShell:249）」；wave 4 註記更新。`grep CommandPalette`＝6 ✅
- F12: T13 掛載點定為 TabBar.tsx（預設，不動 EditorPane）；File Structure 補列；wave 2 註記更新。`grep TabBar`＝2 ✅
- F13: T10 save 閉包補「flush pending didChange」owning 實作＋Step 1 測試＋驗收標準；自動已驗清單同步。`grep "save flush|flush pending didChange"`＝5 ✅
- F14: T5 型別補 FileGrade＋fileGradeOf（含推導測試）；deriveDisplayState 註解對齊。`grep FileGrade|fileGradeOf`＝6 ✅
- nit: T1 Step 2 指令改 `filesNavContent.test.tsx` ✅
- 共用檔清單補 AppShell.tsx／CommandPalette.tsx；結構重驗：15 tasks、必要 ## 節全在 ✅

**Round 3（修復後整輪重驗，controller，2026-07-03）**
- 範圍：全 plan 結構重 grep、五處修復落點與其對 wave 表／共用檔清單／Coverage 對照／自動已驗清單的連動一致性、全文引用路徑存在檢查（見 P5）、修復區與未修區交界重讀。
- 過程中緊了兩處修復自身的措辭不一致（deriveDisplayState 註解 hasVeryLongLine→veryLongLine、wave 2 T13 掛載點句），屬修復驗證範圍非新 finding。
- 新 finding：**0 條 → dry pass**。
- 誠實註記：Round 2/3 為同一人審修（使用者覆寫所致），對抗性弱於協定原型；緩解措施＝先抽程式碼後對宣稱、修復後全域連動重驗。

**Gate G4**：✅ dry pass（Round 3 零新 finding）＋反證紀錄 9 條（Round 2 抽查 8 處＋計數/處置一致性 2 條，全數附檔案:行）＋14 條 findings（R1×9＋R2×5）全有裁決紀錄。

## P5 驗收

變更面＝純文件（plan md）。Battery 由 controller 親自執行：

- **E1 結構完整**：`grep -c "^### Task"`＝15；`grep -n "^## "` 命中 Global Constraints(:20)/File Structure(:36)/任務相依(:94)/自動已驗 vs 待手動(:734)/Plan 級風險(:749)/假設待裁決(:766)/Coverage 對照(:778)/Verification(:787)。**PASS**
- **E2 coverage grep 全數非零**（實跑輸出）：semantic tokens 14、rust-analyzer 25、vtsls 13、pyright 12、marksman 8、markdown-oxide 3、typescript-language-server 4、pylsp 3、lsp.json 10、stale 25、guided 19、Syntax only 10、code actions 11、documentChanges 9、pull 18、Markdown preview 11、quarantine 10、退避 9、format on save 13、workspace symbols 12、didOpen 4、m1 17、m7 11、使用者裁決 8。**PASS**
- **E3 P4 dry pass**：見 Gate G4。**PASS**
- **E4 引用路徑零斷鏈**：regex 抽取 plan 全部 repo 路徑逐一檢查——MISSING 共 30 個，逐項比對後**全數**為 plan 標記 [新]／Create／「若無則新增」的計畫新增檔（lsp_*.rs×4、src/lsp/×12、lspStore×2、LspBridge/MarkdownPreview/SymbolPicker 及測試、checklist、gen-lsp-fixtures、viewRegistry.test.ts、editorPane.lsp.test.tsx）；既有檔引用 0 斷鏈。**PASS**
- Baseline 比對：本 run 只新增/修改 docs 與記帳文件，未動 src/**（`git status` 唯讀快照與 P2 一致，無新增 src 變更）。

**Gate G5**：✅ E1–E4 全部有實跑證據且 PASS。

## P6 收尾

- **狀態：DONE**（kickoff 範圍完成：取捨定案＋M3 plan＋對抗 review dry pass；plan 待使用者核准，核准後實作另開 run——Q1 裁決）。
- **最終變更檔案**：
  - `docs/superpowers/plans/2026-07-03-yuzora-m3-lsp.md`（opus implementer 初稿＋R1 修復；R2 五 findings＋nit 由 controller 親修——使用者覆寫）
  - `.superpowers/sdd/m3-plan-brief.md`（controller 記帳）
  - `.claude/loop/runs/2026-07-03-m3-lsp-kickoff.md`、`.claude/loop/LEDGER.md`、`.claude/skills/yuzora-loop/LEARNINGS.md`（記帳）
- **需使用者確認**：plan 核准＋假設待裁決 A1–A9（見 plan「假設待裁決」節）。
- **殘留事項**：A1–A9 裁決後回填對應 task；實作 run 依 plan wave 0 起跑（lead 預註冊 lib.rs 4 mod 行＋空 stub）。
- **自我改進三問**：
  1. 沒攔到的閘門：R1 reviewer rubric 未含「UI 觸發機制現況可達性」→ F10 到 R2 才被抓 → LEARNINGS 追加 [tighten]。
  2. 浪費時間的規則：無明顯者；不自行放寬。
  3. 陷阱：/clear 使 in-flight subagent 回報遺失、該輪必須整輪重做 → LEARNINGS 追加 [trap]。
