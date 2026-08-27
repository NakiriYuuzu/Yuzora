# Yuzora 部署與發布運維手冊

> 本手冊的 Shell snippets 使用 **Bash／Git Bash／WSL**。Windows PowerShell 必須展開多行命令，並將 `VAR=value cmd` 改寫為 `$env:VAR = "value"`。

> 適用範圍：CI、GitHub Release、Tauri updater、GitHub Pages，以及相關失敗處理。
> 最後查證：2026-08-27。
> Repository：[`NakiriYuuzu/Yuzora`](https://github.com/NakiriYuuzu/Yuzora)。

本文件不得保存 production private key、production password、token、憑證內容或離線備份位置。Repository 內已提交的測試 fixture credential 只有在明確標示為非 production 時才能引用；其他敏感資料只存放於核准的 secret store。

## 1. Source of truth

發生不一致時，依下列順序判斷實際行為：

1. `.github/workflows/*.yml` 與其呼叫的 scripts。
2. `src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml`、`package.json`。
3. 已接受的 `.yuuzu/adr/` 架構決策。
4. 本文件。
5. 歷史規劃文件。

`docs/html/github-cicd-release-plan-2026-07-10.html` 只保留規劃背景，不是現行操作依據。任何 workflow、下載檔名、signing contract 或發布流程變更，都必須在同一個 PR 更新本文件。

Issue 與 PR 的完整工作流程見 [`docs/agents/pull-request-workflow.md`](agents/pull-request-workflow.md)。Issue tracker 慣例見 [`docs/agents/issue-tracker.md`](agents/issue-tracker.md)。

---

## 2. 變更控制原則

- 非平凡變更先建立或更新 GitHub Issue，確認問題、範圍與 acceptance criteria。
- 所有 repository 變更都在獨立 branch 完成，且只能透過 PR 進入 `main`；文件、workflow 與 release commit 也不例外。
- 不直接在 `main` 實作、補 commit、建立 release tag 或手動發布 Release。
- Release PR 必須使用 `Closes #<issue>`／`Fixes #<issue>` 連結本次完整交付的 Issues；只完成部分範圍時才使用 `Refs`。
- Release 的版本、Changelog、lockfile 與 workflow contract 必須在同一個 PR 接受 review 與 required CI。
- Release PR 不得由 agent 或 workflow 只因 CI 成功就自動 merge；必須等待使用者完成候選安裝檔驗證並明確核准。
- PR merge 後，Release workflow 只接受該 exact `main` push CI 成功的 commit，並自動建立 tag、建置、驗證與 Publish。
- Tag 只能由 Release workflow 建立，且只能指向已合併、required CI 全部成功的 immutable `main` commit。
- 已發布的 version、tag 與 artifacts 視為不可變；修正已發布版本時建立新的 patch version。

### 預期的 GitHub 保護設定

`main` 應透過 branch protection 或 repository ruleset 強制：

- Require a pull request before merging。
- Require status checks to pass before merging。
- Block force pushes。
- Block branch deletion。
- 多維護者模式至少一位 reviewer approve；單一維護者模式仍保留 PR 與 required CI。

Required CI checks：

- `Frontend (lint · typecheck · test · build)`
- `Rust compile (macOS)`
- `Rust compile (Windows x86-64)`
- `Rust compile (Linux x86-64)`
- `Real database integration (Linux x86-64)`

`v*` tags 應另設 tag ruleset，限制建立、更新與刪除權限。若 workflow job 名稱改變，必須同步更新 required check contexts。

> 查證狀態：2026-08-15 GitHub API 回報 `main` 尚未啟用 branch protection，repository rulesets 亦為空。在設定完成前，以上規則只能靠維護者人工遵守，不能視為已由平台強制；任何 direct push 都可能略過 PR、candidate 與使用者驗證 gate。

---

## 3. 三條 GitHub Actions workflow

| Workflow | 檔案                                 | 觸發                                    | 職責                                                                                                                                                                    |
| -------- | ------------------------------------ | --------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| CI       | `.github/workflows/ci.yml`           | push 至 `main`；pull request            | Frontend lint、typecheck、test、build；三平台 Rust compile；macOS fmt、exact clippy baseline、Rust tests；Linux 真實資料庫 integration；`release/*` PR macOS／Windows 候選安裝檔 |
| Release  | `.github/workflows/release.yml`      | `CI` workflow 完成                      | 只接受成功的 `main` push CI；自動建立 tag、macOS Developer ID signing／notarization、Windows 建置、updater artifact signing、暫態 draft、固定檔名別名、`latest.json` finalization 與自動 Publish |
| Pages    | `.github/workflows/deploy-pages.yml` | `main` 上 `site/**` 變更；手動 dispatch | 將 `site/` 部署到 GitHub Pages                                                                                                                                          |

Release 與 Pages 的 workflow trigger 互相獨立，但產品頁下載連結使用 `releases/latest/download/...`：發布新的 Latest Release 會立即改變產品頁實際下載內容，即使 Pages 沒有重新部署。

Pages 目前也不等待同一個 `main` SHA 的 CI 成功：`site/**` push 可在 CI 失敗或被取消時完成部署。這是已知 gate 缺口，不得把 Pages workflow 成功視為該 commit 已通過完整 CI。

### CI 重要特性

- Frontend 與 release jobs 固定使用 Bun `1.3.14`，Rust compile、database、candidate 與 Release jobs 固定使用 Rust `1.96.0`；升級任一 toolchain 時需在同一個 PR 更新 CI、candidate、Release workflow 與 exact Clippy baseline，再搭配 `@typescript/native` typecheck 驗證。
- Rust 在 macOS、Windows x86-64、Linux x86-64 執行 `cargo check --locked --all-targets`。
- Clippy 採 exact baseline；warning 新增、消失、搬移或文字改變都會使 CI 失敗。
- Database integration 在 Linux 使用 Docker 啟動 SQLite、PostgreSQL 與 MSSQL fixture。
- `release/*` PR 額外建置未發布的 macOS／Windows candidate installers，僅上傳為保留 14 天的 Actions artifacts，供使用者在 merge 前驗證；Linux 只作為 CI／測試 host，不是桌面發佈平台。
- 同一 ref 上被新 commit 取代的 CI run 會由 concurrency 設定取消。
- 現行 PR CI 沒有獨立執行 `check:version` 與 `check:updater-release`；在新增 blocking contract job 前，Release PR 必須保留第 5 節的本機 preflight 證據。

---

## 4. Release 安全邊界

Yuzora 有兩種不同的簽章邊界，不得混為一談。

### 已啟用 workflow 合約：macOS Developer ID signing／notarization

每個正式發布的 Stable 與 Beta macOS installer 都必須取得下列 GitHub Actions secrets：

- `APPLE_CERTIFICATE`：base64 編碼的 Developer ID Application `.p12`。
- `APPLE_CERTIFICATE_PASSWORD`：該 `.p12` 的匯出密碼。
- `APPLE_SIGNING_IDENTITY`：完整的 `Developer ID Application: ...` identity。
- `APPLE_ID`：供 notarization 使用的 Apple ID。
- `APPLE_PASSWORD`：Apple ID 的 app-specific password，不是一般登入密碼。
- `APPLE_TEAM_ID`：Apple Developer Team ID。

macOS release runner 會先檢查六項值皆非空且 identity 類型正確，再將 `.p12` 匯入 repository 外的暫時 keychain。Tauri build 必須完成 Developer ID signing、Apple notarization 與 stapling；產物上傳前還會逐項執行 strict `codesign`、核對 `Authority=Developer ID Application` 與 `TeamIdentifier`、執行 Gatekeeper `spctl`，並以 `xcrun stapler validate` 驗證 `.app` 與 `.dmg`。任何一步失敗都會阻止 artifact 上傳與 Publish；暫時 certificate 與 keychain 在成功或失敗後都會清除。

此處描述的是 fail-closed workflow 合約，不代表目前 GitHub repository 已完成 secret provisioning，也不代表任何尚未跑過該 workflow 的既有 artifact 已簽章。首次啟用或輪替 credentials 後，必須以實際 release run 的 macOS 驗證 step 與下載後 Gatekeeper smoke test 作為證據。PR CI candidate 為避免向 pull request 暴露 production secrets，仍刻意 unsigned，且不得發布或交付一般使用者。

### 已啟用：Tauri updater artifact signing

Release workflow 必須取得：

- `TAURI_SIGNING_PRIVATE_KEY`
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`

Public key 內嵌於 `src-tauri/tauri.conf.json`。Private key 與密碼由 GitHub Actions secrets 保管，並依本機權威文件 `.yuuzu/adr/0003-updater-signing-key-custody.html` 保留 repository 外加密備份；該 ADR 不屬於公開 runbook，也不得複製其中的敏感保管細節。

任何可存取上述 secrets，或會影響下列檔案的修改，均屬供應鏈安全敏感變更：

- `.github/workflows/release.yml`
- `scripts/verify-updater-release-contract.ts`
- `scripts/finalize-updater-metadata.ts`
- `scripts/prepare-updater-metadata.ts`
- `src-tauri/tauri.conf.json` 的 updater public key／endpoint

這些變更應由指定 maintainer review。建議後續將 signing secrets 放入具 required reviewer 的 protected GitHub Environment；在完成前不得宣稱此 gate 已啟用。

### 尚未啟用：Windows 作業系統平台簽章

- Windows Authenticode code signing 尚未啟用。
- Updater artifact signature 不會消除 Windows SmartScreen 警告；macOS Gatekeeper 信任必須由上述 Developer ID／notarization gate 獨立證明。

### 目前仍需人工補強的 gate

在 workflow 修正前，不得把下列條件描述為已由自動化強制：

- Release guard 只辨識成功的 `main` push CI，尚未查證該 SHA 是否來自 `release/vX.Y.Z` PR、是否跑過 candidate jobs，或是否取得使用者核准。
- Release guard 尚未從 GitHub 查證 source SHA 對應的 PR、candidate run 與使用者核准；既有同版本 draft 的 tag SHA 已強制必須等於本次 `workflow_run.head_sha`，不一致時會 fail closed。
- Release actions 已固定到經審查的完整 commit SHA，checkout 一律停用 persisted credentials；仍應定期審查並更新 pin，並將 signing secrets 移入具 required reviewer 的 protected Environment。
- Metadata finalizer 目前確認 URL／signature 非空與同名 artifact／`.sig` 存在，但尚未強制 URL 屬於目前 repository/tag，也未比較 metadata signature 與 `.sig` 內容。

---

## 5. Release PR

每次發版先建立 release Issue 或在既有 release Issue 更新 acceptance criteria。Stable 使用 `release/vX.Y.Z` branch；Beta 使用 `release/vX.Y.Z-beta.N`。CI 會驗證 branch 必須精確等於 `release/v<package.json version>`，因此 Beta branch 不可只使用泛用 `release/` prefix。

### Stable 與 Beta（GitHub Pre-release）

Yuzora 只使用 GitHub **Pre-release** 表示 Beta，不建立額外的 Beta channel：

| 類型 | Version／tag | GitHub Release | Latest／OTA |
| ---- | ----------- | -------------- | ---------- |
| Stable | `X.Y.Z`／`vX.Y.Z` | `prerelease=false` | 設為 Latest，更新 stable `latest.json` |
| Beta | `X.Y.Z-beta.N`／`vX.Y.Z-beta.N` | `prerelease=true` | 不得設為 Latest；不提供 Beta OTA endpoint，只供手動下載 |

規則：

- Beta 只接受 `X.Y.Z-beta.N`；不以 `rc`、build metadata 或其他自訂 suffix 表示 Beta。
- Beta 不得更新 stable `latest.json`、`releases/latest` 或產品頁固定下載入口。
- Beta 只發布供手動下載的 installer，必須停用 updater artifacts，不產生 `latest.json` 或 updater `.sig`，也不需要存取 updater signing secrets；但 Beta macOS installer 仍必須完成 Developer ID signing／notarization。
- Windows Installer 的 `ProductVersion` 比較只使用三個 numeric fields；所有 channel 透過 `scripts/release-msi-build-config.ts` 產生暫時的 `bundle.windows.wix.version`，不改產品／tag version。第三欄以 `patch * 256 + channel` 編碼：`beta.N` 使用 `N`（1–254），stable 使用 255。例如 legacy `0.0.8` < `0.0.9-beta.1`（`0.0.2305`）< `0.0.9`（`0.0.2559`）< `0.0.10-beta.1`；helper 會拒絕超出 MSI numeric bounds 的 major、minor、patch 或 beta sequence。PR candidate 與 Beta build 都停用 updater artifacts並清空 updater endpoints；Beta macOS release 仍保留 OS signing／notarization，Stable build 另保留 updater signing、stable endpoint 與 updater artifacts。
- PR candidate 是未簽章、未發布的 Actions artifact，用於 merge 前驗證；它不是 Beta Release。
- `.github/workflows/release.yml` 會由版本分類自動選擇 channel：Stable 維持 updater signing、metadata、固定下載別名與 `--latest`；Beta 使用獨立 no-updater build／publish path，固定 `prerelease=true` 且不傳入 `--latest`。兩個 channel 的 macOS release 都必須 Developer ID signed／notarized。不得手動改 GitHub Release 旗標繞過此流程。

### PR 必須包含

- `package.json` version。
- `src-tauri/tauri.conf.json` version。
- `src-tauri/Cargo.toml` version。
- 更新後的 `src-tauri/Cargo.lock`。
- `CHANGELOG.md` 中對應完整 version 的使用者可讀章節，例如 `## [X.Y.Z]` 或 `## [X.Y.Z-beta.N]`。
- 必要的 release／updater contract 修改與測試。

`CHANGELOG.md` 只記錄使用者能感受到的新增、改善、修正與已知限制，不放 commit、內部檔名或純實作細節。GitHub Release body 與 `latest.json.notes` 會由該版本章節自動產生，因此 release notes 必須在 tag 前完成，不能等到 Publish 時才補。

修改 `src-tauri/Cargo.toml` version 後，先讓 Cargo 更新 root package 的 lockfile entry，再確認 `src-tauri/Cargo.lock` 沒有意外的 dependency 變動：

```bash
cd src-tauri
cargo check
cd ..
git diff -- src-tauri/Cargo.lock
```

### Release contract preflight

Stable lane 在乾淨的 release branch 執行：

```bash
VERSION="X.Y.Z"
GITHUB_REF_NAME="v${VERSION}" bun run check:version
bun scripts/release-notes.ts "v${VERSION}"
bun run check:updater-release
```

Beta lane 使用相同 version／notes preflight，但必須驗證獨立的 prerelease contract：

```bash
VERSION="X.Y.Z-beta.N"
GITHUB_REF_NAME="v${VERSION}" bun run check:version
bun scripts/release-notes.ts "v${VERSION}"
bun run check:beta-release
```

PowerShell 使用以下等價步驟；完成後移除只供 preflight 使用的環境變數：

```powershell
$Version = "X.Y.Z" # Beta 改為 X.Y.Z-beta.N
$env:GITHUB_REF_NAME = "v$Version"
bun run check:version
bun scripts/release-notes.ts "v$Version"
if ($Version -match '-beta\.[1-9][0-9]*$') {
  bun run check:beta-release
} else {
  bun run check:updater-release
}
Remove-Item Env:GITHUB_REF_NAME
```

三項都必須成功：

- 三份 product version 與 tag contract 一致。
- `CHANGELOG.md` 存在對應版本且內容非空。
- Stable：Updater signing、macOS Developer ID signing／notarization、stable endpoint、PR merge 後自動 tag／Publish、暫態 draft、MSI-only Windows OTA 與 metadata finalizer contract 完整。
- Beta：macOS Developer ID signing／notarization、`prerelease=true`、沒有 updater signing secrets、沒有 updater artifacts／`.sig`／`latest.json`／stable aliases，且 publish command 不含 `--latest`。

另外確認遠端 `v${VERSION}` tag 與同版本 GitHub Release 都不存在。若已存在 Published Release，不能重用 version；若存在 draft，Release guard 會先強制確認其 tag SHA 與成功的 `main` CI SHA 完全一致，否則 fail closed。符合的 same-SHA draft 只視為前次嘗試留下的可修復狀態：workflow 仍會重新建置 macOS／Windows、修復同一 draft 的 notes 與 assets，再重新通過完整發布 gate。

### 對齊 CI 的本機檢查

```bash
bun install --frozen-lockfile
bun run lint
bun run typecheck
bun run test
bun run build

cd src-tauri
cargo check --locked --all-targets
cargo fmt --package yuzora -- --check
ruby ../.github/scripts/verify-clippy-baseline.rb ../.github/clippy-baseline.json
cargo test --locked
cd ..
```

`bun run build` 會再次執行 typecheck；此處保留獨立 typecheck，以對齊 CI gate 並讓失敗位置清楚。

如需本機執行真實資料庫 integration：

```bash
docker compose -f tests/database/docker-compose.yml --profile mssql up -d --wait
cd src-tauri
YUZORA_P8_DATABASE_PASSWORD='Yuzora-P8-Only-2026!' \
  YUZORA_DATABASE_TEST_ENGINES=sqlite,postgres,mssql \
  cargo test --locked --test database_integration -- --ignored
cd ..
docker compose -f tests/database/docker-compose.yml --profile mssql down -v
```

上述密碼只屬 repository fixture，不是 production secret。

### PR 候選安裝檔與使用者驗證 gate

`release/vX.Y.Z` 與 `release/vX.Y.Z-beta.N` PR 的 CI 都會執行兩個 `Release candidate (...)` jobs；job 會拒絕與 product version 不相符的 release branch。候選檔有以下限制：

- 只存在 GitHub Actions artifacts，不建立或更新 tag。
- 不建立 GitHub Release，也不會成為 `releases/latest`。
- 關閉 updater artifact 產生與 signing，只用於 merge 前的互動式功能驗證。
- 未啟用 OS code signing，Windows SmartScreen 與 macOS Gatekeeper 仍可能警告。

從 PR 的 CI run 下載 Windows 候選檔：

```bash
RUN_ID="123456789" # PR CI 頁面 URL 中的 run ID

gh run download "${RUN_ID}" \
  --repo NakiriYuuzu/Yuzora \
  --name yuzora-release-candidate-windows-x86-64
```

需要 macOS 候選檔時，將 artifact name 改為 `yuzora-release-candidate-macos-universal`。

使用者至少要在本次受影響平台驗證 acceptance criteria。Windows terminal／IME 版本至少包括：

- Microsoft Pinyin 中文 composition、replacement、commit 不重複也不遺失。
- Command Prompt、Windows PowerShell、PowerShell 7 能依設定與單次選擇啟動。
- WSL default 與已安裝 distro 能啟動，Windows／UNC workspace 的 cwd 轉換正確。
- Herdr 沒有任何 Space、連線失敗或不可用時，ADE 必須仍提供 Open Local Folder escape；本機 Terminal 在沒有 workspace 時必須開啟 folder picker，不可 silent no-op。
- Windows-native HERDR 0.8.0 running session 可透過 named pipe 完成 snapshot、schema-gated mutation 與 `events.subscribe`；停止、不相容或缺少 method 時仍需顯示真實 unavailable 原因。若 pane 透過 `wsl.exe` 進入互動式 Linux shell，不得假定 HERDR 能看見其中的 Linux descendant Agent。
- Windows installer 必須包含 `herdr/windows-x86_64/herdr.exe` 與同版 ConPTY runtime。PATH 存在 Herdr 時使用全域版本；暫時移除 PATH 版本並重啟後，必須自動解析到 Yuzora-managed protocol-19 binary，且診斷顯示 `configured=global`、`resolved=default` 與實際 managed path。
- HERDR 診斷與工作區信任介面不顯示 Windows `\\?\` verbatim prefix，且信任授權／撤銷仍作用於原始 canonical path identity。
- 一般 shell 與 TUI 模式的 IME anchor／輸入位置可接受。

驗證結果必須寫入 PR comment 或 review，包含平台、installer、結果與已知限制。只有使用者明確表示「驗證通過」並授權 merge，maintainer／agent 才能 merge。CI 全綠、artifact 存在或 reviewer 沒有留言，都不能推定為使用者核准。

### Merge 前

- PR diff 不含無關修改。
- Acceptance criteria 有對應測試或人工證據。
- Required CI checks 全部成功。
- Release candidate jobs 成功，且使用者已回報受影響平台驗證通過並明確授權 merge。
- Review conversation 全部處理完成。
- Release／updater 敏感檔案已有合適 reviewer。
- PR body 對本次完整交付的 Issues 使用 `Closes`／`Fixes`，讓 merge 自動關閉 Issues；未完成的 Issue 只能使用 `Refs`。
- 遠端 version tag 與同版本 Published Release 不存在；既有 draft 必須先核對 tag SHA，且不得把 draft 內既有 assets 當成已驗證候選。
- PR comment 記錄 release PR number、候選安裝檔 run ID、驗證平台與 candidate head SHA，避免後續一般 `main` commit 被誤認為已驗證的 release source。
- Merge 後由 Release workflow 等待並查證 `main` 上該 exact commit 的 push CI；PR CI 綠燈本身不會直接發布。

---

## 6. PR merge 後自動建立 Release

自動流程依 product version 選擇 Stable 或 Beta channel。Release tag 不由本機或 maintainer 手動建立；完整入口是 release PR：

1. Release PR 包含版本、lockfile、Changelog 與必要的 workflow／contract 修改。
2. PR required CI 與 candidate builds 成功後保持開啟，等待使用者下載安裝檔並完成實機驗證。
3. 使用者在 PR 明確回報驗證通過並授權 merge 後，才 merge 至 `main`；merge 同時透過 `Closes` 關閉已完成 Issues。
4. `main` push 觸發完整 CI；Release workflow 透過 `workflow_run` 接收完成事件。
5. Guard 只接受 `event=push`、`head_branch=main`、`conclusion=success`，並 checkout `workflow_run.head_sha`，確保後續 tag、build 與 checks 使用同一個 immutable commit。
6. Guard 從該 commit 的 `package.json` 解析唯一允許的 Stable `X.Y.Z` 或 Beta `X.Y.Z-beta.N` version，執行版本與 release notes checks；Stable 再執行 updater contract，Beta 改執行 prerelease isolation contract。
7. 若版本 tag 不存在，workflow 建立 annotated `v<version>` tag 並精確指向該成功 CI SHA；接著開始建置。若既有 same-SHA draft，workflow 也會重新建置兩平台並修復該 draft；tag SHA 不同時會 fail closed。
8. 若相同版本已 Published，workflow 安全略過，不會因後續一般 PR 重複發布。

流程政策將 PR 定義為唯一 repository 變更入口，並避免「PR CI 綠燈但尚未進入 `main`」就對外發布。不過，現行 Release guard 尚未從 GitHub 查證 source SHA 對應的 PR、candidate run 與使用者核准；在 branch protection 與此 gate 完成前，仍需依第 4、5 節人工核對。CI、tag、Release 與 Issue 關閉的關係如下：

```text
Issue ──Closes──> Release PR ──candidate artifacts──> user validation
                                                        │ explicit approval
                                                        ▼
                                                     PR merge
                                                        │
                                                        ▼
                                                  main push CI
                                                        │ success
                                                        ▼
                                             auto tag / build / verify
                                                        │ all gates pass
                                                        ▼
                                                   auto Publish
```

若 upgrade 前已存在同版本、且 tag SHA 已由 Guard 驗證等於本次 `workflow_run.head_sha` 的 draft Release，Guard 會進入修復模式：macOS／Windows 都重新建置，`assemble-draft` 重新同步 guard-verified notes、以 `--clobber` 覆寫 versioned assets 與 Stable aliases，再執行該 channel 的完整 metadata／Publish gate。Draft 在 Guard 後消失、變成 Published、channel 改變、notes 讀回不一致或 asset 驗證不完整時都會 fail closed，不會沿用部分或過期候選。

---

## 7. Release workflow 階段

### 7.1 Guard

在任何平台建置前驗證：

1. 上游事件是成功完成的 `main` push CI，而不是 pull request 或其他 branch。
2. Checkout SHA 與成功 CI 的 `workflow_run.head_sha` 完全一致。
3. Stable 才驗證 `TAURI_SIGNING_PRIVATE_KEY` 與 password secret；Beta build step 不接收這些 secrets。
4. 解析出的 tag、`package.json`、`tauri.conf.json`、`Cargo.toml` version 一致，且版本只可為 Stable 或 `-beta.N`。
5. `CHANGELOG.md` 有該版本 release notes。
6. Stable 驗證 updater release contract；Beta 驗證 prerelease isolation contract。
7. 新版本由獨立、無 checkout 的 `create-tag` write job 建立 annotated tag；既有 draft 的 tag SHA 必須與 CI SHA 一致並觸發雙平台重建；已發布版本安全略過。

Guard 與後續 build／metadata jobs 都是 `contents: read`：它們可以 checkout 並執行 repository code，但沒有 write-capable token。所有 contents write 都只存在於無 checkout、只執行固定 inline `gh`/shell 的 job。現行 Guard 不負責證明該 SHA 來自 release PR 或已完成 candidate／使用者驗證；這些仍是明確的人工 gate。任何 guard failure 都不會進入 build。

### 7.2 雙平台建置與 artifact boundary

`fail-fast: false`，單一平台失敗不會中止其他平台：

- Stable macOS universal：Apple Silicon＋Intel；Developer ID signed／notarized，產生 `.dmg`、`.app.tar.gz` 與 updater signature。
- Stable Windows x64：本機產生 NSIS `setup.exe`、`.msi` 與 MSI updater signature。
- Beta macOS／Windows：產生供手動下載的 versioned installers，但不產生 updater archive、`latest.json` 或 `.sig`，且 build environment 不含 updater signing secrets 與 contents-write token；macOS 仍必須 Developer ID signed／notarized，Windows 仍無 Authenticode。

`build` job 只執行 `bun tauri build`、驗證 Tauri CLI 的實際 bundle paths，並以 Actions artifacts 上傳結果；它不建立或上傳 GitHub Release。Fresh release 與 same-SHA draft recovery 都必須讓兩平台 build 成功。之後獨立的無 checkout `assemble-draft` write job 下載已驗證的 Actions artifacts，先在任何 GitHub Release mutation 前驗證本地 handoff與Stable alias sources：沒有 Release 時才建立暫態 draft `Yuzora v<version>`；已有 Guard 核准的 draft 時，重新驗證 draft／channel、同步並讀回比對 release notes。接著以 `gh release upload --clobber` 上傳全部 versioned assets；Stable 固定檔名 aliases 也以 `--clobber` 覆寫。Draft 只用來避免 matrix 尚未完成時讓部分資產對外可見，不是人工發版佇列，也不是略過重建的信任來源。

### 7.3 固定檔名別名

供產品頁 `releases/latest/download/...` 使用：

| 平台    | 固定檔名                                                                            |
| ------- | ----------------------------------------------------------------------------------- |
| macOS   | `Yuzora-macos-universal.dmg`                                                        |
| Windows | `Yuzora-windows-x64-setup.exe`、`Yuzora-windows-x64.msi`                            |

固定檔名如有變更，必須在同一個 PR 更新所有實際 consumer：

- 三個 alias 都要同步 `.github/workflows/release.yml` 與本文件。
- 產品頁直接使用的 macOS DMG 與 Windows NSIS EXE，還要同步 `site/index.html`、`site/downloads.js` 與 `tests/site-downloads.test.js`。
- MSI 若新增其他頁面或 script consumer，也要一併更新並補測試。

固定別名只屬 Stable 手動下載入口；Beta 不會上傳、覆寫或驗證它們。Tauri Stable updater 使用具版本號且帶 `.sig` 的 updater artifacts；Beta 不產生 updater artifacts，兩者不可混為一談。

### 7.4 Finalize updater metadata

只有 Stable 雙平台 build 與 `assemble-draft` 都成功後，metadata 才採兩段式 boundary；same-SHA draft recovery 沒有略過 build／assembly 的旁路：

1. `prepare-updater-metadata` 是 read-only checkout job。它從 draft 以 read token 取得 asset inventory與 `.app.tar.gz.sig`／`.msi.sig`，執行 repository-owned metadata generator，驗證 version、notes、macOS universal archive、MSI URL 與 signatures，然後把 `latest.json` 作為 Actions artifact 上傳。
2. `upload-updater-metadata` 是無 checkout 的 contents-write job。它下載該 metadata artifact、移除 draft 中殘留的 Linux AppImage／DEB／RPM assets，再以 `gh release upload --clobber` 取代 `latest.json`；它不執行 repository code。

不得讓 write-capable token 進入 metadata generator。任一段失敗時不得 Publish。

### 7.5 Automated publish gate

`publish-release`（Stable）是無 checkout 的 contents-write verification/publish job，只在 macOS／Windows 重建、draft assembly、metadata preparation 與 metadata upload 全部成功後執行。Fresh release 與 same-SHA draft recovery 使用相同 gate，不存在以既有 draft 略過 build／assembly 的 Publish 旁路。

Publish 前 workflow 自動驗證：

- Release 仍是 draft、不是 prerelease，且 release body 非空。
- 三個固定檔名別名與 `latest.json` 齊全。
- `latest.json.version` 與 tag 相同，notes 非空。
- `darwin-aarch64`、`darwin-x86_64`、`windows-x86_64` 都有非空 URL 與 signature。
- 不含 Linux 或 Windows NSIS updater key、不含 Linux 固定別名資產，且 Windows OTA URL 使用 `.msi`。

Stable 全部成功後執行 `gh release edit --draft=false --prerelease=false --latest`，並再次查證 `publishedAt`。任一條件失敗時 workflow 結束為失敗，Release 保持 draft，不會出現部分成功卻永久等待人工 Publish 的正常路徑。

`publish-beta-release` 使用獨立、無 checkout 的 contents-write job，僅在 macOS／Windows Beta 重建與 draft assembly 都成功時執行。它驗證 release body 與版本化 `.dmg`、`setup.exe`、`.msi`，並拒絕 `latest.json`、任何 `.sig`、所有 stable fixed aliases，以及任何額外 asset；最後只執行 `gh release edit --draft=false --prerelease=true`，絕不傳入 `--latest`。Beta 不執行 stable metadata finalizer，亦不改變 `releases/latest`。

---

## 8. 自動發布與發布後 smoke test

Release workflow 的 automated publish gate 是 blocking gate；Stable 的 macOS／Windows build、固定別名、updater signatures、metadata completeness 或 MSI-only contract 任一失敗都不會 Publish。Beta 則要求兩平台 versioned installer 完整且不含 updater／stable assets。正常成功路徑不需要 maintainer 再按一次 Publish。

受影響平台的主要互動式驗收已在 release PR merge 前完成。Release Published 後仍應儘快確認正式 artifacts 與 updater 路徑：

- macOS DMG 掛載、安裝與首次啟動。
- Windows NSIS／MSI 安裝；OTA 預期路徑以 MSI 為準。
- 從上一個 stable 版本執行 updater smoke test。
- 確認 release notes 已揭露尚未啟用 Windows Authenticode 的警告，並記錄 macOS Developer ID／notarization 驗證結果。

若人工驗收發現 regression，不覆寫已發布 tag 或 artifacts；立即建立 incident Issue，必要時隱藏受影響 Release，並透過新的 patch release PR 修正。平台驗收結果、Release URL、測試平台與診斷證據回填 release Issue。

---

## 9. Publish 後驗證

### Stable

Stable automated publish gate 成功後，`releases/latest` 會立即指向新版本，產品頁固定下載連結與 App updater endpoint 同時開始對外生效。

### GitHub Release 與 updater metadata

```bash
VERSION=X.Y.Z

gh release view "v${VERSION}" \
  --repo NakiriYuuzu/Yuzora \
  --json tagName,isDraft,isPrerelease,publishedAt,url \
  --jq .

curl -fsSL \
  https://github.com/NakiriYuuzu/Yuzora/releases/latest/download/latest.json \
  | jq '{version,notes,platforms:(.platforms|keys)}'
```

確認：

- Latest Release 為剛發布的 tag。
- `latest.json.version` 等於新版本。
- `latest.json.notes` 非空。
- 至少存在 `darwin-aarch64`、`darwin-x86_64`、`windows-x86_64`。
- 沒有 `windows-*-nsis` key。
- 所有 Windows updater URLs 指向 `.msi`。
- Metadata 中每個 artifact URL 與 signature 都可下載。

### 固定下載 URL

至少確認以下 URL 回傳成功：

- `Yuzora-macos-universal.dmg`
- `Yuzora-windows-x64-setup.exe`
- `Yuzora-windows-x64.msi`

### OTA smoke test

從上一個 stable 版本，在 macOS universal 與 Windows x64 驗證：

1. App 發現新版本。
2. 顯示的 release notes 正確。
3. 下載成功並顯示進度。
4. Signature verification 成功。
5. 安裝與重新啟動成功。
6. Runtime version 顯示新版本。
7. 使用者資料與未儲存文件保護符合預期。

將 Release URL、平台、起始版本、目標版本、結果與診斷證據回填 release Issue。真實 OTA 驗收不得只以 CI artifact 存在代替。

### Beta

Beta 只驗證 GitHub Pre-release 與手動下載，不執行 OTA smoke test：

```bash
VERSION="X.Y.Z-beta.N"

gh release view "v${VERSION}" \
  --repo NakiriYuuzu/Yuzora \
  --json tagName,isDraft,isPrerelease,publishedAt,url \
  --jq .
```

確認 `isDraft=false`、`isPrerelease=true`，且 `releases/latest`、stable `latest.json` 與產品頁固定下載連結仍指向原 Stable。Beta 安裝與啟動結果回填 release Issue；不得把 Beta 成功推定為 Stable release approval。

---

## 10. 失敗與復原

| 狀況                        | 處理原則                                                                                                                                                              |
| --------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Candidate／使用者驗證未完成 | Release PR 保持開啟，不建立 tag、不關閉 Issues、不觸發 Release。修正後重新產生 candidate，直到使用者明確核准 merge。                                                  |
| `main` CI 失敗              | Release workflow 不會建立 tag 或建置。以新的修復 PR 讓 `main` 恢復綠燈。                                                                                              |
| Guard 失敗                  | 不進行 build。修正版本／Changelog／secret／workflow contract，且修正本身也必須走 PR。只有尚未 Publish、未被消費且經 maintainer 明確確認的錯誤 tag，才可進行受控清理。 |
| Draft tag SHA 不一致        | 立即取消 workflow，不得以新 metadata 發布舊 binary。記錄 source SHA、tag SHA 與 draft assets，經 maintainer 授權後決定受控清理或新版本重建。                          |
| Beta 被標為 Latest／Stable  | 立即停止 Publish；不得覆蓋 stable `latest.json` 或固定下載入口。修正 prerelease contract 後以新的 Beta version 重建，不重用已對外發布的 tag。                         |
| 單一平台失敗                | Release 保持 draft。可在同一 run re-run failed jobs；若改以完整 rerun 恢復，Guard 核准的 same-SHA draft 會重建兩平台並以 `--clobber` 修復同名 assets，不得手動混用其他 run／SHA 的候選。 |
| Draft assembly 部分失敗     | Release 保持 draft。完整 rerun 會重新建置兩平台、同步並比對 notes，再以 `--clobber` 覆寫 versioned assets／Stable aliases；draft 消失、已發布或 channel 改變時必須停止。 |
| Finalizer 失敗              | 不得 Publish。檢查 `latest.json`、Windows MSI metadata、缺少的 artifact 或 `.sig`；同一 run 可重試失敗 jobs，完整 rerun 則必須重新通過 build／assembly。              |
| 固定別名缺漏                | 檢查 assembly log、artifact path pattern 與來源候選；由 workflow 的 `--clobber` repair 重傳，不以人工上傳或沿用未驗證 asset 繞過 gate。                                |
| Automated publish gate 失敗 | Release 保持 draft。以 PR 修正 contract 或 workflow；不得手動略過 gate Publish。                                                                                      |
| 發布前需緊急停止            | 在 `Publish verified release` 執行前取消 workflow。Draft 不會成為 `releases/latest`；後續 tag／draft 清理仍需 maintainer 明確授權與事件記錄。                         |
| 已 Publish 後發現問題       | 不覆寫 artifacts、不重用 tag/version。先建立 incident Issue、評估是否暫時隱藏錯誤 Release，再發布新的 patch version 作為永久修正。                                    |
| Updater key 疑似外洩        | 立即停止 Release、限制 secret 存取並啟動供應鏈安全事件。不得直接換 public key；既有安裝只信任內嵌 key，輪替需要獨立遷移設計。                                         |
| Updater key 遺失            | 從核准的加密備份復原並稽核存取。若無可用 private key，既有安裝的信任鏈可能無法延續，必須升級為 release incident。                                                     |
| Pages 部署失敗              | 確認 Pages source 為 GitHub Actions、artifact path 只指向 `site/`，並檢查 deploy job log。                                                                            |
| Pages 成功但 exact-SHA CI 失敗／取消 | 不把部署視為已驗證。確認線上內容影響；必要時透過正常 PR 回復 `site/**` 至已知正常版本，並重新部署有成功 CI 的 SHA。                                          |
| Pages 發布錯誤內容          | 透過正常 PR 回復 `site/**` 至已知正常版本，再讓 Pages workflow 重新部署；不直接改寫遠端 branch 歷史。                                                                 |

任何 destructive cleanup、tag 刪除、Release 隱藏或 secret rotation 都需要 maintainer 明確授權與事件記錄。

---

## 11. GitHub Pages

### 來源與觸發

- Deploy artifact 是完整 `site/` 目錄；現行入口包含 `index.html`、`styles.css`、`app.js`、`downloads.js` 與 `assets/`。網站 favicon、導覽與頁尾使用的 `site/assets/yuzora-icon.png` 必須與目前桌面 app 的 `src-tauri/icons/128x128@2x.png` 一致。
- `main` 上 `site/**` 有變更時自動部署，也可從 Actions 手動 dispatch `Deploy Pages`。
- 現行 Deploy Pages 不等待 CI；部署後必須另外確認相同 `head_sha` 的 `CI` push run 成功。後續應改成 successful `workflow_run` exact-SHA gate，或在部署 workflow 內執行完整 site checks。
- Pages 沒有 bundling step，不得引用 `node_modules` runtime path。
- `site-remotion/` 是影片原始碼，不包含在 Pages artifact。

### 產品頁維護邊界

- 產品頁是無 bundling 的靜態 HTML／CSS／ES module；`app.js` 負責中英文、light/dark theme、section reveal、active navigation、影片 viewport lifecycle、GitHub star badge 與 command palette，平台下載仍由 `downloads.js` 負責。
- 語言切換必須同步 still src、video source、poster、alt、placeholder、aria-label 與 meta/OG content；新增 markup key 時，`app.js` 的 `zh-Hant` 與 `en` dictionaries 必須同時提供。
- Theme 遵循系統偏好並保存至 `yuzora-theme`；no-JS、mobile 與 `prefers-reduced-motion` 必須保持內容可讀，不得依賴動畫才能看見主要資訊。
- Hero、三段 feature media、ADE/HERDR boundary、bento 功能矩陣與 download section 是現行資訊架構；已移除的 Exploded View、Agent Inspector still 與 model showcase 不得重新被 Pages 引用。
- `#primary-download`、`#download-device-note`、platform rows 與 recommended badges 是 `downloads.js` 的穩定 contract。
- 現行無 GSAP 或其他 Pages runtime dependency；不要以 smooth-scroll library 取代 native scrolling。

### 產品頁 smoke test

- Canonical URL 可開啟，HTTP 正確導向 HTTPS。
- Hero、三步工作流、三段 feature media、ADE/HERDR boundary、bento 功能矩陣與 download section 可讀。
- 中文／英文切換後，全部 still、poster、video source、placeholder、meta content 與 accessibility labels 正確。
- Light/dark theme 初始值、手動切換與 persistence 正確；mobile 沒有水平捲軸，no-JS 與 reduced motion 不會隱藏主要內容。
- Feature videos 進入 viewport 時播放、離開時 pause；分頁離開後不應持續播放或消耗資源。
- 裝置偵測只推薦支援的平台與架構，主要下載 CTA 指向固定檔名 Release assets。
- 未支援的 mobile、ChromeOS、Linux、ARM／32-bit Windows 不會收到錯誤的桌面下載推薦。

截至 2026-08-15，GitHub Pages API 回報頁面 URL 為 `http://github.yuuzu.net/Yuzora/`、`https_enforced=false`，外層由 Cloudflare 導向 HTTPS。DNS、Cloudflare 規則、canonical URL 與監控方式應由 maintainer 另行保管；Cloudflare challenge 可能讓單純的無瀏覽器 `curl` smoke test 回傳 403，不能直接等同於頁面部署失敗。

### 功能影片與 still 重製

原始碼位於 `site-remotion/`；render commands 與 composition 規則見 `site-remotion/README.md`，實際 media naming 以 `site-remotion/src/Root.tsx`、`site/app.js` 與 `tests/site-page.test.js` 為準。

```bash
cd site-remotion
bun install

for c in ade-herdr-zh ade-herdr-en remote-db-zh remote-db-en terminal-git-zh terminal-git-en; do
  bunx remotion render "$c" "../site/assets/$c.mp4" --scale=2
done

for lang in zh en; do
  bunx remotion still "ade-herdr-${lang}" \
    "../site/assets/ade-herdr-runtime-${lang}.png" \
    --frame=148 --scale=2
  bunx remotion still "terminal-git-${lang}" \
    "../site/assets/terminal-git-${lang}.png" \
    --frame=270 --scale=1
  bunx remotion still "remote-db-${lang}" \
    "../site/assets/remote-db-${lang}.png" \
    --frame=210 --scale=1
done
```

ADE/HERDR runtime、remote database 與 terminal/git poster stills 必須使用同語言的真實 Remotion frame；Agent Inspector still 已移除。預覽使用 `bun run dev`（Remotion Studio，不自動開啟 browser）；`bun run build` 可驗證並產生 Remotion bundle。Browser visual QA 仍需當次明確授權，不能以靜態檢查取代。影片與頁面變更應透過同一個 PR review，merge 後才由 Pages workflow 部署。

---

## 12. 定期維護

在下列事件後重新查證本文件：

- GitHub Actions workflow 改名或調整 trigger／job。
- Tauri、updater plugin 或 tauri-action 升級。
- Stable／Beta version classification、GitHub prerelease policy 或 publish flags 變更。
- Installer targets 或固定檔名變更。
- Signing key custody、GitHub Environment 或 repository ruleset 變更。
- Pages domain、Cloudflare 或下載頁架構變更。
- 發生 Release、updater 或 Pages incident。

每次查證至少核對：

```text
.github/workflows/ci.yml
.github/workflows/release.yml
.github/workflows/deploy-pages.yml
scripts/verify-version-consistency.ts
scripts/release-notes.ts
scripts/verify-updater-release-contract.ts
scripts/finalize-updater-metadata.ts
package.json
src-tauri/Cargo.toml
src-tauri/tauri.conf.json
site/index.html
site/styles.css
site/app.js
site/downloads.js
```

更新本文件時，在頁首更新「最後查證」日期，並在 PR 說明實際驗證過的 workflow、Release 或 Pages 證據，以及仍未由自動化強制的人工 gate。
