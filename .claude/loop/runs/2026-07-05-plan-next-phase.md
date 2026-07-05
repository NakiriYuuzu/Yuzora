# LOOP RUN — plan-next-phase（M4 kickoff：plan 定稿）

- 日期：2026-07-05
- 狀態：RUNNING
- 任務原文：
  > 開始規劃下一個 phase

## P1 提問

**需求（可驗證陳述）**
- R1: 確認「下一個 phase」的範圍定義＝roadmap 的 M4（Terminal、Preview 與 process manager），並彙整 M3 全部殘留事項的去處（併入 M4 task 或明列 defer＋理由）。
- R2: 產出 M4 實作 plan `docs/superpowers/plans/2026-07-05-yuzora-m4-terminal-preview.md`（opus subagent 撰寫；結構仿 M3 plan：Global Constraints／File Structure 接真實現況／tasks 含逐項驗證／waves 相依／spike 定義／自動 vs 實機清單／風險／A1..An 待裁決／Coverage／Verification）。
- R3: plan 經獨立 opus reviewer 對抗 review 達 dry pass（0 條未被裁決推翻的新 finding）。
- R4: 設計裁量點彙整為 A1..An 待使用者裁決清單（附建議），plan 定稿後於回覆呈給使用者——沿用 M3 模式（kickoff → adjudication 另開 run）。

**已查資料**
- LEDGER：M1–M3 全 DONE；M3 自動側完結（closeout：fable 最終 review 3 修＋M3C-F1 修→CLEAN；battery 534/168+1 全綠）；唯一交付待辦＝m3-manual-checklist 實機驗收（使用者配合項）。
- LEARNINGS 全文已讀（37 條）；與 M4 直接相關：Tauri 非 async command 跑 main thread（阻塞 IO 必 async+spawn_blocking，git_push/git_fetch 同屬暗雷未修）、run_command kill 不殺 process group、jsdom 測不到 watcher/tab/dialog/視覺→實機、動作端與生效端解析必同源。
- 設計文件 `docs/superpowers/specs/2026-07-02-yuzora-mvp-workbench-design.md`：Milestones :84-126（M4=:112-118，M5=AgentZone/Git advanced/收尾）；Terminal 章 :238-244（xterm.js+portable-pty、ConPTY、Job Objects、log lifecycle only）；Live Preview 章 :246-255（child webview spike、偵測三路、port occupied flow、共用 process 基建）；Settings :283-292（preview command/port）；錯誤處理 :299-309（dev server failed→可操作錯誤）；測試策略 :361-377；風險 :396+（:407 preview 嵌入未定→M4 開頭 spike）。
- M3 plan `2026-07-03-yuzora-m3-lsp.md` 結構已抽出（15 tasks/章節模板，供 M4 plan 仿）。
- M3 殘留彙整（來源 wave4/wave6/closeout run files）：
  1. process-group kill：run_command（npm/pip 600s 逾時 kill 不殺孫程序）＋git_service run_git 同 pattern——與 M4 process manager「process tree 清理」天然同批。
  2. LSP lifecycle stop 側 backlog：stop 不 emit server-status／initialized 無 per-language reset／profile 切換不重掛。
  3. EditorPane 開檔失敗 error-pane UX（getDocument 鏈無 .catch；stale URI→unhandled rejection＋空 pane＋孤兒 tab）（wave4 F1 defer）。
  4. format tabSize:4 寫死（wave4 R3-4 defer→T12 過渡註解；wave5 是否已接 Settings 需撰寫者核實現況）。
  5. Windows rust-analyzer zip 解壓（wave6 延後項）。
  6. m3-manual-checklist 實機驗收＝使用者配合項，非 M4 範圍，回覆中提醒。

**假設（非 BLOCKING，已選定預設值）**
- A1: 「下一個 phase」＝M4（依據：roadmap 順序明確、M3 已閉合；使用者原話無其他指涉）。
- A2: 本 run 交付＝plan 定稿＋A 待裁決清單（依據：M3 既定模式 kickoff→adjudication→session-prompts 分 run；「開始規劃」對應 kickoff）。
- A3: 不啟用 worktree（依據：鐵律 4 預設不用；本 run 僅新增 docs 下 1 個 md＋記帳文件，不動 src，低風險）。

**BLOCKING 待問**
- 無（設計裁量點依 M3 模式收進 plan 的 A1..An，待 plan 定稿後由使用者裁決）。

**Gate G1**：✅

## P2 規劃

**Baseline battery**（純文件型任務：baseline＝git status 快照；不動 src 故不跑五件套，P5 以 git diff 證明變更面）

```
$ git status --short
 M .claude/loop/LEDGER.md
 M .claude/loop/runs/2026-07-05-git-commit-all.md
?? .claude/loop/runs/2026-07-05-plan-next-phase.md
$ git log --oneline -3
ca95617 feat(app): 🚀 Import yuzora workbench (M1–M3)
171b326 feat(styles): 🚀 Add initial design system files
```

