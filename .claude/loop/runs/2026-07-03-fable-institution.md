# LOOP RUN — fable-institution

- 日期：2026-07-03
- 狀態：DONE
- 任務原文：
  > 把 Fable 5 的判斷力轉成可長期沿用的制度與檔案（診斷／CLAUDE.md 重寫／調度守則／判斷力外化／派工模板／維護協議／給未來 session 的信），讓之後較弱模型的 session 都因此變強。本 session：無 subagent、無 git、無 worktree。

## P1 提問

**需求（可驗證陳述）**
- R1: `~/.claude/playbooks/harness-diagnosis.md` 落地，含漏 token／失焦／出錯三軸的頭號問題，各附量化證據與具體修法
- R2: `~/.claude/CLAUDE.md` 重寫為精簡路由檔（≤120 行），既有偏好無遺失（git 安全、語言、toolchain、style、plan-first、html-first、pwsh）
- R3: `~/.claude/rules/` 解散：git 4 檔合併去重 → `playbooks/git-conventions.md`；co-founder → `playbooks/co-founder.md`；style 內容併入 CLAUDE.md；原檔已備份
- R4: `playbooks/model-dispatch.md` 落地，model/effort 依實測 schema（不憑印象），含派工三件套／回報合約／升降級／驗證不自驗
- R5: `~/.claude/agents/` 建立 scout／implementer／verifier／reviewer 四角色定義
- R6: `playbooks/judgment.md` 落地，五類判準每條附正例＋反例，含誠實條款
- R7: `playbooks/delegation-templates.md` 落地，搜尋／實作／重構／研究／審查五型
- R8: `playbooks/maintenance.md` ＋ `playbooks/learnings.md` 落地（棘輪／備份／單一真相源／精簡週期）
- R9: `playbooks/letter-to-future-sessions.md` 落地（三件事＋退化模式與預防＋交接待辦）
- R10: 收尾：冷讀對抗自審至 dry pass、全檔 read-back、LEDGER 追加、一頁總結

**已查資料**
- LEDGER／LEARNINGS／SKILL.md v1.1／template.md 已讀；使用者已自行同步專案 CLAUDE.md 至 v1.1（原 v1.0/v1.1 衝突已解，列入診斷證據）
- 量測：rules/ 6 檔 8,535B ＋ 全域 CLAUDE.md 4,302B 每 session 全載；superpowers SessionStart 注入 ~11KB；全域 skills ~80 個＋plugin skills ~40 個
- Agent tool schema 實測：model 只有 sonnet/opus/haiku/fable；無 effort 參數；agent 定義 frontmatter 可設 model（effort key 本地無樣本可證，標 UNVERIFIED）
- rules/ 無活引用（grep commands/skills/hooks 只中歷史 log）

**假設（非 BLOCKING）**
- A1: 未來 session 的 Agent tool model enum 與今日相同（無法跨 session 驗證；已在守則中標註）
- A2: YAML frontmatter 未知 key（effort）不會使 agent 定義解析失敗（一般 YAML 行為；已標 UNVERIFIED）

**BLOCKING 待問**：無（開場四題已由使用者裁決：只重寫全域／可直接瘦身／英文為主／建立 agents）

## P2 規劃

**Baseline battery**（無 git session；以檔案清單＋大小為對照組）

```
4302 ~/.claude/CLAUDE.md
 876 rules/code/style.md | 2182 rules/workflow/co-founder.md
1187 rules/git/changelog.md | 1818 rules/git/commit.md | 1258 rules/git/pr.md | 1214 rules/git/release.md
~/.claude/playbooks/ 不存在；~/.claude/agents/ 不存在
備份：~/.claude/backups/2026-07-03-fable-institution/（CLAUDE.md＋rules/ 全 7 檔，已 read-back 確認）
```

**步驟**
1. A 診斷落檔 → verify: read-back＋引用路徑存在
2. B CLAUDE.md 重寫＋rules/ 解散（git-conventions／co-founder 遷移）→ verify: read-back＋偏好對照表無遺失
3. C model-dispatch.md＋agents/ 四檔 → verify: read-back＋frontmatter 可解析
4. D judgment.md → verify: 每條判準有正反例
5. E delegation-templates.md → verify: 五型齊備、三件套欄位齊備
6. F maintenance.md＋learnings.md → verify: read-back
7. G letter → verify: read-back＋交接待辦含未竟事項
8. 收尾：冷讀自審（找互打／錯路徑／模糊語句）→ 修至 dry pass → 全檔驗證 → LEDGER＋memory → 總結

**預計變更檔案**：上列 R1–R9 全部路徑＋本 run file＋LEDGER.md＋memory
**不碰範圍**：yuzora 專案 CLAUDE.md／SKILL.md／settings.json；~/.claude/settings.json；hooks/；plugins/（只寫建議）
**風險**：刪 rules/ 若有未發現引用 → 已 grep 確認無活引用，且備份可還原
**驗收證據清單（固定）**
- E1: 全部新檔 read-back（檔案存在＋行數）
- E2: 新舊 CLAUDE.md 偏好對照（每條舊偏好在新結構中的位置）
- E3: 檔內互相引用的路徑逐一 ls 存在
- E4: 冷讀自審紀錄 ≥3 條反證嘗試＋dry pass

## P3 執行

