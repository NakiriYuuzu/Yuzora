# LOOP RUN — m3-plan-adjudication

- 日期：2026-07-03
- 狀態：RUNNING
- 任務原文：
  > 請你重新梳理一遍這個 plans，然後一一詢問

## P1 提問

**需求（可驗證陳述）**
- R1: 向使用者重新梳理 M3 plan（waves／tasks／已鎖定裁決／測試策略），資訊與 plan 現行內容一致。
- R2: 假設待裁決 A1–A9 逐項詢問使用者，取得明確裁決（AskUserQuestion，每項獨立卡片）。
- R3: 裁決結果回填 plan「假設待裁決」節與對應 task（只可依裁決改，不得夾帶其他變更）。
- R4: 回填後純文件 battery（結構 grep＋裁決反映 grep）通過。

**已查資料**
- plan 現行版（dry pass 後）：docs/superpowers/plans/2026-07-03-yuzora-m3-lsp.md——本 session 剛審修完，內容在案。
- 前 run（m3-lsp-kickoff，DONE）：Q1 裁決＝plan 核准後才實作；A1–A9 為殘留事項，正是本 run 要收的。
- LEARNINGS 已讀（本 session 稍早），無新增相關條目。

**假設（非 BLOCKING）**
- A-run1: 使用者「不使用 subagent／git 寫入／worktree」三約束延續本 session 全程（依據：同 session 內明確指示未撤回）。回填由 controller 親寫。

**BLOCKING 待問**
- 即 A1–A9 本身——本 run 的主體就是把它們當 BLOCKING 問完。

**Gate G1**：✅（問題清單即 A1–A9，逐項問）

## P2 規劃

**Baseline**：plan 現行版＝kickoff run P5 驗收通過版（結構 15 tasks／E1–E4 PASS，證據見 runs/2026-07-03-m3-lsp-kickoff.md）。
**步驟**
1. 梳理呈現＋AskUserQuestion 分 3 批（3+3+3）問 A1–A9，末批附整體核准問題 → verify: 每項有明確選項或 Other 回覆。
2. 裁決回填 plan（假設待裁決節逐條標「裁決：…（2026-07-03）」＋連動 task 內容）→ verify: grep 每條裁決標記。
3. 純文件 battery：結構 grep（15 tasks／必要節）＋引用路徑不新增斷鏈 → 預期全綠。
4. P6：run file 收尾＋LEDGER。