**步驟**
1. Controller 寫 W1 brief（plan 撰寫）→ verify: run file 留摘要 → 預期: brief 含輸入材料/結構要求/殘留清單/A 要求/鐵律轉述
2. 派 opus implementer 撰寫 M4 plan → verify: 檔案存在＋章節齊＋回報自檢 → 預期: 結構仿 M3 plan、殘留逐項有去處
3. Controller 親驗草稿（讀全文、抽驗引用路徑存在、殘留收錄逐項核）→ verify: grep 引用路徑 → 預期: 零斷鏈
4. 派獨立 opus reviewer 對抗 review（rubric 見 brief）→ verify: findings＋反證 ≥3 → 預期: 回報格式合規
5. 裁決 findings→（如有真 finding）回派修→整輪重跑全新 reviewer 至 dry pass → verify: run file 裁決紀錄 → 預期: dry pass
6. P5 文件 battery＋git 變更面比對 → verify: 見 E1-E5 → 預期: 全 PASS

**預計變更檔案**
- `docs/superpowers/plans/2026-07-05-yuzora-m4-terminal-preview.md`（新增；opus 撰寫）
- `.claude/loop/runs/2026-07-05-plan-next-phase.md`（run file；controller）
- `.claude/loop/LEDGER.md`（controller）

**不碰範圍（non-goals）**
- 不動 `src/**`、`src-tauri/**`、既有 specs/plans/checklists；不實作任何 M4 內容（含 spike）；不改 m3-manual-checklist；不 git 寫入。

**風險**
- 可能弄壞：無產品碼變更；風險在 plan 品質（虛構 API、殘留漏收、範圍蔓延）／守門：P4 獨立 review rubric 明列「技術事實核實」「殘留對照」「範圍對 design doc」。

**驗收證據清單（實作前固定，只可加嚴）**
- E1: plan 文件存在且必要章節齊（範圍/non-goals、Global Constraints、File Structure、tasks 含逐項驗證、waves 相依、spike wave-0 blocking 定義、自動 vs 實機清單、風險、A1..An、Coverage、Verification）。
- E2: plan 文內引用的既有檔案路徑抽驗全部存在（零斷鏈）。
- E3: P4 dry pass＋反證紀錄 ≥3＋每條 finding 有裁決紀錄。
- E4: `git status --short` 對照 baseline：新增/修改僅限預計變更檔案清單（src/src-tauri 零觸碰）。
- E5: P1 彙整的 M3 殘留 5 項在 plan 中逐項有去處（併入 task 或 defer＋理由）。

**Gate G2**：✅

## P3 執行

**派工紀錄**
- W1（m4-plan-writer，implementer，model: opus）派出：撰寫 `docs/superpowers/plans/2026-07-05-yuzora-m4-terminal-preview.md`。brief 要點：唯一寫入白名單＝該新檔；必讀 design doc（:112-118/:238-255/:281-309/:361-377/:396+）＋M3 plan 結構範本＋現況碼（lib.rs/git_service/lsp_service/lsp_download/panel/stores/Settings/ipc）＋LEARNINGS；M4 範圍嚴格對 design doc、spike=wave 0 blocking（child webview 不確證 API 標 [SPIKE 驗證] 禁虛構）；M3 殘留 5 項逐項給去處（E5）；A1..An 附建議不拍板（至少含 xterm 套件集/pty crate/session 持久性/fallback 深度/UI 佈局歸屬/port 探測/殘留併入/checklist 模式）；回報含章節清單、Task 總表、A 全文、殘留對照、引用路徑自檢。→ 回報待收
- W1 中途（LEARNINGS 已知模式）：agent 完成 Rust infra 偵察即停（plan 檔未產出，ls 核實無 m4 檔）。偵察品質佳（lsp_service=長駐範本 HashMap+Arc+Channel/emit 雙通道＋backoff；git_service/lsp_download=deadline-poll-kill 逐字同型；**全倉庫 grep 零 process-group/JobObject——殘留 1 為全 codebase 共通缺口，M4 首建**；Cargo 無任何 pty crate；logging kind=String，audit 規劃未落地）。SendMessage 續推續寫 plan，非重派。
- W1 中途 2：收我續推後改派前端 plumbing 偵察再停（plan 檔仍未產出，ls 核實）。偵察發現（織入 plan 用）：**TerminalDrawer.tsx＋PreviewPanel.tsx 骨架已存在**（AppShell:58-59 terminalOpen/previewOpen local state、WorkspaceRail toggle 已接）；ipc Channel wrapper 僅 2 例同型（searchWorkspace/lspStart）；types 判別鍵慣例 status（process 態）/kind（內容分級）/type（stream 事件）；i18n 漸進式（strings.ts 與 inline 中文並存，StatusBar/SettingsDialog 全 inline）；store reset 兩種前例無統一慣例；實測 vitest 59 檔 534 綠。第二次 SendMessage：材料已齊、禁再派偵察、立即動筆。
- W1 完成回報（第三停）：plan 產出 `docs/superpowers/plans/2026-07-05-yuzora-m4-terminal-preview.md` 669 行；12 tasks/7 waves；A1..A12；殘留 5+1 對照；自檢 38 引用路徑 0 MISS；[SPIKE 驗證] 18 處 5 類／[假設] 5 處 4 類。
- **Controller 親驗 ✅**：wc -l 669 吻合；NUL 掃描乾淨（perl 零輸出）；章節 grep 齊全（E1 結構：Global Constraints/File Structure/任務相依/T1-T12/自動 vs 手動/風險 14 條/A1-A12/殘留對照/Coverage/Verification）；全文通讀完畢；五項關鍵宣稱抽驗全吻合——①xterm 依賴 package.json:40-41 `@xterm/addon-fit@^0.11.0`+`@xterm/xterm@^6.0.0` ✅②EditorPane formatting `tabSize: 4, insertSpaces: true` 確仍寫死 ✅③styles.css `--term-*` 26 處 ✅④useUserActionLog 確為 no-op stub（註解自陳 later phase）✅⑤TerminalDrawer 270 行（plan 稱 271，差 1 無害）/PreviewPanel 71 行吻合 ✅。→ 進 P4。
- W1 增補（R1 派出後、與完成回報交錯的續推所致）：補 A13（i18n 新字串歸屬——建議進 strings＋placeholder 中文化；替代 inline／全面遷移不採），標題與 Coverage 同步 A1..A13。Controller 親驗：670 行、NUL 乾淨、A13 內容於 :641／Coverage :660 ✅。註：Global Constraints :33 已按 A13 建議選項行文（M3「建議即預設、裁決後回填」慣例）；R1 從磁碟自行取證會讀到 A13 版，rubric 面 8 涵蓋。

