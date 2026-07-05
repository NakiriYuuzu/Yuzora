# CLAUDE.md — yuzora

## 任務閉環（最高優先）

本專案**所有任務**必須走 yuzora-loop 閉環：動手前先讀 `.claude/skills/yuzora-loop/SKILL.md`，
複製 template 建立 run file（`.claude/loop/runs/`），依 TRIAGE→P1 提問→P2 規劃→P3 執行→P4 對抗 review→P5 驗收→P6 收尾 的閘門順序執行。
只有純問答／唯讀查詢可免走，回覆開頭標 `LOOP-EXEMPT: <原因>`。

## 鐵律（v1.1，2026-07-03 使用者批准修訂）

1. 撰寫與對抗 review 交給 subagent；主對話為 controller（規劃、派工、驗證、裁決），親手只寫記帳文件（run file／LEDGER／LEARNINGS／brief）——其餘一切撰寫（含閉環協定／設定／文件修訂）一律派 opus subagent。Fallback 退自寫僅限派工實際失敗且有證據。
2. Subagent 僅限 opus（派工必帶 `model: opus`；禁 sonnet／haiku／其他）。撰寫者與 reviewer 分離；每輪 review 派全新 reviewer。
3. Git 僅限唯讀（status／diff／log／show／blame）；寫入類一律禁止——唯使用者在對話中明確下令**該具體寫入操作**才執行該一項（使用者覆寫，非閉環授權，不擴及其他寫入指令）。
4. Worktree 預設不使用；任何任務可在 P1 詢問使用者是否啟用（大範圍／高風險應問），未獲同意不用。
5. 證據先於宣稱：沒貼實際指令輸出不得寫 PASS／DONE；subagent 的證據 controller 仍須重跑驗證。

## 快速事實

- Toolchain：bun。驗證指令：`bun run build`（含 tsc）、`bun run test`（vitest）、`cd src-tauri && cargo check`。
- dev server port 1420；`bun run tauri:dev` 起桌面 app。
- jsdom 測不到的（watcher／tab／dialog／視覺）→ 用 `gui-acceptance` skill 實機驗收。
- 語言：回覆繁體中文；技術詞英文、UI 文字中文。

## 重要路徑

| 用途 | 路徑 |
|---|---|
| 閉環協定 | `.claude/skills/yuzora-loop/SKILL.md` |
| run file 模板 | `.claude/skills/yuzora-loop/template.md` |
| 歷史陷阱（P1 必讀） | `.claude/skills/yuzora-loop/LEARNINGS.md` |
| run 索引 | `.claude/loop/LEDGER.md`、`.claude/loop/runs/` |
| 設計與計畫 | `docs/superpowers/specs/`、`docs/superpowers/plans/` |
| M1/M2 architecture decisions | `.superpowers/sdd/progress.md`、`.superpowers/sdd/m2-progress.md` |
| 驗收 checklists | `docs/superpowers/checklists/` |
| 結案報告慣例 | `docs/html/<題目>-<YYYY-MM-DD>.html` |
