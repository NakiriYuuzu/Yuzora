# LOOP RUN — yuzora-loop-bootstrap

- 日期：2026-07-03
- 狀態：DONE
- 任務原文：
  > 今天是我為數不多能夠接觸 Fable-5 的日子，再之後的維護只能使用 opus, sonnet, haiku 進行維護，所以需要一個強大的 loop 閉環，能夠每次任務都必須依照 loop 的腳本進行提問，規劃，執行，對抗review，驗收，response 的方式下執行；為此專案設計好一套專用 loop 閉環。限制：1. 不使用 subagent 2. 不使用 opus, sonnet, haiku 3. 不使用 git 4. 不使用 worktree

（註：本 run 是閉環系統自身的建立，故為「邊建立邊套用」的 dogfood run，也是後續 run 的示範樣本。）

## P1 提問

**需求（可驗證陳述）**
- R1: 存在一套本專案專用、每次任務強制執行的閉環，階段為 提問→規劃→執行→對抗 review→驗收→response（驗證：skill＋強制機制存在且路徑可達）
- R2: 閉環在無 subagent、無模型委派、無 git、無 worktree 的條件下完整可運作（驗證：協定全文無任何依賴這四者的步驟）
- R3: 較小模型（opus/sonnet/haiku）能照著執行（驗證：全部judgment 外置為模板、閘門、checklist、固定指令）
- R4: 融入兩篇參考文章的理念——loops 的 verification-as-skill／明確 success criteria／fresh reviewer，與 recursive self-improvement 的自我改進迴路＋安全棘輪

**已查資料**
- claude.com/blog/getting-started-with-loops：四種 loop 結構；驗證要編成 skill 且量化；成功準則要明確；secondary reviewer 較不偏頗
- anthropic.com/institute/recursive-self-improvement：瓶頸在 review 而非產碼；自我改進需 human oversight → 轉化為「棘輪原則」
- `.superpowers/sdd/progress.md`：舊 subagent-driven 流程的 ledger／brief／report／review 慣例（保留精神、去掉 subagent）
- `.claude/skills/gui-acceptance/SKILL.md`：實機驗收管道與專案陷阱（納入 LEARNINGS 種子）
- `package.json`：驗收指令 bun run build／test；src-tauri 用 cargo check

**假設（非 BLOCKING，已選定預設值）**
- A1: 「設計好一套」＝直接落地成可用檔案（不只是規格書），因為 Fable-5 存取期有限，交付物必須今天完成（依據：任務動機）
- A2: run file 放 `.claude/loop/runs/`、索引 `.claude/loop/LEDGER.md`（與 skill 同住 .claude，取代舊 .superpowers/sdd 位置）
- A3: 「不使用 git」解讀為閉環全面禁止執行 git 指令（含唯讀），變更追蹤改用變更檔案清單＋baseline
- A4: 「不使用 opus/sonnet/haiku」解讀為閉環內不得做模型委派（未來 session 本身跑什麼模型由使用者決定）
- A5: 強制機制三層：專案 CLAUDE.md（常駐 context）＋skill pushy description（自動觸發）＋UserPromptSubmit hook（每則訊息注入提醒）

**BLOCKING 待問**：無

## P2 規劃

**Baseline battery**：本 run 變更面為「純文件／設定」——baseline 即「目標檔案皆不存在」（CLAUDE.md、.claude/settings.json 均確認不存在後才建立，無覆蓋風險）。

**步驟**
1. 撰寫 SKILL.md（協定＋閘門） → verify: frontmatter 正確、skill 被系統註冊 → 預期: 出現在 available skills
2. 撰寫 template.md／LEARNINGS.md（種子）／LEDGER.md → verify: 檔案存在、格式一致
3. 撰寫專案 CLAUDE.md（鐵律＋路徑表） → verify: 檔案存在、引用路徑全部可達
4. 建立 .claude/settings.json UserPromptSubmit hook → verify: pipe-test＋jq schema 驗證
5. 對抗 review（P4） → verify: dry pass＋反證紀錄 ≥3
6. 產出 docs/html 設計文件 → verify: 檔案存在、依 docs/html 命名慣例