**變更檔案**（全域層，路徑省略 /Users/yuuzu）
- ~/.claude/CLAUDE.md — institution block（precedence／hard rules／code style／routing 表）置頂＋原始行為準則全文保留（使用者指示）
- ~/.claude/playbooks/harness-diagnosis.md — A 診斷（三軸＋量化證據＋修法＋re-audit 指令）
- ~/.claude/playbooks/model-dispatch.md — C 調度守則（實測 schema／三件套／回報合約／升降級／驗證不自驗）
- ~/.claude/playbooks/judgment.md — D 判斷力五類 rubric＋誠實條款，每條正反例
- ~/.claude/playbooks/delegation-templates.md — E 五型派工模板
- ~/.claude/playbooks/maintenance.md — F 維護協議（棘輪／備份／單一真相源／尺寸紀律）
- ~/.claude/playbooks/learnings.md — 全域教訓 log（種子 5 條）
- ~/.claude/playbooks/letter-to-future-sessions.md — G 信（三件事＋五種退化＋交接待辦）
- ~/.claude/playbooks/git-conventions.md — 原 rules/git 4 檔合併去重
- ~/.claude/playbooks/co-founder.md — 自 rules/workflow 遷移，加按需載入引言
- ~/.claude/agents/{scout,implementer,verifier,reviewer}.md — 四角色定義
- ~/.claude/rules/ — 解散（原 7 檔備份於 backups/2026-07-03-fable-institution/）
- 本 run file、.claude/loop/LEDGER.md、memory（global-institution-playbooks.md＋MEMORY.md）

**計畫修正**：使用者中途指示（1）implementer 改 opus/high（2）reviewer 改 opus/xhigh（3）CLAUDE.md 保留原始內容、新規則置頂而非取代。已同步修正 5 個引用處（model-dispatch 表格、templates ×2、diagnosis 數字 ×2、maintenance 行數上限）並記入 learnings。

## P4 對抗 review（冷讀自審；本 session 使用者禁 subagent，依 SKILL.md 鐵律 1 退回自審模式）

**Round 1 反證嘗試（節錄，共 20 項掃描）**
- 嘗試：yuzora「僅限 opus」專案遇到 dispatch 守則的 project override 條款 → **F1**：原句把 restrict 與 ban 混為一談，弱模型會誤讀成退回主對話 → 已拆分兩種情況修復
- 嘗試：使用者用英文提問時的語言規則 → **F2**：重寫版丟失「match the user's language」 → 修復（後由原文恢復自然解決）
- 嘗試：發現無關 dead code 時的行為 → **F3**：重寫版丟失「mention it, don't delete it」與 push-back 條款 → 修復（同上）
- 嘗試：CLAUDE.md hard rule 3 備份規則 vs maintenance.md 變更程序步驟 1 — 通過（同一程序，maintenance 為 canonical）
- 嘗試：routing 表 8 路徑 vs playbooks/ 實際檔案 — 通過（ls 全數存在）
- 嘗試：delegation-templates 巢狀 code fence 解析 — 通過（無巢狀）
- 嘗試：learnings 格式 vs maintenance §Where lessons go — 通過（tag 集一致）

**Round 2（使用者三項修正落地後整輪重跑）**：grep 一致性掃描無殘留（implementer=sonnet 僅存在於 learnings 的慣例紀錄行，非殘留）；表格 vs frontmatter 一致；0 新 finding。

**Dry pass**：第 2 輪 0 新 finding ✅（反證紀錄 7 條 ≥ 3）

## P5 驗收

- E1 全檔 read-back：14 檔存在、行數與檔尾完整（wc -lc 實際輸出見對話：CLAUDE.md 126 行 7,030B；playbooks 9 檔；agents 4 檔；total 735 行 50,575B）PASS
- E2 偏好對照：原始五段落標題 grep 計數 = 5、結尾句 = 1、institution block = 1，原文全文保留＋新規則置頂 PASS
- E3 引用路徑：routing 表 8 檔全存在；agents frontmatter 4/4 以 `---` 開頭可解析；備份 7 檔完整 PASS
- E4 對抗自審：dry pass＋7 條反證紀錄 PASS
- 與 baseline 比對：rules/ 移除為計畫內變更（備份可還原）；無其他檔案受影響（grep 無活引用）PASS

## P6 收尾

- 結果：DONE
- 最終變更檔案：見 P3 清單
- 需使用者確認的假設：A1（未來 session 的 Agent model enum 同今日）、A2（`effort:` frontmatter key 未驗證，YAML 未知 key 無害但效果未確認）
- 殘留事項：①驗證 effort key（找 claude-code-guide agent）②使用者裁決：skill 清理、superpowers plugin/hook 去留 ③首個可用 subagent 的 session 派 reviewer 對 playbooks＋agents 做真 fresh-context 對抗審查（信中已留指令）
- 自我改進三問：閘門漏洞—無；浪費規則—無建議放寬；新陷阱—雙檔同文分歧、rules/ 全載、effort 機制三條已入全域 learnings.md（本 run 變更屬全域層，專案 LEARNINGS 不追加）
- LEDGER 已追加：✅
- 結案報告：docs/html/fable-institution-closeout-report-2026-07-03.html（html-first，含制度地圖／驗收證據／fresh-review prompt／eval 回饋層）
