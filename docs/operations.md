# Yuzora 部署與發布運維手冊

> 適用範圍：CI、GitHub Release、Tauri updater、GitHub Pages，以及相關失敗處理。
> 最後查證：2026-07-19。
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
- 實作在獨立 branch 完成，透過 PR 進入 `main`。
- 不直接在 `main` 實作或製作 release commit。
- Release 的版本、Changelog、lockfile 與 workflow contract 變更也必須經過 PR。
- Tag 只能指向已合併、且該 exact commit 的 required CI 全部成功的 `main` commit。
- 不得在同一個 push 同時推送尚未驗證的 `main` commit 與 release tag。
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

| Workflow | 檔案 | 觸發 | 職責 |
|---|---|---|---|
| CI | `.github/workflows/ci.yml` | push 至 `main`；pull request | Frontend lint、typecheck、test、build；三平台 Rust compile；macOS fmt、exact clippy baseline、Rust tests；Linux 真實資料庫 integration |
| Release | `.github/workflows/release.yml` | push tag `v*` | Release guard、三平台建置、updater artifact signing、draft Release、固定檔名別名、`latest.json` finalization |
| Pages | `.github/workflows/deploy-pages.yml` | `main` 上 `site/**` 變更；手動 dispatch | 將 `site/` 部署到 GitHub Pages |

Release 與 Pages 的 workflow trigger 互相獨立，但產品頁下載連結使用 `releases/latest/download/...`：發布新的 Latest Release 會立即改變產品頁實際下載內容，即使 Pages 沒有重新部署。

### CI 重要特性

- Frontend 使用 Bun 與 `@typescript/native` typecheck。
- Rust 在 macOS、Windows x86-64、Linux x86-64 執行 `cargo check --locked --all-targets`。
- Clippy 採 exact baseline；warning 新增、消失、搬移或文字改變都會使 CI 失敗。
- Database integration 在 Linux 使用 Docker 啟動 SQLite、PostgreSQL 與 MSSQL fixture。
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
- Updater signing、stable endpoint、draft Release、MSI-only Windows OTA 與 metadata finalizer contract 完整。

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

### Merge 前

- PR diff 不含無關修改。
- Acceptance criteria 有對應測試或人工證據。
- Required CI checks 全部成功。
- Review conversation 全部處理完成。
- Release／updater 敏感檔案已有合適 reviewer。
- Merge 後再次確認 `main` 上該 exact commit 的 CI，而不是只依賴較早 commit 的綠燈。

---

## 6. 建立 Release tag

只有版本 PR 已 merge，且準備加 tag 的 exact `origin/main` commit 已通過完整 CI 後才能建立 tag。不要憑肉眼看到較早的綠燈，就重新讀取一個可能已前進的 `origin/main` 並直接發布。

以下流程先以 fail-fast 模式抓取遠端、鎖定 immutable SHA，再向 GitHub 查證該 SHA 的 `main` push CI 已完成且成功：

```bash
set -euo pipefail

VERSION=X.Y.Z
REPOSITORY=NakiriYuuzu/Yuzora

if [[ ! "${VERSION}" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]]; then
  echo "VERSION must be a stable SemVer such as 0.0.3" >&2
  exit 1
fi

git fetch --prune --tags origin
VERIFIED_SHA="$(git rev-parse origin/main)"

CI_SUCCESS_COUNT="$(
  gh run list \
    --repo "${REPOSITORY}" \
    --workflow CI \
    --commit "${VERIFIED_SHA}" \
    --limit 20 \
    --json headSha,event,status,conclusion \
  | jq --arg sha "${VERIFIED_SHA}" '
      [.[] | select(
        .headSha == $sha
        and .event == "push"
        and .status == "completed"
        and .conclusion == "success"
      )]
      | length
    '
)"

if [ "${CI_SUCCESS_COUNT}" -lt 1 ]; then
  echo "no successful main CI run found for ${VERIFIED_SHA}" >&2
  exit 1
fi

PACKAGE_VERSION="$(git show "${VERIFIED_SHA}:package.json" | jq -er '.version')"
TAURI_VERSION="$(git show "${VERIFIED_SHA}:src-tauri/tauri.conf.json" | jq -er '.version')"
CARGO_VERSION="$(
  git show "${VERIFIED_SHA}:src-tauri/Cargo.toml" \
  | awk '
      $0 == "[package]" { in_package = 1; next }
      in_package && /^\[/ { in_package = 0 }
      in_package && /^version = "/ {
        value = $0
        sub(/^version = "/, "", value)
        sub(/"$/, "", value)
        version = value
      }
      END {
        if (version == "") exit 1
        print version
      }
    '
)"
CHANGELOG_TEXT="$(git show "${VERIFIED_SHA}:CHANGELOG.md")"

if [ "${PACKAGE_VERSION}" != "${VERSION}" ] \
  || [ "${TAURI_VERSION}" != "${VERSION}" ] \
  || [ "${CARGO_VERSION}" != "${VERSION}" ]; then
  printf 'version mismatch: requested=%s package=%s tauri=%s cargo=%s\n' \
    "${VERSION}" "${PACKAGE_VERSION}" "${TAURI_VERSION}" "${CARGO_VERSION}" >&2
  exit 1
fi

RELEASE_NOTES="$(
  awk -v heading="## [${VERSION}]" '
    index($0, heading) == 1 { in_section = 1; next }
    in_section && /^## \[/ { exit }
    in_section { print }
  ' <<<"${CHANGELOG_TEXT}"
)"
if ! grep -q '[^[:space:]]' <<<"${RELEASE_NOTES}"; then
  echo "CHANGELOG.md has no non-empty section for ${VERSION}" >&2
  exit 1
fi

if git rev-parse -q --verify "refs/tags/v${VERSION}" >/dev/null; then
  echo "tag v${VERSION} already exists" >&2
  exit 1
fi

if gh release view "v${VERSION}" --repo "${REPOSITORY}" >/dev/null 2>&1; then
  echo "Release v${VERSION} already exists" >&2
  exit 1
fi

REMOTE_MAIN_SHA="$(
  gh api "repos/${REPOSITORY}/git/ref/heads/main" --jq '.object.sha'
)"
if [ "${REMOTE_MAIN_SHA}" != "${VERIFIED_SHA}" ]; then
  echo "main advanced to ${REMOTE_MAIN_SHA}; restart release verification" >&2
  exit 1
fi

git tag -a "v${VERSION}" "${VERIFIED_SHA}" -m "Yuzora v${VERSION}"
git push origin "v${VERSION}"
```