**變更檔案**
- （待填）

**計畫修正**
- 無

## P4 對抗 review

**Reviewer 派工**
- R1（m4-plan-r1，reviewer type，model: opus，fresh／未參與撰寫）派出：只給客觀輸入（R1-R4、plan 路徑、design doc、M3 plan、殘留清單），不給撰寫者自辯。rubric 9 面：範圍完備（對 design doc 漏/超）、技術事實核實（親讀 ≥8 檔對行號宣稱）、[SPIKE 驗證]/[假設] 標記完備（掃未標記虛構 API：child webview/portable-pty/xterm v6/onCloseRequested）、UI 觸發鏈可達、相依圖與共用檔交集逐 wave 查、殘留去處理由對碼（#3 .catch 宣稱）、驗證方式可行性反證（pty spawn flakiness/xterm jsdom/port 衝突）、A 完備性（漏列裁量點）、≥3 反證紀錄。以打回票為目標。→ 回報待收（idle 無內容→SendMessage 催收，LEARNINGS 慣例）

**Round 1（R1 回報）**：1 major＋6 minor＋2 nit；反證通過 13 條（≥3 達標，含 lib.rs 14 mod／TerminalDrawer 逐行號／PreviewPanel／AppShell／LspBridge／SettingsDialog+openSettings 鏈／xterm+portable-pty 版本核／殘留#1#3#4 對碼／probe ephemeral／wave 無循環／onCloseRequested 實存 window.d.ts:1255）；rubric 8 面全覆蓋。

**Findings 裁決（controller）**——全部 9 條裁決為**真 finding**，回 P3 派修 W1：
- F1〔major〕app-close 清理無 owner、macOS fallback 不成立（piped stdio 無 SIGHUP／unix 父死不殺子孫／KILL_ON_JOB_CLOSE Windows-only／lib.rs:93 `.run(generate_context!())` 無 RunEvent callback、無 task 認領）→ 確認修：Rust app-exit cleanup 落 T11 交付（lib.rs run() callback 化＋iterate managers kill_tree）＋T3/T4 Drop 兜底；前端 onCloseRequested 降為輔。
- F2〔minor〕lib.rs command 數 30→38 — **controller 親驗 ✅**（sed+grep 39 行含 generate_handler 本身，實 38）→ 修數字。
- F3〔minor〕i18n 字串 wave 倒置（T10 wave5 供 T8/T9 wave4 消費）→ 修：lead 於 wave4 前預建 strings.terminal/preview 骨架（同 lib.rs stub 慣例）或 T8/T9 inline→T10 收斂，擇一載明。
- F4〔minor〕--term-* 無 ANSI 16 色 — **controller 親驗 ✅**（sort -u 僅 13 token：bg/bar/fg/fg2/line/chip/hover/amber/blue/coral/green/lime/ok）→ 修：ANSI 16 色硬編（雙主題常數）、token 只映射 bg/fg 等，:447 措辭同步。
- F5〔minor〕pty 輸出無節流/backpressure（yes/cat 大檔 flood 塞爆 Channel）→ 修：T3 reader coalesce 實作要點＋風險表新列。
- F6〔minor〕panels.test「No dev server」getByText 與「必續綠」衝突 → 修：措辭改「suppress 斷言保留、selector 隨 badge 更新」。
- F7〔minor〕kill(pgid,0)==ESRCH 殭屍 reap race → 修：測試改 poll-with-timeout。
- F8〔nit〕ResponsiveFrame 檔位標「A」未入清單 → 修：入 A14。
- F9〔nit〕preview URL 任意網址安全面未定 → 修：直接明定 MVP 鎖 localhost（安全預設不裁量、任意 URL 列 non-goal），不新增 A。

