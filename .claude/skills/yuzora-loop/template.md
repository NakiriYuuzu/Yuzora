# LOOP RUN — <slug>

- 日期：YYYY-MM-DD
- 狀態：RUNNING ｜ BLOCKED ｜ DONE
- 任務原文：
  > （逐字貼上使用者請求）

## P1 提問

**需求（可驗證陳述）**
- R1:
- R2:

**已查資料**（LEDGER／LEARNINGS／specs／plans／sdd／design——寫「查了哪裡、得到什麼」）
-

**假設（非 BLOCKING，已選定預設值）**
- A1: …（依據：…）

**BLOCKING 待問**（無則寫「無」；有則先問使用者，得到回覆前不進 P2）
- Q1:

## P2 規劃

**Baseline battery**（實際輸出尾段；含 `git status --short` 快照，唯讀）

```
（貼輸出）
```

**步驟**
1. … → verify: `…` → 預期: …

**預計變更檔案**
-

**不碰範圍（non-goals）**
-

**風險**
- 可能弄壞：… ／ 守門測試：…

**驗收證據清單（實作前固定，只可加嚴）**
- E1:

## P3 執行

**派工紀錄**（每個 brief：目標／變更範圍 → opus subagent 回報摘要 → controller 復驗 verify 結果 綠／紅）
-

**變更檔案**（subagent 回報、controller 彙整：路徑 — 為什麼）
-

**計畫修正**（如有：原因＋新增步驟；沒有寫「無」）
-

## P4 對抗 review

**Reviewer 派工**（獨立 opus subagent，未參與撰寫；brief 給 R 清單＋變更檔案＋rubric，不給作者自辯）
- reviewer：…

**Round 1（reviewer 回報）**
- 反證嘗試 1：<場景> → 結果（通過，因為 <檔案:行> ／ finding F1）
- 反證嘗試 2：
- 反證嘗試 3：
- Rubric：邊界□ 時序□ state□ 主題/i18n□ scope□ 孤兒□

**Findings 裁決**（controller：真 finding 回 P3 派修；誤判寫裁決理由；每條修復後整輪重跑）
- F1: … → 裁決：… → 修復：… → 復驗：…

**Dry pass**：第 N 輪 0 新 finding ✅／未達成

## P5 驗收

- E1: `<指令>` →

  ```
  （實際輸出尾段）
  ```

  PASS／FAIL
- 與 baseline 比對：無新紅燈 ✅／有（處理紀錄）

## P6 收尾

- 結果：DONE ｜ BLOCKED（原因）
- 最終變更檔案：
- 需使用者確認的假設：
- 殘留事項：
- 自我改進三問：
  - 閘門漏洞：無／有（已追加 LEARNINGS `[tighten]`）
  - 浪費規則：無／有（已寫進回覆建議放寬，待使用者裁決）
  - 新陷阱／慣例：無／有（已追加 LEARNINGS）
- LEDGER 已追加：✅
