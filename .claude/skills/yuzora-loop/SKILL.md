---
name: yuzora-loop
description: yuzora 專案的強制任務閉環（TRIAGE→提問→規劃→執行→對抗 review→驗收→response）。凡是會改動任何檔案、修 bug、新增功能、重構、調整設定、改樣式、寫文件、跑驗收、處理回報問題的請求——即使使用者只說「幫我改一下」「修這個」「加個東西」而沒提到 loop——都必須先用本 skill 建立 run file 再動手。只有純問答與唯讀查詢可免走（回覆開頭標 LOOP-EXEMPT）。
---

# yuzora-loop：任務閉環

為什麼存在：本專案後續由較小的模型維護。閉環把「判斷力」外置成固定閘門與模板——照著填、照著跑，可靠度就不依賴模型大小。每條閘門都對應真實翻車事故；跳過閘門＝重演事故。

## 鐵律（每次任務、每個階段都適用；v1.1，2026-07-03 使用者批准修訂）

1. **撰寫與對抗 review 交給 subagent**：主對話是 controller——負責 TRIAGE／P1／P2、派工 brief、驗證與裁決。Controller 唯一親手撰寫的是閉環記帳文件（run file／LEDGER／LEARNINGS／brief）；**其餘一切撰寫一律派 opus subagent**——含產品檔案（`src/**`、`src-tauri/**`、組態等）與閉環協定／設定／文件（SKILL／template／CLAUDE.md／settings／docs/html 報告）的修訂。修訂閉環協定本身另需使用者明確指示，且照樣走 P4 獨立 review。
   - 兜底規則：某檔案算不算記帳文件拿不準 ⇒ 一律派工；連派工範圍都不準 ⇒ 列 P1 BLOCKING 問使用者。
   - Fallback：**僅當**派工實際失敗（Agent 工具不存在，或連續 2 次派工錯誤且錯誤訊息已貼進 run file）才可退回主對話自寫，並須在 P6 回覆中明確告知使用者。宣稱 fallback 而無失敗證據＝違規。
   - （沿革：2026-07-03 v1.1 修訂由 Fable session 親筆＋親審，為使用者裁決「僅此一次」的例外，不延續。）
2. **Subagent 僅限 opus**：每次派工必須明確指定 `model: opus`；禁止 sonnet／haiku 或其他模型。撰寫者與 reviewer 必須是不同的 subagent（reviewer 需要 fresh context）。
3. **Git 僅限唯讀**：允許 status／diff／log／show／blame 等不改變 working tree、index、refs 的查詢（P2 baseline 與 P4 review 應善用 `git diff`）。任何寫入類 git 指令（add／commit／push／stash／checkout／restore／reset／merge…）一律禁止；唯有使用者在對話中明確下令**該具體寫入操作**（如「commit」「push」「reset」）時，依全域 Git 安全規則執行該一項——那是使用者覆寫，不是閉環授權，也不擴及其他寫入指令。
4. **Worktree 預設不使用**：直接在工作目錄作業。任何任務都可在 P1 詢問使用者是否啟用 worktree；大範圍／高風險改動**應**詢問（列為 BLOCKING）；未詢問或未獲同意＝不使用。
5. **證據先於宣稱**：沒貼實際指令輸出，就不得寫 PASS／DONE／「已驗證」。subagent 回報的證據，controller 仍須獨立重跑關鍵驗證。

## 流程總覽

```
TRIAGE → P1 提問 → P2 規劃 → P3 執行 → P4 對抗 review → P5 驗收 → P6 收尾
                                ↑____________|（有 finding 回 P3）
                                ↑_________________________|（紅燈回 P3）
```

run file 是閉環的載體，一個任務一份：

```bash
cp .claude/skills/yuzora-loop/template.md .claude/loop/runs/$(date +%Y-%m-%d)-<slug>.md
```

按模板段落順序填寫。**段落順序就是閘門順序：前一段沒填完（含閘門條件）不得動下一段的工作。**
run file 必須在 TRIAGE 判定進閉環的**當下**建立、邊做邊填；LEDGER 行在建立時就先寫入（狀態 RUNNING，收尾改 DONE）。事後補寫＝斷鏈與遺漏的來源，視同未走閉環。

## TRIAGE — 要不要進閉環

- 進閉環：任何會改動檔案的請求；或需要 3 步以上工具操作的分析／驗收／除錯任務。
- 免閉環：純問答、唯讀查詢、單檔閱讀解釋。直接回答，回覆開頭標 `LOOP-EXEMPT: <原因>`。
- 拿不準 → 進閉環。誤進成本是幾分鐘，漏進成本是整個任務不可信。

