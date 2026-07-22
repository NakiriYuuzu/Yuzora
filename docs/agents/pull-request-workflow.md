# Issue、Branch、Commit 與 Pull Request 工作流程

本文件定義 Yuzora 的一般工程變更如何從 GitHub Issue 進入 `main`。部署、Release、Updater 與 Pages 的操作細節見 [`docs/operations.md`](../operations.md)。

Issue tracker 與 triage label 規則分別見：

- [`docs/agents/issue-tracker.md`](issue-tracker.md)
- [`docs/agents/triage-labels.md`](triage-labels.md)

## 1. 核心責任

| Artifact | 回答的問題 | 主要內容 |
|---|---|---|
| Issue | 為什麼做？做到什麼算完成？ | 問題、背景、範圍、acceptance criteria、優先級與決策 |
| Branch | 這項尚未完成的工作在哪裡？ | 與 `main` 隔離的實作範圍 |
| Commit | 具體做了哪一步？ | 可理解、可驗證的程式或文件修改 |
| Pull Request | 這批 commits 是否可以進入 `main`？ | Diff、實作說明、測試證據、風險、review 與 CI |
| Merge | Repository 是否正式接受這項變更？ | 通過所有 gate 後納入主線 |

Pull Request 是 implementation artifact，不是 feature request intake。非平凡工作應先由 Issue 定義需求，再由 PR 實作。

---

## 2. 何時需要 Issue

下列工作必須先有 Issue：

- 新功能或產品行為改變。
- 使用者可感知的 bug。
- 跨多個 module、store、IPC 或平台的修改。
- Security、資料庫 migration、Release、Updater 或相容性風險。
- 需要產品、UX、架構或 scope 決策。
- 需要多人協作、排程或後續驗收。

下列低風險修改可由 PR 本身承擔完整脈絡，不強制另開 Issue：

- 明確 typo。
- 純文件連結修正。
- 無行為改變的小型維護。
- Reviewer 能只從 PR 完整理解 Why、Scope 與驗證方式的修改。

不要為了符合流程建立重複 Issue。開始工作前先搜尋既有 Issues；若已有相同工作，更新該 Issue，而不是另建一張。

---

## 3. Issue ready 條件

實作前至少確認：

- 問題與使用者影響清楚。
- Scope 與 out-of-scope 清楚。
- Acceptance criteria 可驗證。
- 已讀取 Issue body、labels 與 comments。
- 沒有另一個 Issue 或 PR 正在處理相同範圍。
- 已套用 `triage-labels.md` 定義的 canonical label。

常用狀態：

- `needs-triage`：尚待 maintainer 評估。
- `needs-info`：等待回報者補資料。
- `ready-for-agent`：規格完整，可交由 agent 實作。
- `ready-for-human`：需要人工環境、權限或判斷。
- `wontfix`：明確不處理。

Issue body 保持為目前有效規格；執行進度、測試證據與後續診斷寫入 comments。

---

## 4. Branch

每一項獨立工作使用獨立 branch，不直接在 `main` 實作。

建議命名：

```text
feat/<issue>-<short-name>
fix/<issue>-<short-name>
docs/<issue>-<short-name>
refactor/<issue>-<short-name>
test/<issue>-<short-name>
release/vX.Y.Z
```

例如：

```text
fix/23-windows-terminal-ime
feat/24-app-updater
release/v0.0.3
```

Branch 應只包含該 Issue 的必要修改。發現不相關問題時建立 follow-up Issue，不順手擴張目前 PR。

### Agent Git 安全規則

Agent 不得自行執行 `git add`、`git commit`、`git push` 或等價操作，除非使用者在目前對話明確授權。建立 branch、tag、worktree 或其他會改變 Git 狀態的操作，也應先取得符合當前任務的明確授權。

---

## 5. Commit

Commit 是 PR 的實作歷史，不是 PR 的替代品。

良好的 commit 應：

- 每個 commit 只有一個可說明的目的。
- 程式與對應測試盡量放在同一 commit。
- 不混入 formatter、重構或其他 Issue 的變更。
- Commit message 說明行為，不只列出檔名。
- 任一 commit 都不得包含 secret、token、private key 或 production credential。

建議格式：

```text
<type>(<scope>): <imperative summary>
```

例如：

```text
fix(terminal): position IME near the active prompt
feat(updater): show release notes before installation
docs(operations): document release verification gates
```

常用 type：`feat`、`fix`、`docs`、`test`、`refactor`、`perf`、`chore`。

不要求為了形式製造大量極小 commits；重點是 reviewer 能理解修改順序，且最終 PR diff 保持聚焦。

---

## 6. 建立 Pull Request

### Draft PR

符合下列情況時先開 Draft PR：

- 需要提早確認架構或 UI 方向。
- 工作跨多個 checkpoint。
- 希望先取得 CI 結果，但尚未準備好 merge。
- 有已知缺口，需要在 PR 內清楚列出。

Draft 不代表可以省略說明；仍應連結 Issue 並描述目前狀態。

### PR title

PR title 應能成為清楚的 squash commit message：

```text
fix(terminal): position Windows IME near the active prompt
```

避免：

```text
fix stuff
updates
WIP
issue 23
```

### PR body

建議模板：