執行前必須由操作者確認：

- `VERIFIED_SHA` 是預定發布的 exact `main` commit，而不是另一個 branch 的成功 run。
- 該 commit 內三份版本與 `CHANGELOG.md` 正確。
- GitHub Actions signing secret 名稱存在。
- 本機沒有把其他 branch 或未驗證 commit 一併 push。

流程會在建立 local tag 前再次查詢 GitHub 的 `main` SHA；若已前進就中止並要求重跑。遠端 branch 與 tag 建立之間沒有原子 compare-and-create，因此若 `main` 在最後檢查後才前進，tag 仍只會指向已通過 CI 的 immutable `VERIFIED_SHA`，不會靜默改發新 tip；maintainer 再判斷新 commit 是否必須納入本次 Release。Tag push 是唯一 Release workflow 觸發點，不要使用同一個 push 同時推送 `main` 與 tag。

---

## 7. Release workflow 階段

### 7.1 Guard

在任何平台建置前驗證：

1. `TAURI_SIGNING_PRIVATE_KEY` 與 password secret 非空。
2. Tag、`package.json`、`tauri.conf.json`、`Cargo.toml` version 一致。
3. `CHANGELOG.md` 有該版本 release notes。
4. Updater release contract 完整。

Guard 失敗時所有 build 都不會執行。

### 7.2 三平台建置

`fail-fast: false`，單一平台失敗不會中止其他平台：

- macOS universal：Apple Silicon＋Intel；產生 `.dmg` 與 updater app archive／signature。
- Windows x64：產生 NSIS `.exe`、`.msi` 與 updater signatures。
- Linux x86-64：產生 `.AppImage`、`.deb`、`.rpm` 與 updater signatures。

所有平台上傳至同一個 **draft** Release，名稱為 `Yuzora vX.Y.Z`。

### 7.3 固定檔名別名

供產品頁 `releases/latest/download/...` 使用：

| 平台 | 固定檔名 |
|---|---|
| macOS | `Yuzora-macos-universal.dmg` |
| Windows | `Yuzora-windows-x64-setup.exe`、`Yuzora-windows-x64.msi` |
| Linux | `Yuzora-linux-x86_64.AppImage`、`Yuzora-linux-amd64.deb`、`Yuzora-linux-x86_64.rpm` |

固定檔名如有變更，必須在同一個 PR 更新所有實際 consumer：

- 六個 alias 都要同步 `.github/workflows/release.yml` 與本文件。
- 產品頁直接使用的 macOS DMG、Windows NSIS EXE、Linux AppImage 三個主要 alias，還要同步 `site/index.html`、`site/downloads.js` 與 `tests/site-downloads.test.js`。
- MSI、DEB、RPM 若新增其他頁面或 script consumer，也要一併更新並補測試。

固定別名是手動下載入口；Tauri updater 使用的是具版本號且帶 `.sig` 的 updater artifacts，不應把兩者混為同一套檔案。

### 7.4 Finalize updater metadata

所有 build 成功後，`finalize-updater-metadata` job：

- 下載 draft 中的 `latest.json`。
- 驗證 metadata version 與 notes。
- 移除 Windows NSIS updater entries。
- 強制 Windows updater URL 指向 MSI。
- 驗證每個 metadata artifact 與 `.sig` 都存在於 Release。
- 以 finalized `latest.json` 覆蓋 draft 中的原始檔案。