**R1 九修回報＋controller 親驗 ✅**：678 行、NUL 乾淨；抽驗——F1 RunEvent 主路徑 14 處（T11 交付 lib.rs callback 化＋T3/T4 Drop 兜底＋前端降 best-effort＋風險表新列）；F2 :46「38 個 command」（並自糾 TerminalDrawer 270 行）；F3 相依表/T10/A13 三處二擇一載明；F4 :436 13-token＋ANSI 硬編；F5 coalesce＋風險新列；F6「必續綠」grep=0 已刪、selector 更新語意；F7 :172 poll-with-timeout ESRCH；F8 A14 :649；F9 :508 localhost 政策＋:632 風險同步。A1..A14。→ 派全新 R2 整輪重跑。

**Round 2（m4-plan-r2，全新 opus reviewer，fresh）派出**：整輪完整重跑（rubric 同 R1 九面）＋額外驗證 R1 九修落地正確性（重點：RunEvent 三層設計在 Tauri v2 現實成立性——RunEvent::Exit/ExitRequested 語意、exit 時 managed state 可取得性）。→ 回報待收（idle 催收一次）

**Round 2 回報**：2 major＋3 minor＋1 nit；反證通過 8 條（RunEvent 三層設計 Tauri v2 成立——app.rs:220-232/:1366 佐證；portable-pty 確無；xterm6 API 全實存；serde↔TS 判別鍵一致；baseline 吻合；殘留對碼；sh 子孫樹有效；前端行號 20+ 全中）。R1 盲區（wave-4 相依）被 R2 抓到——全新 reviewer 制度生效。

**Findings 裁決（controller）**——全部 6 條裁決為**真 finding**，回 P3 派修 W1：
- F1〔major〕wave 4 T8/T9 同檔共改 `editorPanel.test.tsx` — **controller 親驗 ✅**（全檔 render AppShell；:31-45 `toggles the preview dock` 斷言 `Start or connect to a dev server`——T9 填實 PreviewPanel 必波及，plan 卻只配給 T8）→ 修：wave 4 序列化 T8→T9＋T9 契約明定 mount 不自動發 IPC（detect on-demand，與 A7 一致）。
- F2〔major，條件 A13=strings〕wave 4 T8/T9 同檔共寫 `i18n.ts`（「供 T8/T9 填鍵」＝兩 subagent 並寫，與 lib.rs 同型碰撞卻未序列化）→ 修：序列化接力載明（隨 F1 之 wave 4 序列化自然解，相依表明寫）。
- F3〔minor〕`.build(generate_context!())?` 於回 `()` 的 run() 不能編譯 — **controller 親驗 ✅**（lib.rs:17 `pub fn run()`；現況風格 `.expect`）→ 修：snippet 改 `.expect`。
- F4〔minor/latent〕app-exit 清理未界定：grace×N 退出阻塞上限／Exit+ExitRequested 雙觸發重工／watcher-vs-callback double-wait 鎖序 → 修：T11 契約補單一觸發點＋冪等、grace 預算、stopped-flag 擁有權約定（仿 lsp_service stop 優先權）。
- F5〔minor〕process_kill mod 預註冊時點「wave 1 前」vs T2 於 wave 0 需已註冊（TDD 不然不編譯）——plan 自相矛盾 → 修：改「wave 0 派發前」（:101/:151 同步）。
- F6〔nit〕:603 自動已驗清單混入 M3 遺留 fileGrade → 刪。

**R2 六修回報＋controller 親驗 ✅**：680 行、NUL 乾淨；抽驗——F1 :104 wave 4 序列化 T8→T9（點名 editorPanel.test.tsx:31-45 波及）＋:507 mount 不自動 detect＋test 檔劃 T9；F2 :104/:536/:650 三處接力語意；F3 `.build(...)?` 殘留 grep=0、`.expect` 2 處；F4 :54/:553/:559 單一觸發點＋grace 預算＋stopped 擁有權三點齊；F5 :101 wave 0 派發前＋舊句 grep=0；F6 fileGrade grep=0。
- Controller 復驗追加發現：風險表 :624 殘留「Exit/ExitRequested」並列與 F4 單一觸發點矛盾 → 快修派 W1（C1）。
- C1 回報＋controller 親驗 ✅：`Exit/ExitRequested` 並列 grep=0、`RunEvent::ExitRequested` 4 處（:54/:553/:559/:624 一致）、680 行。→ 派全新 R3 整輪重跑。

