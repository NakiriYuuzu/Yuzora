# LOOP RUN — m3-session-prompts

- 日期：2026-07-03
- 狀態：RUNNING
- 任務原文：
  > 在下一個 sessions 開始執行，再那之前先提供 prompts 用於下一個 sessions

## P1 提問

**需求（可驗證陳述）**
- R1: 產出 M3 實作 session 用的 kickoff prompts 檔（首發 wave 0、通用接續、收尾驗收三式），貼入新 session 即可依 yuzora-loop 閉環＋定稿 plan 接續執行。
- R2: prompts 內含：必讀清單、進度接續方法（LEDGER）、鐵律轉述（opus subagent／git 唯讀／worktree 不用／證據先於宣稱）、五件套與基線、wave 0 lib.rs 預註冊特別事項、「A1–A9＋A9' 已裁決不得重議」。
- R3: 檔內引用路徑全部存在（文件 battery）。

**已查資料**
- M3 plan 已定稿（2026-07-03，A1–A9＋A9' 回填完畢）；LEDGER 三筆 m3-* run 全 DONE。
- 命名前例：docs/superpowers/plans/*-agent-loop-prompts.md（M1/M2 有 prompts 檔前例）。
- kickoff run 曾列 non-goal「不寫 agent-loop-prompts」——現由使用者明確指示推翻（使用者指示優先）。

**假設（非 BLOCKING）**
- A1: prompts 檔歸 docs/superpowers/plans/（沿 M1/M2 前例），名 2026-07-03-yuzora-m3-session-prompts.md。
- A2: 分類為 brief 類記帳文件（給未來 controller 的派工指令）→ controller 親寫合規（鐵律 1 白名單含 brief）。
- A3: wave 0 的 lib.rs 預註冊（4 mod 行＋空 stub）依使用者核准之 plan 明文由 lead 親做＋run file 記錄；prompts 如此指示。
- A4: 下個 session 執行模式回歸鐵律預設（opus subagent-driven）——先前「不用 subagent」為該次對話一次性覆寫，prompts 不延續之；使用者屆時可再覆寫。

**BLOCKING 待問**：無（使用者指令明確：先 prompts、下 session 執行）。

**Gate G1**：✅

## P2 規劃

1. 親寫 prompts 檔（三式＋wave 注意事項表）→ verify: 檔案存在＋結構 grep。
2. 文件 battery：引用路徑存在 → 預期零斷鏈。
3. P6 收尾＋LEDGER＋回覆內文附 prompts 全文。

**預計變更檔案**：docs/superpowers/plans/2026-07-03-yuzora-m3-session-prompts.md（新增）、本 run file、LEDGER。
**non-goals**：不動 M3 plan、不動 src/**、git 唯讀、不開始實作。
**證據清單**：E1 檔案存在＋三式 prompt 齊；E2 路徑零斷鏈。

**Gate G2**：✅

## P3 執行

- 親寫 `docs/superpowers/plans/2026-07-03-yuzora-m3-session-prompts.md`：Prompt A（wave 0 首發，含 lib.rs 預註冊特別事項）／Prompt B（通用接續，含 wave 2/3/4/5 關鍵注意與共用檔清單）／Prompt C（收尾驗收，五件套終驗＋gui-acceptance＋closeout 報告）＋wave 速查表。

## P4 對抗 review

- Controller 自查：三式與 plan 相依表/裁決一致（9 handler/10 wrapper/T12→T11 序列/七 server 一鍵安裝/TabBar 掛載全部反映）；鐵律轉述完整（opus/git 唯讀/worktree/證據）；「A1–A9＋A9' 不得重議」在 A/B 兩式皆有。無矛盾。

## P5 驗收

- E1 三式齊：`grep -c "^## Prompt"`＝3（:8/:41/:68）。**PASS**
- E2 路徑：MISSING 僅 m3-manual-checklist.md 與 gen-lsp-fixtures.ts——皆為 T15 計畫新增檔，Prompt C 於其存在後才使用，屬正確前向引用；其餘零斷鏈。**PASS**
- 關鍵約束 grep：不得重議 2、唯讀 5、opus 4、五件套指令 6、預註冊 2、openSettings 2。**PASS**

## P6 收尾

- **狀態：DONE**。變更檔案：prompts 檔（新增）、本 run file、LEDGER。
- 假設 A4 需使用者知悉：prompts 預設下個 session 回歸鐵律（opus subagent-driven）——先前「不用 subagent」視為一次性覆寫；若要延續請於新 session 開頭自行加註。
- 殘留：無；下個 session 貼 Prompt A 即開工。
