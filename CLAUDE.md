# CLAUDE.md — yuzora

## 快速事實

- Toolchain：bun。驗證指令：`bun run build`（含 tsc）、`bun run test`（vitest）、`cd src-tauri && cargo check`。
- dev server port 1420；`bun run tauri:dev` 起桌面 app。
- jsdom 測不到的（watcher／tab／dialog／視覺）→ 用 `gui-acceptance` skill 實機驗收。
- 語言：回覆繁體中文；技術詞英文、UI 文字中文。
- Git 寫入（add／commit／push 等）僅在使用者明確下令時執行。

## 重要路徑

| 用途 | 路徑 |
|---|---|
| 設計與計畫 | `docs/superpowers/specs/`、`docs/superpowers/plans/` |
| M1/M2 architecture decisions | `.superpowers/sdd/progress.md`、`.superpowers/sdd/m2-progress.md` |
| 驗收 checklists | `docs/superpowers/checklists/` |
| 結案報告慣例 | `docs/html/<題目>-<YYYY-MM-DD>.html` |
| 歷史 run 紀錄（已停用的閉環制度，僅供考古） | `.claude/loop/` |