**Round 3（m4-plan-r3，全新 opus reviewer）回報**：非 dry pass——1 major＋3 minor＋1 nit；反證通過 7 條（wave3 T6/T7 零交集、wave5 T10/T11 零交集、tag="type" 有 search_service.rs:24 先例、殘留 #1 兩 named 點實存、editorPanel.test 歸屬正確、SettingsDialog 行號全中、--term-* 無 ANSI 硬編有據）；事實層近乎全中，findings 集中設計完備性。

**Findings 裁決（controller）**——5 條全裁決為真，回 P3 派修 W1：
- F1〔major〕T11「唯一保證」清理路徑靠未宣告 API：T3/T4 介面無 public kill_all、lsp 範本 servers private＋Drop 不可呼叫、pty/process_service 不在 T11 Files——ownership 死結 → 修：T3/T4 介面各明列 `pub fn kill_all(&self)`＋該 task 單元測；T11 只呼叫。
- F2〔minor/latent〕pty 雙關閉 × 無 session→Err ⇒ workspace 切換/自然退出常見路徑 unhandled rejection（bridge closeWorkspace→reset unmount→每 session 再 ptyClose→Err reject）→ 修：pty_close/pty_close_workspace 冪等（missing→Ok）；pty_write/pty_resize 維持 Err（真錯誤）。
- F3〔minor〕App.tsx bridge 預註冊未言明建 stub（lib.rs 有）→ import 不存在模組編譯斷 → 修：lead 於 T8 派發前建兩 stub bridge（return null）＋mount 行，比照 lib.rs 慣例明寫。
- F4〔minor/robustness〕**推翻 R2-F4① 字面的對立主張，controller 仲裁接受 R3**：R2 要單一觸發點的前提是「雙跑有害」，但修訂已確立 kill_tree 冪等（雙跑無害），前提消失；R3「同時 match ExitRequested 與 Exit」防某路徑僅發其一漏清，零成本更安全 → 修：match 兩者＋註明冪等；R2-F4 ②預算③擁有權不動。裁決理由記此。
- F5〔nit〕「capturing Channel harness 仿 lsp_service」措辭不符範本（lsp 實為 closure OnMessage，非 Channel）→ 修：T3/T4 介面註明 manager 層收 closure（可測 seam）、command 層包 Channel→closure；測試措辭改 capturing closure。

**R3 五修回報＋controller 親驗 ✅**：687 行、NUL 乾淨；抽驗——F1 kill_all 11 處（T3/T4 介面＋測項＋T11 只呼叫）；F2 close 冪等 5 處（write/resize 維持 Err）；F3 stub bridge 2 處；F4 雙 match 3 處＋「單一觸發點」grep=0（②③保留）；F5 capturing Channel grep=0。→ 派全新 R4 整輪重跑（零 finding 即 dry pass）。

**Round 4（m4-plan-r4，全新 opus reviewer）派出**：整輪 rubric＋驗三輪修訂自洽性（wave 0/1/4 預註冊/序列化/stub 語句互證）＋kill_all/冪等 close 可測性。→ 回報待收（idle 催收一次）

**Round 4 回報**：**零 major**＋5 minor/nit（非 dry pass）；反證通過 10 條（.build().run(callback) 正典＋lib.rs:17/93-94 核、共用檔逐 wave 無競寫、基線 169/534 實測吻合、editorPanel.test/retrofit 點/tabSize/--term-13-token/LspBridge/capabilities/close 冪等全 HOLDS）；抽驗 30+ 宣稱全中。收斂軌跡 9→6→5→5、major 1→2→1→0。

**Findings 裁決（controller）**——5 條全裁決為真，回 P3 派修 W1：
- F1〔minor〕:3/:687「A1–A12」樣板與 :643/:677「A1..A14」自相矛盾（使用者依 :687 只裁 12 條則 A13/A14 懸置，A13 直接決定 wave-4 i18n 策略）→ 修：兩處同步 A1–A14。
- F2〔minor〕:223 `configure_new_group→portable-pty spawn_command` 型別牆（helper 收 &mut std::process::Command；portable-pty 用自有 CommandBuilder 無 pre_exec）→ 修：pty 路徑改走 kill_tree_pid（pgid）＋pty session 語意，configure_new_group 限 std Command spawn 點（process_service/retrofit 點）。
- F3〔minor/latent〕Output/Exit 雙執行緒送同 Channel 無序（exit-watcher 先送 Exit、coalesce buffer 未 flush → 前端先 [exited] 再冒末行）→ 修：Exit 由 reader thread 於 EOF flush 後送（單源排序）；exit-watcher 只 wait 收屍清 map 不送事件。
- F4〔nit〕:59 File Structure 型別名與 T5 實際不一致（PreviewNavState 全文未定義）→ 修：:59 同步 T5 實名。
- F5〔minor/observation〕spike item 5（z-order）需真 app overlay stack，fixtures-only 驗不到 → 修：T1 明定 item 5 於臨時分支改真 app 驗。