```markdown
## Why

Closes #<issue-number>

說明問題、使用者影響，以及為什麼現在處理。

## Scope

- 做了什麼
- 明確沒有做什麼

## Acceptance criteria

- [ ] 對應 Issue acceptance criterion 1
- [ ] 對應 Issue acceptance criterion 2

## Verification

- `bun run test ...`
- `bun run typecheck`
- `cargo test ...`
- 人工平台／GUI 證據

## Risk and rollback

- 主要風險
- 失敗時如何停用、回復或發行修正版
```

若有 UI、平台或真實資料庫行為，加入 screenshot、錄影、log、Release URL 或測試環境等可追溯證據。

`Closes #123`、`Fixes #123` 或 `Resolves #123` 只用於 PR merge 後應自動關閉的 Issue。若 PR 只完成部分工作，改用 `Refs #123` 並說明剩餘範圍。

---

## 7. Author self-review

送出正式 review 前，PR author 應先檢查：

- Diff 中每一行都能追溯到 Issue 或必要測試。
- 沒有 debug log、臨時檔、generated secret 或本機路徑。
- 沒有意外修改 lockfile、格式或無關檔案。
- 新增 UI 文字已同步 `en` 與 `zh-TW`。
- IPC 修改維持 React → `src/lib/ipc.ts` → Tauri command 邊界。
- 新增 Tauri command 已正確註冊，且 shutdown／inventory tests 未被破壞。
- Tests 先驗證使用者行為或 regression，不只驗證實作細節。
- PR body 的 commands 與證據是實際執行結果，不是預計執行項目。
- Known limitations 與未完成工作已明確列出。

若工作樹還有不屬於 PR 的修改，不要把它們一併提交；先隔離範圍或停止並詢問。

---

## 8. Review

Reviewer 沿兩個軸檢查：

### Spec

- 修改是否真正解決 Issue？
- Acceptance criteria 是否都有對應實作與證據？
- 是否出現未核准的 scope expansion？
- 使用者可見行為、錯誤狀態與平台差異是否合理？

### Standards

- 是否遵循 repository 架構與既有 style？
- 是否有可重現的測試？
- 是否引入 security、data loss、concurrency 或 release 風險？
- 是否只修改必要範圍？
- 文件、Changelog、i18n 與操作手冊是否同步？

Review comment 應指出具體失敗情境或 acceptance gap。純偏好但不影響正確性、維護性或一致性的建議，標為 non-blocking。

Author 修正後應回覆處理方式；不要只把 conversation resolve 而不說明。

---

## 9. CI 與 merge gate

PR merge 前必須：

- Required CI checks 全部成功。
- Branch 沒有 unresolved merge conflict。
- Blocking review comments 已處理。
- Acceptance criteria 已完成，或 Issue／PR 明確縮小並重新核准 scope。
- 沒有洩漏 secret 或 production credential。
- Release、Updater、workflow、database migration 等高風險變更已取得合適 reviewer。

CI 被新 commit 取消時，以最新 commit 的結果為準；較舊 commit 的綠燈不能用來 merge 新內容。

### Merge strategy

預設使用 **Squash and merge**：

- PR title 成為 `main` 上的 commit summary。
- PR body 與 Issue 保留完整討論、驗證及子 commits 歷史。
- 避免把大量 fixup commits 帶入主線。

只有在保留 commit 邊界本身具有明確價值時才使用 rebase 或 merge commit，並在 PR 說明原因。Repository 設定應逐步收斂到團隊實際採用的策略，避免每次臨時選擇。

Merge 後刪除已完成的短期 branch；release tag 與長期維護 branch 不適用此規則。

---

## 10. Merge 後

- 確認 GitHub 已依 `Closes`／`Fixes` 關閉對應 Issue。
- 若 Issue 只完成部分範圍，保留開啟並更新剩餘 acceptance criteria。
- 將 GUI、平台、Release、OTA 或 production smoke 證據回填 Issue comment。
- 觀察 `main` push CI；PR CI 成功不代表 merge commit 的 post-merge run 一定成功。
- 發現 regression 時建立新 Issue，連結原 PR 與失敗證據。
- 不在已 merge PR 偷渡新的 scope；後續修改使用新的 branch／PR。

---

## 11. Release PR 的額外規則

Release PR 除了一般 PR gate，還必須符合 [`docs/operations.md`](../operations.md)：

- 三份 version 與 `src-tauri/Cargo.lock` 一致。
- `CHANGELOG.md` 有對應版本使用者說明。
- Release／updater contract checks 通過。
- Tag 尚未存在。
- Merge 後 exact `main` commit 的完整 CI 通過，才可建立 tag。
- Tag push 與 `main` push 分開，Release workflow 不得與尚未完成的 CI 競速。
- Draft assets、`latest.json`、signatures 與三平台 smoke test 完成後才 Publish。

Release workflow、signing key、updater public key 或 stable endpoint 的變更，視為 supply-chain sensitive review。

---

## 12. Break-glass

只有 production outage、active security incident 或 repository 無法透過正常 PR 修復時，maintainer 才能考慮 break-glass。

Break-glass 必須：

1. 有明確 incident Issue 或事後立即補建。
2. 記錄為何正常 PR 流程不可行。
3. 保持修改最小化。
4. 執行可用的最低必要驗證。
5. 事後建立 review PR 或 audit record。
6. 恢復 branch／tag protection，確認沒有遺留例外權限。

方便、趕時間或修改很小，都不是 bypass CI／review 的理由。
