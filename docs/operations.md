# Yuzora 部署與發布運維手冊

> 適用範圍：CI、GitHub Release、Tauri updater、GitHub Pages，以及相關失敗處理。
> 最後查證：2026-07-24。
> Repository：[`NakiriYuuzu/Yuzora`](https://github.com/NakiriYuuzu/Yuzora)。

本文件不得保存 private key、密碼、token、憑證內容或離線備份位置。敏感資料只存放於核准的 secret store。

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

> 查證狀態：2026-07-19 GitHub API 回報 `main` 尚未啟用 branch protection，repository rulesets 亦為空。在設定完成前，以上規則只能靠維護者人工遵守，不能視為已由平台強制。

---

## 3. 三條 GitHub Actions workflow

| Workflow | 檔案                                 | 觸發                                    | 職責                                                                                                                                                                    |
| -------- | ------------------------------------ | --------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| CI       | `.github/workflows/ci.yml`           | push 至 `main`；pull request            | Frontend lint、typecheck、test、build；三平台 Rust compile；macOS fmt、exact clippy baseline、Rust tests；Linux 真實資料庫 integration；`release/*` PR 三平台候選安裝檔 |
| Release  | `.github/workflows/release.yml`      | `CI` workflow 完成                      | 只接受成功的 `main` push CI；自動建立 tag、三平台建置、updater artifact signing、暫態 draft、固定檔名別名、`latest.json` finalization 與自動 Publish                    |
| Pages    | `.github/workflows/deploy-pages.yml` | `main` 上 `site/**` 變更；手動 dispatch | 將 `site/` 部署到 GitHub Pages                                                                                                                                          |

Release 與 Pages 的 workflow trigger 互相獨立，但產品頁下載連結使用 `releases/latest/download/...`：發布新的 Latest Release 會立即改變產品頁實際下載內容，即使 Pages 沒有重新部署。

### CI 重要特性

- Frontend 使用 Bun 與 `@typescript/native` typecheck。
- Rust 在 macOS、Windows x86-64、Linux x86-64 執行 `cargo check --locked --all-targets`。
- Clippy 採 exact baseline；warning 新增、消失、搬移或文字改變都會使 CI 失敗。
- Database integration 在 Linux 使用 Docker 啟動 SQLite、PostgreSQL 與 MSSQL fixture。
- `release/*` PR 額外建置未發布的 macOS／Windows／Linux candidate installers，僅上傳為保留 14 天的 Actions artifacts，供使用者在 merge 前驗證。
- 同一 ref 上被新 commit 取代的 CI run 會由 concurrency 設定取消。

---

## 4. Release 安全邊界

Yuzora 有兩種不同的簽章邊界，不得混為一談。

### 已啟用：Tauri updater artifact signing

Release workflow 必須取得：

- `TAURI_SIGNING_PRIVATE_KEY`
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`

Public key 內嵌於 `src-tauri/tauri.conf.json`。Private key 與密碼由 GitHub Actions secrets 保管，並依本機權威文件 `.yuuzu/adr/0003-updater-signing-key-custody.html` 保留 repository 外加密備份；該 ADR 不屬於公開 runbook，也不得複製其中的敏感保管細節。

任何可存取上述 secrets，或會影響下列檔案的修改，均屬供應鏈安全敏感變更：

- `.github/workflows/release.yml`
- `scripts/verify-updater-release-contract.ts`
- `scripts/finalize-updater-metadata.ts`
- `src-tauri/tauri.conf.json` 的 updater public key／endpoint

這些變更應由指定 maintainer review。建議後續將 signing secrets 放入具 required reviewer 的 protected GitHub Environment；在完成前不得宣稱此 gate 已啟用。

### 尚未啟用：作業系統平台簽章

- macOS code signing／notarization 尚未啟用。
- Windows Authenticode code signing 尚未啟用。
- Updater artifact signature 不會消除 macOS Gatekeeper 或 Windows SmartScreen 警告。

---

## 5. Release PR

每次發版先建立 release Issue 或在既有 release Issue 更新 acceptance criteria，再建立 `release/vX.Y.Z` branch 與 PR。

### PR 必須包含

- `package.json` version。
- `src-tauri/tauri.conf.json` version。
- `src-tauri/Cargo.toml` version。
- 更新後的 `src-tauri/Cargo.lock`。
- `CHANGELOG.md` 中對應的 `## [X.Y.Z]` 使用者可讀章節。
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

在乾淨的 release branch 執行：

```bash
VERSION=X.Y.Z
GITHUB_REF_NAME="v${VERSION}" bun run check:version
bun scripts/release-notes.ts "v${VERSION}"
bun run check:updater-release
```

三項都必須成功：

- 三份 product version 與 tag contract 一致。
- `CHANGELOG.md` 存在對應版本且內容非空。
- Updater signing、stable endpoint、PR merge 後自動 tag／Publish、暫態 draft、MSI-only Windows OTA 與 metadata finalizer contract 完整。

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

`release/vX.Y.Z` PR 的 CI 會執行三個 `Release candidate (...)` jobs。候選檔有以下限制：

- 只存在 GitHub Actions artifacts，不建立或更新 tag。
- 不建立 GitHub Release，也不會成為 `releases/latest`。
- 關閉 updater artifact 產生與 signing，只用於 merge 前的互動式功能驗證。
- 未啟用 OS code signing，Windows SmartScreen 與 macOS Gatekeeper 仍可能警告。

從 PR 的 CI run 下載 Windows 候選檔：

```bash
RUN_ID=<release-pr-ci-run-id>

gh run download "${RUN_ID}" \
  --repo NakiriYuuzu/Yuzora \
  --name yuzora-release-candidate-windows-x86-64
```

需要其他平台時將 artifact name 改為：

- `yuzora-release-candidate-macos-universal`
- `yuzora-release-candidate-linux-x86-64`

使用者至少要在本次受影響平台驗證 acceptance criteria。Windows terminal／IME 版本至少包括：

- Microsoft Pinyin 中文 composition、replacement、commit 不重複也不遺失。
- Command Prompt、Windows PowerShell、PowerShell 7 能依設定與單次選擇啟動。
- WSL default 與已安裝 distro 能啟動，Windows／UNC workspace 的 cwd 轉換正確。
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
- Merge 後由 Release workflow 等待並查證 `main` 上該 exact commit 的 push CI；PR CI 綠燈本身不會直接發布。

---

## 6. PR merge 後自動建立 Release

Release tag 不由本機或 maintainer 手動建立。完整入口是 release PR：

1. Release PR 包含版本、lockfile、Changelog 與必要的 workflow／contract 修改。
2. PR required CI 與 candidate builds 成功後保持開啟，等待使用者下載安裝檔並完成實機驗證。
3. 使用者在 PR 明確回報驗證通過並授權 merge 後，才 merge 至 `main`；merge 同時透過 `Closes` 關閉已完成 Issues。
4. `main` push 觸發完整 CI；Release workflow 透過 `workflow_run` 接收完成事件。
5. Guard 只接受 `event=push`、`head_branch=main`、`conclusion=success`，並 checkout `workflow_run.head_sha`，確保驗證與發布的是同一個 immutable commit。
6. Guard 從該 commit 的 `package.json` 解析 stable version，執行版本、release notes 與 updater contract checks。
7. 若版本 tag 不存在，workflow 建立 annotated `vX.Y.Z` tag 並精確指向該成功 CI SHA；接著開始建置。
8. 若相同版本已 Published，workflow 安全略過，不會因後續一般 PR 重複發布。

這個設計讓 PR 成為唯一 repository 變更入口，同時避免「PR CI 綠燈但尚未進入 `main`」就對外發布。CI、tag、Release 與 Issue 關閉的關係如下：

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

若 upgrade 前已存在同版本的 draft Release，Guard 會進入銜接模式：不重建、不覆寫既有 binary artifacts，但會重新執行 deterministic metadata finalizer 並只覆蓋 `latest.json`；後續 automated publish gate 完整驗證資產與 updater metadata 後才移除 draft 狀態。驗證不完整時 workflow 失敗，Release 保持 draft。

---

## 7. Release workflow 階段

### 7.1 Guard

在任何平台建置前驗證：

1. 上游事件是成功完成的 `main` push CI，而不是 pull request 或其他 branch。
2. Checkout SHA 與成功 CI 的 `workflow_run.head_sha` 完全一致。
3. `TAURI_SIGNING_PRIVATE_KEY` 與 password secret 非空。
4. 解析出的 tag、`package.json`、`tauri.conf.json`、`Cargo.toml` version 一致。
5. `CHANGELOG.md` 有該版本 release notes。
6. Updater release contract 完整。
7. 新版本自動建立 annotated tag；已發布版本則安全略過。

Guard 失敗時所有 build 都不會執行。

### 7.2 三平台建置

`fail-fast: false`，單一平台失敗不會中止其他平台：

- macOS universal：Apple Silicon＋Intel；產生 `.dmg` 與 updater app archive／signature。
- Windows x64：產生 NSIS `.exe`、`.msi` 與 updater signatures。
- Linux x86-64：產生 `.AppImage`、`.deb`、`.rpm` 與 updater signatures。

所有平台上傳至同一個**暫態 draft** Release，名稱為 `Yuzora vX.Y.Z`。Draft 只用來避免 matrix 尚未完成時讓部分資產對外可見，不是人工發版佇列。

### 7.3 固定檔名別名

供產品頁 `releases/latest/download/...` 使用：

| 平台    | 固定檔名                                                                            |
| ------- | ----------------------------------------------------------------------------------- |
| macOS   | `Yuzora-macos-universal.dmg`                                                        |
| Windows | `Yuzora-windows-x64-setup.exe`、`Yuzora-windows-x64.msi`                            |
| Linux   | `Yuzora-linux-x86_64.AppImage`、`Yuzora-linux-amd64.deb`、`Yuzora-linux-x86_64.rpm` |

固定檔名如有變更，必須在同一個 PR 更新所有實際 consumer：

- 六個 alias 都要同步 `.github/workflows/release.yml` 與本文件。
- 產品頁直接使用的 macOS DMG、Windows NSIS EXE、Linux AppImage 三個主要 alias，還要同步 `site/index.html`、`site/downloads.js` 與 `tests/site-downloads.test.js`。
- MSI、DEB、RPM 若新增其他頁面或 script consumer，也要一併更新並補測試。

固定別名是手動下載入口；Tauri updater 使用的是具版本號且帶 `.sig` 的 updater artifacts，不應把兩者混為同一套檔案。

### 7.4 Finalize updater metadata

所有 build 成功後，或既有同版本 draft 進入銜接模式時，`finalize-updater-metadata` job：

- 使用 Guard 解析出的 `tag_name` 查找 draft Release；不得依賴 `workflow_run` 的 `GITHUB_REF_NAME`，其值是 `main` 而不是 release tag。
- 下載 draft 中的 `latest.json`。
- 驗證 metadata version 與 notes。
- 移除 Windows NSIS updater entries。
- 強制 Windows updater URL 指向 MSI。
- 驗證每個 metadata artifact 與 `.sig` 都存在於 Release。
- 以 finalized `latest.json` 覆蓋 draft 中的原始檔案。

Finalizer 未成功時不得 Publish。

### 7.5 Automated publish gate

`publish-release` 只在下列其中一條路徑成立時執行：

- 新版本的三平台 build 與 metadata finalizer 全部成功。
- 既有同版本 draft 的銜接模式啟用、build 正確略過，且 metadata finalizer 成功。

Publish 前 workflow 自動驗證：

- Release 仍是 draft、不是 prerelease，且 release body 非空。
- 六個固定檔名別名與 `latest.json` 齊全。
- `latest.json.version` 與 tag 相同，notes 非空。
- `darwin-aarch64`、`darwin-x86_64`、`linux-x86_64`、`windows-x86_64` 都有非空 URL 與 signature。
- 不含 Windows NSIS updater key，且 Windows OTA URL 使用 `.msi`。

全部成功後執行 `gh release edit --draft=false --prerelease=false --latest`，並再次查證 `publishedAt`。任一條件失敗時 workflow 結束為失敗，Release 保持 draft，不會出現部分成功卻永久等待人工 Publish 的正常路徑。

---

## 8. 自動發布與發布後 smoke test

Release workflow 的 automated publish gate 是 blocking gate；三平台 build、固定別名、updater signatures、metadata completeness 或 MSI-only contract 任一失敗都不會 Publish。正常成功路徑不需要 maintainer 再按一次 Publish。

受影響平台的主要互動式驗收已在 release PR merge 前完成。Release Published 後仍應儘快確認正式 artifacts 與 updater 路徑：

- macOS DMG 掛載、安裝與首次啟動。
- Windows NSIS／MSI 安裝；OTA 預期路徑以 MSI 為準。
- Linux 主要格式啟動。
- 從上一個 stable 版本執行 updater smoke test。
- 確認 release notes 已揭露尚未啟用 macOS／Windows OS code signing 的警告。

若人工驗收發現 regression，不覆寫已發布 tag 或 artifacts；立即建立 incident Issue，必要時隱藏受影響 Release，並透過新的 patch release PR 修正。平台驗收結果、Release URL、測試平台與診斷證據回填 release Issue。

---

## 9. Publish 後驗證

Automated publish gate 成功後，`releases/latest` 會立即指向新版本，產品頁固定下載連結與 App updater endpoint 同時開始對外生效。

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
- 至少存在 `darwin-aarch64`、`darwin-x86_64`、`linux-x86_64`、`windows-x86_64`。
- 沒有 `windows-*-nsis` key。
- 所有 Windows updater URLs 指向 `.msi`。
- Metadata 中每個 artifact URL 與 signature 都可下載。

### 固定下載 URL

至少確認以下 URL 回傳成功：

- `Yuzora-macos-universal.dmg`
- `Yuzora-windows-x64-setup.exe`
- `Yuzora-windows-x64.msi`
- `Yuzora-linux-x86_64.AppImage`
- `Yuzora-linux-amd64.deb`
- `Yuzora-linux-x86_64.rpm`

### OTA smoke test

從上一個 stable 版本，在 macOS universal、Windows x64、Linux x86-64 驗證：

1. App 發現新版本。
2. 顯示的 release notes 正確。
3. 下載成功並顯示進度。
4. Signature verification 成功。
5. 安裝與重新啟動成功。
6. Runtime version 顯示新版本。
7. 使用者資料與未儲存文件保護符合預期。

將 Release URL、平台、起始版本、目標版本、結果與診斷證據回填 release Issue。真實 OTA 驗收不得只以 CI artifact 存在代替。

---

## 10. 失敗與復原

| 狀況                        | 處理原則                                                                                                                                                              |
| --------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Candidate／使用者驗證未完成 | Release PR 保持開啟，不建立 tag、不關閉 Issues、不觸發 Release。修正後重新產生 candidate，直到使用者明確核准 merge。                                                  |
| `main` CI 失敗              | Release workflow 不會建立 tag 或建置。以新的修復 PR 讓 `main` 恢復綠燈。                                                                                              |
| Guard 失敗                  | 不進行 build。修正版本／Changelog／secret／workflow contract，且修正本身也必須走 PR。只有尚未 Publish、未被消費且經 maintainer 明確確認的錯誤 tag，才可進行受控清理。 |
| 單一平台失敗                | 其他平台可保留。只 re-run failed job；先檢查 draft 是否已存在該平台固定別名，避免同名 asset 重傳造成 HTTP 422。                                                       |
| Finalizer 失敗              | 不得 Publish。檢查 `latest.json`、Windows MSI metadata、缺少的 artifact 或 `.sig`。                                                                                   |
| 固定別名缺漏                | 檢查 `Upload fixed-name alias assets` log、artifact path pattern 與 draft 中是否已有同名 asset。                                                                      |
| Automated publish gate 失敗 | Release 保持 draft。以 PR 修正 contract 或 workflow；不得手動略過 gate Publish。                                                                                      |
| 發布前需緊急停止            | 在 `Publish verified release` 執行前取消 workflow。Draft 不會成為 `releases/latest`；後續 tag／draft 清理仍需 maintainer 明確授權與事件記錄。                         |
| 已 Publish 後發現問題       | 不覆寫 artifacts、不重用 tag/version。先建立 incident Issue、評估是否暫時隱藏錯誤 Release，再發布新的 patch version 作為永久修正。                                    |
| Updater key 疑似外洩        | 立即停止 Release、限制 secret 存取並啟動供應鏈安全事件。不得直接換 public key；既有安裝只信任內嵌 key，輪替需要獨立遷移設計。                                         |
| Updater key 遺失            | 從核准的加密備份復原並稽核存取。若無可用 private key，既有安裝的信任鏈可能無法延續，必須升級為 release incident。                                                     |
| Pages 部署失敗              | 確認 Pages source 為 GitHub Actions、artifact path 只指向 `site/`，並檢查 deploy job log。                                                                            |
| Pages 發布錯誤內容          | 透過正常 PR 回復 `site/**` 至已知正常版本，再讓 Pages workflow 重新部署；不直接改寫遠端 branch 歷史。                                                                 |

任何 destructive cleanup、tag 刪除、Release 隱藏或 secret rotation 都需要 maintainer 明確授權與事件記錄。

---

## 11. GitHub Pages

### 來源與觸發

- 來源：`site/index.html`、`site/downloads.js`、`site/assets/*.mp4`。
- `main` 上 `site/**` 有變更時自動部署。
- 也可從 Actions 手動 dispatch `Deploy Pages`。
- `site-remotion/` 是影片原始碼，不應包含在 Pages artifact。

### 產品頁 smoke test

- Canonical URL 可開啟，HTTP 會正確導向 HTTPS。
- 中文／英文切換正常。
- 六支影片可載入，語言切換後影片 source 正確。
- 裝置偵測只推薦支援的平台與架構。
- 三個主要下載 CTA 指向固定檔名 Release assets。
- 未支援的 mobile、ChromeOS、ARM／32-bit Windows 或 Linux 不會收到錯誤的 x64 下載推薦。

截至 2026-07-19，GitHub Pages API 回報頁面 URL 為 `http://github.yuuzu.net/Yuzora/`，外層由 Cloudflare 導向 HTTPS。DNS、Cloudflare 規則、canonical URL 與監控方式應由 maintainer 另行保管；Cloudflare challenge 可能讓單純的無瀏覽器 `curl` smoke test 回傳 403，不能直接等同於頁面部署失敗。

### 功能影片重渲染

原始碼位於 `site-remotion/`，視覺規範見 `docs/design/gh-pages/DESIGN.md`。

```bash
cd site-remotion
bun install

for c in agentzone-zh agentzone-en remote-db-zh remote-db-en terminal-git-zh terminal-git-en; do
  bunx remotion render "$c" "../site/assets/$c.mp4" --scale=2
done
```

預覽使用 `bun run dev`。影片與頁面變更應透過同一個 PR review，merge 後才由 Pages workflow 部署。

---

## 12. 定期維護

在下列事件後重新查證本文件：

- GitHub Actions workflow 改名或調整 trigger／job。
- Tauri、updater plugin 或 tauri-action 升級。
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
site/downloads.js
```

更新本文件時，在頁首更新「最後查證」日期，並在 PR 說明實際驗證過的 workflow、Release 或 Pages 證據。