**R4 五修回報＋controller 親驗 ✅**：689 行、NUL 乾淨；抽驗——F1 `A1[–—-]A12` 三型 dash grep=0；F2 kill_tree_pid 4 處＋configure_new_group 適用範圍限縮；F3 :224 單源排序＋:246 事件排序測項；F4 舊型別名 grep=0；F5 :129 真 app overlay stack 環境句。[假設] 5→6（portable-pty spawn 語意誠實增標）。→ 派全新 R5 整輪重跑。

**Round 5（m4-plan-r5，全新 opus reviewer）派出**：整輪 rubric＋四輪修訂交叉自洽（kill_all/冪等/排序/kill_tree_pid 新約束互證）。→ 回報待收（idle 催收一次）

**Round 5 回報**：零 major＋2 minor（非 dry pass）；反證通過 6 組（RunEvent API 合法性、20+ 檔 line-ref 全中、no-new-deps 宣稱、--term 13 token、wave4 序列化事實、double-close 冪等化解）；零 scope creep。收斂 9→6→5→5→2。

**Findings 裁決（controller）**——2 條全裁決為真，回 P3 派修 W1：
- F1〔minor/latent〕byte-count coalesce＋Output{data:String} 在 flush 邊界切開合法多位元組 UTF-8（echo 中文/emoji 於 N-byte 門檻或 M-ms timer 落在 3-byte 碼位中間 → lossy 兩端各產 U+FFFD）→ 修：reader flush 採 UTF-8 邊界感知（只送到最後完整碼位、殘尾留待下 chunk、EOF lossy 收尾），[假設] 涵蓋面同步。
- F2〔minor/latent〕Exit{code} 的 code 無源——R3-F4（reader 送 Exit 保排序）×（watcher 獨佔 wait）兩修訂的副產物：reader 拿不到 wait() 的 code，前端永遠 [exited] 無 code → 修：執行緒模型改單源——reader EOF 後親自 child.wait() 取 code（EOF 後 wait 短暫不 block）→ flush → 送 Exit{code} → 清 map＋log；獨立 exit-watcher 廢除（kill/close 路徑 kill_tree 後 reader 同樣經 EOF 收尾，stopped flag 語意保留）。排序保證與 code 有源同時成立、無 double-wait。

**R5 兩修回報＋controller 親驗 ✅**：690 行、NUL 乾淨；抽驗——F1 邊界感知 3 處＋:247 測項；F2 :225 單源 reader（廢除 watcher＋wait 取 code＋排序保證三合一）＋:246 `exit 3→Exit{code:3}` 測項＋T3 段 watcher 僅剩歷史對照句（:294 為 T4 dev server 合法保留）。[假設] 6→7（誠實增標）。W1 問「收斂可否逕判 dry pass」——**駁回**：dry pass 定義＝完整一輪 0 新 finding，須 R6 實跑證明。→ 派全新 R6。

**Round 6（m4-plan-r6，全新 opus reviewer）派出**：整輪 rubric＋五輪修訂交叉自洽（R5 單源 reader vs T11 stopped 擁有權 vs T4 exit watcher 一致性）。→ 回報待收（idle 催收一次）

**Round 6 回報**：零 major＋6 minor/nit；反證通過 7 組（序列化分工核實、RunEvent 改法合法、kill_all 冪等鏡射 lsp、retrofit 不破既有 timeout 測試、portable-pty 非依賴、Channel 契約忠實、殘留 #4/#5 對碼）。R6 自稱「dry pass 但 6 條」——**controller 駁正：有未推翻 finding 即非 dry pass**（協定定義），照裁照修。

**Findings 裁決（controller）**——6 條全裁決為真，回 P3 派修 W1：
- F1〔minor〕unix killpg 需 libc/nix 直接依賴，依賴帳（:40-42）/T2 Files/:101「Cargo.toml 僅 T3 動」全漏列 → 修：依賴帳＋T2 補 cargo add libc（或 nix 擇一記 report）；:101 改「T2（+libc）、T3（+portable-pty）不同 wave 無衝突」。
- F2〔minor〕process 端 exit-watcher 與 kill_tree(&mut Child) 擁有權未 reconcile（pty 端已單源化、process 端留白；blocking wait move Child 的字面直覺實作會死鎖設計）→ 修：T4 明述 Child 存 Mutex<Option<Child>>＋watcher try_wait 輪詢（仿 lsp_service :508-517/:589-607），kill 路徑從 Mutex 取 &mut。
- F3〔minor〕dev_server_stop 無 UI caller/測試（A4 說控制在 PreviewPanel header 但 T9 契約只有 start 側）→ 修：T9 契約補 running 時 Stop 按鈕→devServerStop＋測項。
- F4〔nit〕removeSession 關 active pane 時 activePaneId 改派未定義（dangling→高亮/focus 斷）→ 修：T6 契約補改派規則（剩餘首位或 null）＋測項。
- F5〔nit〕pty_list 無 UI 消費者恐 orphan → 裁決：**刪 pty_list command**（Simplicity First；manager 保留 pub sessions_for 供測試/未來，ipc wrapper 同步刪、wrapper 數同步）。
- F6〔nit〕「每 M ms flush」與 blocking read() 矛盾（阻塞中無法自主計時）→ 修：flush 時機改「每次 read() 返回即 flush（事件率受 read 節律自然限制、8KB buffer 天然聚合）＋UTF-8 殘尾跨 read 保留；實測仍過高再加批次計時記 report」。