**預計變更檔案**：.claude/skills/yuzora-loop/*、.claude/loop/*、CLAUDE.md、.claude/settings.json、docs/html/新報告
**不碰範圍（non-goals）**：src/**、src-tauri/**、既有 skills、docs 既有檔案、git 狀態
**風險**：settings.json 寫壞會讓整份設定靜默失效 ／ 守門：jq schema 驗證

**驗收證據清單（實作前固定）**
- E1: hook 指令 pipe-test 輸出正確文字
- E2: `jq -e` 對 settings.json 的 hook 路徑驗證 exit 0
- E3: SKILL.md／CLAUDE.md 內所有引用路徑存在性檢查全 OK
- E4: yuzora-loop 出現在系統 available skills 清單
- E5: 設計文件存在於 docs/html/ 且符合命名慣例

## P3 執行

**步驟紀錄**
- 步驟 1-3：四檔並行寫入 → skill 立即被系統註冊（tool result 顯示 available skills 含 yuzora-loop）→ 綠
- 步驟 4：settings.json 寫入 → pipe-test＋jq 驗證 → 綠
- 步驟 5：P4 找到 F1/F2 → 修復 → 復掃 → 綠
- 步驟 6：docs/html 設計文件 → 見 P5 E5

**變更檔案**
- `.claude/skills/yuzora-loop/SKILL.md` — 閉環協定本體
- `.claude/skills/yuzora-loop/template.md` — run file 模板（每任務複製）
- `.claude/skills/yuzora-loop/LEARNINGS.md` — 自我改進日誌（含 4 條種子陷阱）
- `.claude/loop/LEDGER.md` — run 索引
- `.claude/loop/runs/2026-07-03-yuzora-loop-bootstrap.md` — 本檔
- `CLAUDE.md` — 專案級強制宣告＋鐵律＋路徑表
- `.claude/settings.json` — UserPromptSubmit hook（每則訊息注入閉環提醒）
- `docs/html/yuzora-loop-closed-loop-design-2026-07-03.html` — 設計文件

**計畫修正**：無

## P4 對抗 review

**Round 1**
- 反證嘗試 1（R1 強制性）：未來 haiku session 收到「幫我修這個 bug」，會不會直接動手跳過閉環？→ 三層防線：CLAUDE.md 常駐（最高優先段）＋skill description 觸發＋hook 每訊息注入 → 通過（殘留：hook 需重啟 session 才載入，已列殘留事項）
- 反證嘗試 2（R2 無 subagent）：閉環引用的 gui-acceptance skill 內含「§2 若派 subagent 執行」，較小模型可能誤入 → **F1**
- 反證嘗試 3（R2 無模型委派）：P1 要求閱讀的 .superpowers/sdd/progress.md 開頭寫「所有 subagent 一律使用 sonnet」，可能被當成現行指示 → **F2**
- 反證嘗試 4（R2 無 git）：協定全文與 battery 逐條掃描，無任何 git 依賴；變更追蹤由變更檔案清單＋baseline 取代 → 通過
- 反證嘗試 5（R3 可執行性）：P4 會不會被弱模型寫成「looks good」敷衍？→ 閘門要求 ≥3 條具體反證＋rubric 勾選＋dry pass 才能出場 → 通過
- Rubric：邊界✅ 時序 n/a state n/a 主題/i18n✅（技術詞英文、說明中文） scope✅（8 檔皆對應 R1-R4） 孤兒✅（無）

**Findings**
- F1: gui-acceptance §2 subagent 段落與鐵律衝突 → 修復：SKILL.md「與其他 skill 的關係」明文標註該段不適用 → 復驗：重讀修改處，語義明確
- F2: sdd 歷史檔的過時流程指示可能誤導 → 修復：P1 閱讀清單加註「流程指示已全部失效，只取 architecture decisions 與陷阱」 → 復驗：重讀修改處，語義明確

**Dry pass**：第 2 輪 0 新 finding ✅

## P5 驗收

- E1: pipe-test → 輸出 `[yuzora-loop] 任務型訊息（會改檔案或需多步驟）：先讀 .claude/skills/yuzora-loop/SKILL.md...` PASS
- E2: `jq -e '.hooks.UserPromptSubmit[] | .hooks[] | select(.type == "command") | .command' .claude/settings.json` → exit 0、印出指令字串 PASS
- E3: 14 條引用路徑存在性檢查 → 全部 OK（含 fixtures/out、.superpowers/sdd/*、docs/design/project） PASS
- E4: 系統 available skills 清單出現 `yuzora-loop`（寫入後的 tool result 即包含） PASS
- E5: `docs/html/yuzora-loop-closed-loop-design-2026-07-03.html` 存在 PASS
- 與 baseline 比對：本 run 未觸碰 src/**、src-tauri/**，無需跑 bun/cargo battery；目標檔案自無到有，無覆蓋 ✅

## P6 收尾

- 結果：DONE
- 最終變更檔案：見 P3（8 檔，全部新增，無修改既有檔案）
- 需使用者確認的假設：A2（run file 位置）、A3（git 全面禁止含唯讀）、A5（hook 每訊息注入是否會嫌吵——嫌吵可刪 .claude/settings.json 的 hook，保留 CLAUDE.md＋skill 兩層）
- 殘留事項：
  1. hook 本 session 不會生效（settings watcher 只監看啟動時已有設定檔的目錄）——下次啟動 session 自動載入，或本 session 開 `/hooks` 重載
  2. 閉環協定尚未經過較小模型實測——建議下次用 opus/sonnet 跑一個小任務驗證閘門遵循度，結果回寫 LEARNINGS
- 自我改進三問：
  - 閘門漏洞：無（本 run 為初版建立）
  - 浪費規則：暫無，待實測
  - 新陷阱／慣例：LEARNINGS 已含 4 條種子
- LEDGER 已追加：✅
