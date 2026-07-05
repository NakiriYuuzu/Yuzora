# LOOP RUN — loop-simplify

- 日期：2026-07-05
- 狀態：DONE（Q1 裁決＝整個閉環制度移除；本 run 為閉環最後一筆紀錄）
- 任務原文：
  > 將這些內容移除
  >
  > （承接前文：使用者問「為什麼開發越來越瑣碎、一個功能要跑大量 ai-review」，我診斷出五個結構性原因並列出四條可放寬的槓桿；使用者回覆「將這些內容移除」）

## P1 提問

**需求（可驗證陳述）**
- R1: 依使用者裁決的範圍，移除／放寬 yuzora-loop 協定中造成 review 輪數膨脹的規則（待 Q1 確定範圍）。
- R2: 修訂後的協定文件自洽（無殘留引用已刪規則的段落；hook／CLAUDE.md／template 與 SKILL.md 一致）。

**已查資料**
- SKILL.md 全文（前一輪已讀）：膨脹來源＝dry pass 整輪重跑＋全新 reviewer（:93-94）、reviewer 打回票目標＋≥3 反證配額（:90）、鐵律 1/2 全派工 opus、無分級制、棘輪原則（:124）。
- LEDGER：wave2 12 輪 47 修、closeout 36 輪 125 修——輪數膨脹實證。
- LEARNINGS.md：9 [tighten]／10 [trap]／9 [convention]，只增不刪。
- 棘輪原則明訂「放寬只有使用者能批准」——本次使用者已表達放寬意向，符合修訂前提；「修訂閉環協定本身需使用者明確指示」——「將這些內容移除」即該指示，但範圍有歧義。

**假設（非 BLOCKING，已選定預設值）**
- （暫無；範圍確定後補）

**BLOCKING 待問**
- Q1: 「這些內容」的移除範圍——(a) 只放寬前述四條槓桿（保留閉環骨架）；(b) 整個 yuzora-loop 制度移除（SKILL＋hook＋CLAUDE.md 強制條款）；(c) 其他組合。猜錯代價：改錯協定方向、拆掉使用者想留的制度（>30 分鐘重工＋不可逆風險）。

## 執行與收尾（Q1 裁決＝(b) 整個閉環制度移除；使用者直接指令優先於協定自身流程，未再走派工／P4）

**變更檔案**
- `.claude/skills/yuzora-loop/`（含 SKILL.md、template.md、LEARNINGS.md）— 整個刪除
- `.claude/settings.json` — 清空 UserPromptSubmit hook（`{}`）
- `CLAUDE.md` — 移除任務閉環強制條款與鐵律，保留快速事實與重要路徑
- `.claude/loop/`（LEDGER＋runs）— 保留為歷史檔案，不再有任何效力

**備註**
- 所有被刪檔案皆已在 commit ca95617 入版控，可由 git 歷史還原。
- 全域 git 安全規則（不主動 add/commit/push）不受影響，仍然有效。