**R6 六修回報＋controller 親驗 ✅**：694 行、NUL 乾淨；抽驗——F1 libc 4 處（依賴帳/T2 Files/:101 分帳＋附帶校正 :151 wave0 殘留矛盾）；F2 Mutex<Option<Child>>＋兩種擁有權模型並列；F3 :520 Stop 控制＋測項；F4 改派 2 處；F5 pty_list/ptyList 殘留 grep=0＋計數同步（5 command/9 wrapper）；F6 read-返回即-flush 3 處。[SPIKE 驗證] 18→19（誠實增標）。W1 再提「收斂逕判 dry pass」——再次駁回（協定）。→ 派全新 R7。

**Round 7（m4-plan-r7，全新 opus reviewer）派出**：整輪 rubric＋六輪修訂交叉自洽（pty 單源 vs process Mutex 兩模型、計數、序列化）；brief 新增防湊數條款（已標 [SPIKE]/[假設]/記-report 的待驗項與可辯護設計選擇不得報 finding——finding 須為執行時翻車或誤導實作者的實質缺陷）。→ 回報待收

**Round 7 回報**：零 major＋4 minor＋1 nit；反證通過 10 組（RunEvent 路徑、兩擁有權模型無死鎖、serde 逐型一致、共用檔無並改、T8/T9 互不打爛、無 process-tree kill 屬實、lsp 範本非虛構、xterm 前提、R1 scope、async 判別精準）；R7 另派 5 平行 scout 複驗，其中 2 項 R7 主報告未吸收、由 controller 收割為 C2/C3。

**Findings 裁決（controller）**——R7 5 條＋controller 收割 2 條，全裁決為真，回 P3 派修 W1：
- R7-F1〔minor〕wrapper 計數 9 vs 10 自相矛盾＋devServerStatus 全 plan 無消費者 → 裁決：**比照 pty_list 刪除 devServerStatus/dev_server_status**（ProcessBridge 常駐 App.tsx、listen 不漏事件；workspace 切換即 stop；「錯過狀態」場景不存在；manager 留 non-command status helper 供測試）；計數統一 9 wrapper／dev_server 4 command。
- R7-F2〔minor〕stdout parse 不到 port 時 preview 無導航目標 → 修：port 決定序「stdout parse > start 傳入 port > detect likelyPort」，皆無則顯示可操作等待/手動設定態。
- R7-F3〔minor〕kill_tree 未載明 reap 直屬子——zombie 下 kill(pid,0) 永不 ESRCH，T2 測試照字面自造 timeout → 修：kill_tree 契約補 kill 後 wait() 直屬子；測試斷言分層（直屬子 wait 收屍、孫 poll ESRCH）。
- R7-F4〔minor〕terminal 端「mount/visible 不自動發 IPC」不變量未明載（PreviewPanel 有、drawer 無）→ 修：T8 明寫 session 僅由 New/Split 使用者動作建立。
- R7-F5〔nit〕:236 lsp_service 行號漂移（capturing closure 實在 :1250、noop harness :989）→ 修正。
- C2〔minor，scout 揭露〕殘留 #3 理由句語意錯——「rejection 已由 IIFE .catch 吞（:216）」不實：該 catch 屬 LSP-extensions mount promise，getDocument 外鏈**無** catch（unhandled rejection 仍在；與 wave4 F1 原始紀錄一致）→ 修：理由句改誠實（defer 結論不變）；StatusBar :85-95 半句正確保留。
- C3〔nit，scout 揭露〕spec 引用三處漂移：:18「:396-408」超 EOF（spec 僅 407 行）→ :396-407；:540 描述掛位（command/port bullet 在 :291 非 :283）；:133「open externally 符 spec :307」不實（:307 僅「可操作錯誤」；open externally 為 A6 設計添加，出處拆分標明）→ 修。
- R7 附帶（併入低成本修）：:568 onCloseRequested 沿用既有 isTauri() guard（AppShell:123）註明；pty lifecycle 測試比照 poll-with-timeout。