**預計變更檔案**：plan（回填）、本 run file、LEDGER。
**non-goals**：不動 src/**、不改 plan 其他內容、git 唯讀。
**證據清單**：E1 裁決紀錄（本檔）；E2 回填 grep 輸出；E3 結構 battery 輸出。

**Gate G2**：✅

## P3 執行

**裁決紀錄（2026-07-03，AskUserQuestion 逐項）**
- A1 跨檔 Workspace：**M3 做完整自訂 Workspace**（使用者於詳解降級 vs 完整兩案後選完整）→ 擴 T6/T10。
- A2 RA download 依賴：**ureq＋背景執行緒**（使用者先問 async 加速，經解釋「速度由網路決定、兩案皆背景執行＋進度事件」後選 ureq）→ T14 明寫進度事件。
- A3 symbols UI：**cmdk 雙模式選單**（照建議）。
- A4 Markdown 渲染器：**markdown-it＋白名單 sanitize**（照建議）。
- A5 semantic tokens：**完整 LSP token 對照表**（22 type＋10 modifier，非建議之最小集）→ 擴 T8。
- A6 指令名：**照清單、官方為準**（照建議）。
- A7 format on save：**預設 OFF、放 LSP 區**（照建議）。
- A8 trace 落盤：使用者要求深比較（ultrathink）→ 量級分析後建議修正為獨立檔 → **獨立 trace 檔＋主 log 指標事件**。
- A9 發佈策略：使用者兩輪追問（bundle 影響／乾淨主機＋Windows）→ 最終「**2，但都提供一鍵下載**」＝**不 bundle＋四語言皆一鍵安裝**。Controller 詮釋（回填後供使用者最終過目確認）：rust-analyzer／marksman＝managed download（官方 release binary）；vtsls／Pyright（Node 系）＝偵測 npm 後一鍵代跑安裝至 `~/.yuzora/servers` 私有 prefix，無 npm 則 guided manual（提示先裝 Node）；一切失敗 fallback guided manual install → T14 擴為四語言 managed install。
- 核准流程：**回填後再給使用者看一次**（本 run 交付＝回填＋變更摘要呈現）。

**回填連動範圍（controller 親寫，使用者覆寫下）**
- A1 → T6（新增 src/lsp/workspace.ts 自訂 Workspace＋測試）、T10（跨檔行為改背景載入語意）、風險表、File Structure。
- A5 → T8（完整 token 對照＋editor.css 全 class＋雙主題驗證）。
- A8 → 新增 lsp_set_trace command（Rust tee 獨立檔）→ **handler 計數 8→9、T14 第 10 個**——T4/T5/T12/File Structure/wave 表全部連動更新（此計數正是 R1-F6 抓過的點，務必全域一致）。
- A9 → T14 改名 managed server install（四語言）、handler 改 lsp_install_server、T15 checklist 連動。
- A2/A3/A4/A6/A7 → 對應 task 標裁決＋小幅措辭定稿。

**A9' 補充裁決（2026-07-03，使用者過目時追問「是否選擇要下載哪一個」）**
- 答覆：一鍵安裝為 per 語言、裝當下選定 profile；指出 T14 原僅明列四預設、替代 profile 安裝方式各異（tls=npm、markdown-oxide=binary、pylsp=pip）。
- 裁決：**全部 7 個 curated adapter 皆可一鍵安裝**。回填：T14 三路徑（binary×3／npm×3／pip×1 venv）、LspInstallProgress 加 "pip" phase、T12 按鈕文案（無 npm／無 python 提示）、T4 which 加 pyenv bin、wave 表／File Structure／T15／待手動清單／裁決結果節全數連動；「四語言」殘留清除（僅 :804 裁決沿革保留）。
- 使用者同時回覆「確認沒問題」→ **plan 定稿**。

## P4 對抗 review

- 型態：裁決轉錄＋連動回填；controller 回填後全域一致性自查（計數 grep＋殘留 grep）＋使用者最終過目（本 run 交付即含此 gate——使用者裁決「回填後再給我看一次」）。
- 自查發現並修復：File Structure :65 MarkdownPreview 行殘留「見假設待裁決 A4」→ 改 A4 裁決措辭。其餘「明確降級」1 處為裁決沿革敘述（正當保留）。

## P5 驗收

- E1 裁決紀錄：本檔 P3 段（A1–A9 全數含使用者原話要點與詮釋）。**PASS**
- E2 回填 grep（實跑）：YuzoraWorkspace 6、workspace.ts 7、lsp_set_trace 5、lspSetTrace 4、lsp_install_server 6、lspInstallServer 4、lsp:install-progress 5、markdown-it 6、cmdk 4、預設 OFF 3、獨立 trace 檔 3、一鍵安裝 15——A1–A9 全數可 grep。**PASS**
- E3 結構 battery：15 tasks；必要 ## 節全在（Global Constraints :20／File Structure :36／任務相依 :95／自動已驗 :760／風險 :775／假設裁決結果 :792／Coverage :804／Verification :813）；handler/wrapper 計數三處一致（9 handler＋第 10 個 install＋10 wrapper）；「假設待裁決」殘留＝0；既有檔路徑引用零斷鏈。**PASS**

## P6 收尾

- **狀態：DONE**（梳理＋A1–A9＋A9' 全數裁決回填；**使用者已確認「沒問題」→ plan 定稿**）。
- A9' 終驗證據（實跑）：A9' 出現 8 處、markdown-oxide 7、pylsp 10；15 tasks 結構不變；「四語言」殘留清至僅 :804 沿革；既有檔路徑引用零斷鏈。
- **最終變更檔案**：plan（裁決回填，controller 親寫——使用者覆寫）、本 run file、LEDGER。
- **需使用者確認**：已全數確認（2026-07-03「確認沒問題」＋A9' 裁決）。
- **殘留事項**：實作 run 依 wave 0 起跑需使用者下令（現行鐵律：plan 核准後才實作）。
- **自我改進三問**：閘門皆有攔（自查抓到 1 處殘留）；無建議放寬；學到——AskUserQuestion 逐項裁決時使用者的追問（「更仔細解釋」「ultrathink 對比」）本身會推翻原建議（A8 即是），深查後修正建議並如實標註「原建議」是正確做法，不硬撐原案。