Finalizer 未成功時不得 Publish。

---

## 8. Draft Release 人工驗收

### Workflow 與內容

- `Tag / version consistency` 成功。
- 三個平台 build 全部成功。
- `Finalize updater metadata` 成功。
- Release 維持 draft，未被意外標為 prerelease。
- Release body 與 `CHANGELOG.md` 對應版本一致且非空。

### Assets

- 六個固定檔名別名齊全。
- 三平台 versioned installers 齊全。
- Updater artifacts 對應的 `.sig` 齊全。
- `latest.json` 存在。
- Updater metadata 至少包含 `darwin-aarch64`、`darwin-x86_64`、`linux-x86_64`、`windows-x86_64` 四個支援目標。
- Windows 仍保留 NSIS／MSI 手動安裝檔，但 OTA metadata 只使用 MSI。

可先列出 draft：

```bash
VERSION=X.Y.Z
gh release view "v${VERSION}" \
  --repo NakiriYuuzu/Yuzora \
  --json tagName,name,isDraft,isPrerelease,body,assets \
  --jq '{tagName,name,isDraft,isPrerelease,body,assets:[.assets[].name]}'
```

如需檢查 draft `latest.json`，下載至臨時目錄後確認：

```bash
set -euo pipefail

VERSION=X.Y.Z
TMP_DIR="$(mktemp -d)"
gh release download "v${VERSION}" \
  --repo NakiriYuuzu/Yuzora \
  --pattern latest.json \
  --dir "${TMP_DIR}"

jq '{version,notes,platforms:(.platforms|keys)}' "${TMP_DIR}/latest.json"

for key in darwin-aarch64 darwin-x86_64 linux-x86_64 windows-x86_64; do
  if ! jq -e --arg key "${key}" '
    .platforms[$key] != null
    and (.platforms[$key].url | type == "string" and length > 0)
    and (.platforms[$key].signature | type == "string" and length > 0)
  ' "${TMP_DIR}/latest.json" >/dev/null; then
    echo "latest.json is missing a valid ${key} updater entry" >&2
    exit 1
  fi
done
```

目前 finalizer 會驗證 metadata 中已存在的 entries，並強制 `windows-x86_64` 存在；它尚未強制 macOS／Linux platform completeness。因此上述四個 key 是 Publish 前的人工 blocking gate，直到 script contract 在另案中補強為止。

檢查完成後移除臨時目錄。不得把下載的 signing metadata 或其他暫存檔提交進 repository。

### 安裝抽驗

Publish 前至少確認：

- macOS DMG 可掛載並安裝。
- Windows NSIS 與 MSI 可啟動安裝；OTA 預期路徑以 MSI 為準。
- Linux 主要下載格式可啟動。
- 未啟用 OS code signing 的警告已列入 release notes。
- 若本版改動 updater contract，使用隔離測試環境驗證，不以 production 使用者作為首次測試者。

所有條件通過後才由 maintainer 人工 Publish。

---

## 9. Publish 後驗證

Publish 後 `releases/latest` 會立即指向新版本，產品頁固定下載連結與 App updater endpoint 同時開始對外生效。

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

| 狀況 | 處理原則 |
|---|---|
| Guard 失敗 | 不進行 build。修正 release PR／版本／Changelog／secret contract 後重新走完整流程。只有尚未 Publish、未被消費且經 maintainer 明確確認的錯誤 tag，才可進行受控清理。 |
| 單一平台失敗 | 其他平台可保留。只 re-run failed job；先檢查 draft 是否已存在該平台固定別名，避免同名 asset 重傳造成 HTTP 422。 |
| Finalizer 失敗 | 不得 Publish。檢查 `latest.json`、Windows MSI metadata、缺少的 artifact 或 `.sig`。 |
| 固定別名缺漏 | 檢查 `Upload fixed-name alias assets` log、artifact path pattern 與 draft 中是否已有同名 asset。 |
| Publish 前反悔 | Draft 不會成為 `releases/latest`。由 maintainer 決定保留供調查或受控移除；同時處理已建立但不再使用的 tag。 |
| 已 Publish 後發現問題 | 不覆寫 artifacts、不重用 tag/version。先建立 incident Issue、評估是否暫時隱藏錯誤 Release，再發布新的 patch version 作為永久修正。 |
| Updater key 疑似外洩 | 立即停止 Release、限制 secret 存取並啟動供應鏈安全事件。不得直接換 public key；既有安裝只信任內嵌 key，輪替需要獨立遷移設計。 |
| Updater key 遺失 | 從核准的加密備份復原並稽核存取。若無可用 private key，既有安裝的信任鏈可能無法延續，必須升級為 release incident。 |
| Pages 部署失敗 | 確認 Pages source 為 GitHub Actions、artifact path 只指向 `site/`，並檢查 deploy job log。 |
| Pages 發布錯誤內容 | 透過正常 PR 回復 `site/**` 至已知正常版本，再讓 Pages workflow 重新部署；不直接改寫遠端 branch 歷史。 |

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