**R7 七修回報＋controller 親驗 ✅**：697 行、NUL 乾淨；抽驗——F1 devServerStatus live 殘留 0（:319 四 command 明列＋:378 九 wrapper 與介面一致）；F2 決定序 1 處；F3 reap 2 處＋分層斷言；F4 不自動 ptyOpen 1 處；C2 :676 誠實化（「風險仍在……不虛報」）；C3 :396-407（:396-408 歸零）。→ 派全新 R8。

**Round 8（m4-plan-r8，全新 opus reviewer）派出**：整輪 rubric＋七輪修訂交叉自洽＋計數一致性；brief 防湊數條款再加嚴（nit 級行號 ±2 漂移不構成 finding）。→ 回報待收

**Round 8（m4-plan-r8，全新 opus reviewer）回報：零 finding，dry pass ✅**——親讀 13+ 檔、10 組反證全數打回票未果（lsp 範本行號全準、殘留 #3/#4 誠實化屬實、retrofit 點分類正確、wave4 分段無碰撞、RunEvent 路徑成立、依賴宣稱屬實、13 token 精確、UI 接點全中、i18n 序列化自洽、StrictMode [假設] 已標）；rubric 8 面全 ✓。**Gate G4 ✅**（累計 8 輪、40 findings 全裁決、反證 50+ 組；major 軌跡 1→2→1→0→0→0→0→0，findings 軌跡 9→6→5→5→2→6→7→0）。

## P5 驗收

- E1（章節齊全）：grep 章節結構 → 9 個 ## 章節（Global Constraints/File Structure/任務相依/自動 vs 手動/風險/A1..A14/殘留對照/Coverage/Verification）＋12 個 ### Task ✅ PASS
- E2（引用零斷鏈）：核心 12 路徑存在性檢查全 OK（spec/M3 plan/checklist/lib.rs/lsp_service/git_service/lsp_download/TerminalDrawer/PreviewPanel/LspBridge/ipc.ts/EditorPane）；W1 自檢 38 路徑 0 MISS＋八輪 reviewer 交叉核實（R8 親讀 13 檔全中）✅ PASS
- E3（dry pass＋反證＋裁決）：R8 零 finding dry pass；反證紀錄 50+ 組（遠超 ≥3）；40 條 findings 每條有裁決紀錄（見 P4 段）✅ PASS
- E4（變更面）：本 run 產出＝M4 plan（docs/superpowers/ 在 .gitignore 故不現於 git status，與 m3-closeout 紀錄一致）＋run file（??）＋LEDGER（M）——皆在預計清單；src/src-tauri 零觸碰 ✅ PASS。git status 中其餘變更（settings.json M／yuzora-loop skill 三檔 D／CLAUDE.md M／loop-simplify run file ??）屬**平行 loop-simplify run**（使用者裁決移除閉環制度），非本 run 產出，已核實其 run file 記帳吻合。
- E5（殘留去處）：plan「M3 殘留 5 項去處對照」表完整——#1 併入 T2（retrofit 兩 named 點八輪核實）、#2-#5 defer 附理由（#3 理由經 C2-R7 誠實化）、#6 checklist 為使用者配合項 ✅ PASS
- NUL 終掃：乾淨 ✅
- 與 baseline 比對：無新紅燈（本 run 未動任何程式碼；文件型 battery 全過）✅

**Gate G5 ✅**

## P6 收尾

- 結果：**DONE**
- 最終變更檔案：
  - `docs/superpowers/plans/2026-07-05-yuzora-m4-terminal-preview.md`（新增，697 行；opus W1 撰寫、八輪對抗 review 40 findings 全修訂）
  - `.claude/loop/runs/2026-07-05-plan-next-phase.md`（本 run file）
  - `.claude/loop/LEDGER.md`（RUNNING→DONE 行）
- 需使用者確認的假設：A1（下一 phase=M4）roadmap 佐證成立；A2（交付=plan 定稿＋A 待裁決清單，沿 M3 kickoff→adjudication 模式）；A3（不用 worktree）——三者執行中均未被推翻。
- 殘留事項：①**A1..A14 待使用者裁決**（plan :634-662；裁決後回填 plan 再開實作 run）；②m3-manual-checklist 實機驗收（M3 交付待辦，使用者配合 gui-acceptance）；③T1 spike（wave 0 blocking）為 M4 實作第一步。
- 制度備註：收尾期間平行 loop-simplify run 依使用者裁決**移除整個 yuzora-loop 閉環制度**（SKILL/template/LEARNINGS 刪除、hook 清空、CLAUDE.md 條款移除；.claude/loop/ 降為歷史檔案）。本 run 於制度有效期間完成全部閘門（G1-G5），記帳依歷史準確性補完；LEARNINGS 追加與「建議放寬」清單因制度廢止不再執行。本 run 的實證觀察（八輪 review major 於 R4 後歸零、後四輪 findings 以表面一致性為主——邊際效益遞減）與使用者簡化決策方向一致。
- LEDGER 已更新：✅