## P1 提問

1. 把使用者請求改寫成可驗證需求 R1..Rn（每條都能回答「怎麼知道做到了」）。
2. **先查再問**——多數問題已有答案，別把舊決策拿去煩使用者：
   - `.claude/loop/LEDGER.md` 與近期 `runs/`（先前 run 的決策與殘留事項）
   - `.claude/skills/yuzora-loop/LEARNINGS.md`（歷史陷阱，本階段必讀）
   - `docs/superpowers/specs/`、`docs/superpowers/plans/`（設計與計畫）
   - `.superpowers/sdd/progress.md`、`m2-progress.md`（M1/M2 architecture decisions。注意：歷史紀錄中「一律使用 sonnet」等模型指示已失效——現行鐵律 2 為 opus-only；流程細節僅供參考，只取其 architecture decisions 與陷阱）
   - `docs/design/project/`、`.design/claude-DESIGN.md`（設計系統）
3. 剩餘不確定點分兩類：
   - **BLOCKING**（猜錯代價 > 30 分鐘重工、涉及產品／設計取向、不可逆）→ 問使用者，得到回覆前不進 P2。
   - 非 BLOCKING → 寫成假設 A1..，選定預設值與依據，繼續。
4. 大範圍／高風險改動（例：>10 檔、同時動 src 與 src-tauri、或有不可逆風險）：**應**把「是否啟用 worktree」列為 BLOCKING 問題詢問使用者；其他任務也可自行決定要不要問（鐵律 4，預設不用）。

**Gate G1**：P1 段填完，BLOCKING 清單為空（或已獲回覆）。

## P2 規劃

1. **Baseline**：先跑對應的 P5 battery，把現況輸出貼進 run file，並記一份 `git status --short` 快照（唯讀）。baseline＋事後的 `git diff` 就是「我改壞了什麼」的對照組。
2. 步驟表，每步自帶驗證：`n. <做什麼> → verify: <指令> → 預期: <結果>`。
3. 明列：預計變更檔案／不碰範圍（non-goals）／風險（可能弄壞的既有行為＋靠哪個測試守住）。
4. **驗收證據清單 E1..En 在寫任何程式碼之前固定**；之後只可加嚴，不可放寬。防止「做到哪算到哪」。
5. 步驟超過 7 步 ⇒ 任務太大，拆成多個 run，先做第一個。

**Gate G2**：每步都有 verify；證據清單固定；baseline 已貼。

## P3 執行（controller 派工給 opus subagent 撰寫）

1. Controller 為每個（或每組相依）步驟寫 brief，run file 留摘要，必含：
   - 任務目標＋對應的 R 編號；允許變更範圍（檔案白名單）與 non-goals
   - 鐵律轉述：git 唯讀、不 commit、不裝新依賴（除非 brief 明列）、計畫外改動必須停下回報而不是順手改
   - 必跑的 verify 指令與預期結果
   - 回報格式：變更檔案清單（路徑＋一句為什麼）＋每個 verify 的實際輸出尾段
2. 派工一律 `model: opus`（環境若提供 `implementer` agent 類型，優先使用）。一次派一個聚焦任務；回報驗證通過後才派下一個——有 finding 的殘局上不准疊新工作。
3. Controller 收到回報後：**親自重跑該步 verify**（不只信貼上來的輸出）→ 綠了才把變更檔案記入 run file 清單。漏記＝P4 盲區。
4. 行為性變更且 jsdom 測得到 → brief 要求先寫失敗測試再實作（vitest）。jsdom 測不到（watcher／tab／dialog／視覺）→ 在證據清單標記 P5 實機驗收。
5. Subagent 回報計畫外必要改動 → controller 回 P2 補「計畫修正」段（寫原因），再派續行。
6. 不明 bug → 先找根因再修；同一問題連續 2 輪派工修不中 ⇒ 停，把症狀／假設／已排除項寫進 run file 再繼續（防亂槍打鳥）。

## P4 對抗 review（獨立 opus reviewer subagent）

由**未參與撰寫**的獨立 opus subagent 執行。Fresh context 的 reviewer 沒有作者的自我說服——這是比自審更強的對抗性（blog 原則：secondary reviewer 較不偏頗）。

