# LOOP RUN — git-commit-all

- 日期：2026-07-05
- 狀態：RUNNING
- 任務原文：
  > 先將當下的內容進行 git-commit (全部）
  > ---
  > 在送出之前先提供commit-message，我確認才能送出

## P1 提問

**需求（可驗證陳述）**
- R1: 當下全部內容（非 .gitignore 排除者）staged 並 commit；commit 後 `git status` 乾淨（僅剩 ignored）。
- R2: commit 前先向使用者提供 commit message，**取得確認後才執行 commit**（使用者明定閘門）。
- R3: commit message 符合專案慣例（git:commit skill：`<type><scope>: <emoji> <description>` + body；參照既有 commit 171b326 格式）。

**已查資料**
- LEDGER：M3 closeout DONE（2026-07-05），全部 14 runs DONE，無進行中工作 → 現在是乾淨的 commit 時點。
- git status：1 staged deletion（舊 .gitignore）＋22 untracked 路徑；唯一既有 commit 171b326 `feat(styles): 🚀 Add initial design system files`。
- 舊 .gitignore（被刪）內容：`.idea` / `.superpowers` / `docs/superpowers` → untracked 清單不含 `.superpowers/`、`docs/superpowers/` ⇒ 新 .gitignore 應仍排除它們（待讀檔確認）。
- git:commit skill 已載入（格式規範＋emoji 表）。skill 內「絕對不可 git add」規則與使用者本次明令「commit 全部」衝突 → 使用者直接指令優先（superpowers 優先序：user instructions > skills）。

**鐵律適用性註記**
- 鐵律 3：使用者已在本對話明確下令「git-commit (全部)」⇒ 使用者覆寫成立，僅限本次 add+commit 這一項操作；push／reset 等不在授權內。
- 鐵律 1：本 run 不撰寫任何產品／協定檔案（唯一親筆＝本 run file 與 LEDGER＝記帳文件）。commit message 為 git 操作之參數（非檔案），由使用者親自裁決，不屬派工範圍。
- 鐵律 2／P3 派工：不適用——git 寫入操作依 brief 慣例禁止 subagent 執行，必須 controller 親執。
- P4 對抗 review 之等效閘門＝使用者親自確認 commit message（任務明定）；無程式碼變更 ⇒ 無 reviewer 派工對象。

**假設（非 BLOCKING，已選定預設值）**
- A1: 「全部」＝ `git add -A` 後所有未被 .gitignore 排除的檔案（依據：使用者說「全部」；ignored 檔案本來就是使用者先前設計要排除的——但 `.superpowers/`、`docs/superpowers/` 被排除一事將在回覆中明確告知，供使用者裁決是否調整）。
- A2: commit message 語言＝中文 body＋英文 subject 技術詞（依據：171b326 為英文 subject；skill 範例 body 為中文；回覆會給選項讓使用者挑）。

**BLOCKING 待問**
- Q1: commit message 確認（＝使用者明定閘門，於 P3 呈交；得到確認前不執行 commit）。

## P2 規劃

**Baseline battery**（git 快照，唯讀）

```
171b326 feat(styles): 🚀 Add initial design system files
D  .gitignore
?? .claude/ .design/ .editorconfig .gitignore .vscode/ CLAUDE.md README.md
?? app-icon.png bun.lock components.json docs/html/ docs/recap/ fixtures/
?? index.html package.json public/ spikes/ src-tauri/ src/ tsconfig*.json vite.config.ts
（23 行 porcelain）
```

**步驟**
1. 讀新 .gitignore＋junk／大檔掃描 → verify: `git ls-files --others --exclude-standard` 掃 node_modules/target/dist/.DS_Store → 預期: 零命中、無異常大檔
2. `git add -A` → verify: `git status --short` → 預期: 全部轉 staged（無 `??`）
3. 生成 commit message（3 檔次）呈使用者 → verify: 使用者回覆確認 → 預期: 選定一則
4. `git commit` → verify: `git log -1 --stat` ＋ `git status` → 預期: 新 commit 存在、working tree clean

**預計變更檔案**
- 無檔案內容變更；僅 git index／refs（使用者授權之 add＋commit）。

**不碰範圍（non-goals）**
- 不 push、不動 remote、不改任何檔案內容、不調整 .gitignore、不 tag。

**風險**
- 可能誤入 junk（node_modules／target／dist／.DS_Store）→ 守門：步驟 1 掃描，命中即停下回報。
- 大型二進位誤入 → 守門：du 掃 top 檔案；>5MB 非預期檔案即停下回報。

**驗收證據清單（實作前固定，只可加嚴）**
- E1: junk 掃描零命中（輸出貼 run file）
- E2: `git status --short` 顯示全 staged
- E3: 使用者對 commit message 的確認訊息（對話紀錄）
- E4: `git log -1 --stat` 顯示新 commit ＋ `git status` clean

## P3 執行

**派工紀錄**
- 不派工（git 寫入必須 controller 親執；見 P1 鐵律註記）。

**變更檔案**
- （無檔案內容變更）

**計畫修正**
- （待填）

## P4 對抗 review

- 等效閘門＝使用者親自確認 commit message（見 P1 註記）。
- （待填：使用者裁決紀錄）

## P5 驗收

- （待填 E1–E4）

## P6 收尾

- （待填）