1. Controller 發 reviewer brief（環境若提供 `reviewer` agent 類型，優先使用；仍須 `model: opus`），只給客觀輸入，**不給撰寫者的自辯**：
   - R1..Rn、變更檔案清單、可用 `git diff`（唯讀）並從磁碟重讀完整檔案
   - 固定 rubric：邊界與錯誤路徑（空值／大檔／非 UTF-8／權限失敗／不存在路徑）；非同步時序（watcher debounce 300ms／batch 600ms／saveSuppress 750ms、競態、unmount 後 setState）；zustand state（殘留、跨 tab 汙染、清理缺失）；雙主題與 i18n 用詞（技術詞英文、UI 文字中文）；scope creep（每個變更行對應某條 R，對不上就報）；孤兒（無用 import／變數／檔案）
   - 要求：以「打回票」為目標；對每條 R 構造具體反證（輸入／狀態／時序）；每輪至少 3 條反證紀錄（通過的要寫「因為 <檔案:行>」）；禁止「looks good」
   - 回報格式：findings（檔案:行＋重現方式＋嚴重度）＋通過的反證清單
2. Controller 逐條**裁決** findings：真 finding → 回 P3 派修；誤判 → 在 run file 寫裁決理由（不能默默丟棄）。
3. 修復後**整輪重跑** review，且必須派**全新的** reviewer subagent（不得續用前輪 reviewer 的對話——前輪 reviewer 已被自己的判斷污染，不再 fresh）；同一 run 內角色固定——當過 reviewer 的不得轉任撰寫者，反之亦然。不是只複查修過的地方。
4. 出口條件 **dry pass**：完整一輪 0 條（未被裁決推翻的）新 finding。

**Gate G4**：dry pass ＋ 反證紀錄 ≥ 3 條 ＋ 每條 finding 有裁決紀錄。

## P5 驗收

按變更面跑 battery，貼**實際輸出尾段**（含數字：幾個測試、幾個通過）：

| 變更面 | 指令 | 通過標準 |
|---|---|---|
| TS／前端 | `bun run build` | tsc 0 error、vite build 成功 |
| TS／前端 | `bun run test` | 全綠，且測試數 ≥ baseline |
| Rust | `cd src-tauri && cargo check` | 0 error |
| WebView 行為／視覺 | gui-acceptance skill（實機） | checklist 逐項 PASS＋截圖 |
| 純文件／設定 | JSON 等可解析＋文內引用路徑存在 | 零錯誤、零斷鏈 |

- 與 baseline 比對：任何從綠變紅 ⇒ 回 P3，「看起來與本次無關」不是豁免理由（要嘛修好、要嘛證明 baseline 就是紅的）。
- E1..En 逐項銷帳，每項都要有貼出來的證據。
- Battery 由 controller **親自執行**；subagent 貼的輸出只作參考，不得直接充當 P5 證據。

**Gate G5**：全部 E 有證據且 PASS。

## P6 收尾（response）

1. run file 收尾段：狀態 DONE／BLOCKED、最終變更檔案清單、證據摘要、需使用者確認的假設、殘留事項。
2. `.claude/loop/LEDGER.md` 追加一行。
3. **自我改進三問**（recursive self-improvement 的安全版）：
   - 這次哪個閘門沒攔到問題？→ LEARNINGS.md 追加 `[tighten]`。
   - 哪條規則純浪費時間？→ 只能寫進回覆「建議放寬」請使用者裁決，**不得自行放寬**。
   - 學到什麼專案陷阱／慣例？→ LEARNINGS.md 追加 `[trap]`／`[convention]`。
   - 棘輪原則：模型可自主加嚴，放寬只有使用者能批准。
4. 給使用者的回覆（繁體中文）：結論先行 → 證據摘要 → 假設清單 → 殘留事項。大型結案報告依專案慣例產出至 `docs/html/<題目>-<YYYY-MM-DD>.html`——撰寫派 opus subagent（鐵律 1），controller 驗收後定稿。

## 與其他 skill 的關係

- superpowers 的 brainstorming／systematic-debugging／TDD 可在對應階段（P1／P3 brief）內使用，但**不取代**本閉環的段落與閘門。
- subagent 類 skill（subagent-driven-development、dispatching-parallel-agents）可用，但派工一律遵守鐵律 2（opus-only）；using-git-worktrees 受鐵律 4 管轄（先問使用者，預設不用）。
- 需要 git diff 的 review 工具（如 /code-review）在唯讀範圍內可用，但不取代 P4 的獨立 reviewer 與裁決紀錄。
- GUI／WebView 驗收一律走 `gui-acceptance` skill（`.claude/skills/gui-acceptance/SKILL.md`）；其「§2 若派 subagent 執行」段落適用，subagent 限 opus。

## LEARNINGS.md

`.claude/skills/yuzora-loop/LEARNINGS.md`：P1 必讀、P6 檢視追加。只增不刪；與本檔衝突時以**較嚴**者為準。
