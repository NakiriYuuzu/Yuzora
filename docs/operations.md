# Yuzora 官網部署與運維

> 適用範圍：官網（GitHub Pages）的建置、部署與素材重製。App 的打包、簽章、發布與更新見 [`docs/beta2/release.md`](beta2/release.md)；搜尋收錄見 [seo.md](seo.md)。
> 最後更新：2026-11-13（Tauri 版移除，官網的測試與部署改由 `site.yml` 負責）。新的 workflow 尚未在 GitHub 上實際執行過。
> Repository：[`NakiriYuuzu/Yuzora`](https://github.com/NakiriYuuzu/Yuzora)。

這份手冊原本也涵蓋 Tauri 版（v0.1.0-beta.1 以前）的 CI、Release、updater、原生 Runtime 與 Companion。Tauri 版已從 repo 移除，那些章節一併拿掉；需要時讀 git 歷史（`git show v0.1.2:docs/operations.md`）。

本文件不得保存 production private key、production password、token、憑證內容或離線備份位置；敏感資料只存放於核准的 secret store。

## 1. GitHub Actions workflows

| Workflow | 觸發 | 作用 |
| --- | --- | --- |
| `app-ci.yml` | 改到 `app/**` 等路徑的 PR 與 push | 建置與測試 App，固定狀態名稱 `App (build · test)` |
| `app-package.yml` | 改到打包相關路徑的 PR 與 push、手動 dispatch | 四平台打包並安裝檢查，固定狀態名稱 `App (package)`；不建 tag、不發布、不讀 secret |
| `app-release.yml` | 手動 dispatch | 分階段發布 App；只有 `sign` job 讀 signing secret |
| `site.yml` | 改到官網相關路徑的 PR 與 push 到 `main`、手動 dispatch | `Site (test)`；push 到 `main` 或從 `main` 手動 dispatch 時再由 `Deploy Pages` job 部署 |

`app-release.yml` 只能手動 dispatch，其餘三個都有路徑條件：沒有任何一個 check 會在每個 PR 上出現。

## 2. GitHub Pages

### 來源與觸發

- 沿用 GitHub Actions 部署，Pages source 維持 `build_type=workflow`，不建立 `gh-pages` 分支。
- Deploy artifact 是完整 `site/` 目錄：`index.html`、`styles.css`、`app.js`、`i18n.js`、`downloads.js`、`assets/`、`demo/`，以及建置時產生的 `en/` 與 `sitemap.xml`。
- `site.yml` 的 `Site (test)` 依序執行 `bun install --frozen-lockfile --ignore-scripts`、`bun run lint`、`bun run site:seo` 與 `bun run test`，使用 Bun `1.4.2`。`lint` 是 eslint 的 recommended 規則，範圍是 `site/*.js`、`scripts/*.mjs` 與 `tests/*.js`（`eslint.config.js`）。觸發路徑是 `site/**`、`tests/*.js`、`scripts/generate-site-pages.mjs`、`site-remotion/package.json`、`eslint.config.js`、根目錄兩份 CHANGELOG、`package.json`、`bun.lock` 與 workflow 本身。
- push 到 `main`，或從 Actions 在 `main` 上手動 dispatch `Site` 時，`Site (test)` 把它剛建置並測過的 `site/` 上傳成 Pages artifact，再由 `Deploy Pages` job 部署。只改 `app/` 的 push 不會重新部署官網。
- 這個 gate 只驗證測試通過，仍不取代瀏覽器 smoke test。
- 官網保持靜態 ES module，不得在發布頁面引用 `node_modules` runtime path。
- `site/demo/` 是 Tauri 版 Demo 的最後一次建置（由 tag `v0.1.2` 的原始碼以 `demo:build` 產生），以靜態檔提交。它的原始碼不在 repo 裡，不能在這裡重建。官網角色的 markup（`site/index.html` 裡 `companions:start` 與 `companions:end` 之間）與 `assets/brand/companions.css` 也是最後一次產生的結果。
- `site-remotion/` 是影片原始碼，不包含在 Pages artifact。

### 產品頁維護邊界

- 產品頁是靜態 HTML／CSS／ES module；`site:seo` 產生可獨立索引的中英文頁面；`app.js` 負責語言連結、light/dark theme、section reveal、active navigation、影片 viewport lifecycle、GitHub star badge 與 command palette，平台下載仍由 `downloads.js` 負責。Demo 入口沿用目前語言與主題。
- Demo 使用範例資料與記憶體 transport，範例終端機、檔案、Git 與 SQL 不連接真實 host。
- 中文首頁與 `/en/` 各有 canonical、hreflang、社群預覽及 SoftwareApplication JSON-LD。語言切換使用真實連結，不以 localStorage 覆寫網址的語言。新增 markup key 時，`site/i18n.js` 的 `zh-Hant` 與 `en` dictionaries 必須同時提供，中文 HTML 也須同步；`site:seo` 在建置時轉換英文 still、video、poster 與可及性標籤。詳見 [SEO 維護與搜尋收錄](seo.md)。
- Theme 遵循系統偏好並保存至 `yuzora-theme`；no-JS、mobile 與 `prefers-reduced-motion` 必須保持內容可讀，不得依賴動畫才能看見主要資訊。
- Hero、三段 feature media、ADE/HERDR boundary、bento 功能矩陣與 download section 是現行資訊架構；已移除的 Exploded View、Agent Inspector still 與 model showcase 不得重新被 Pages 引用。
- `#primary-download`、`#download-device-note`、platform rows 與 recommended badges 是 `downloads.js` 的穩定 contract。
- 現行無 GSAP 或其他 Pages runtime dependency；不要以 smooth-scroll library 取代 native scrolling。
- 產品頁的影片、截圖與互動 Demo 仍是 Tauri 版的畫面。介紹新 App 的產品頁在 draft PR [#209](https://github.com/NakiriYuuzu/Yuzora/pull/209)（影片與 poster 改成靜態截圖、移除互動 Demo），尚未合併；合併時一併刪除 `site/demo/`。

### 產品頁 smoke test

- 中英文 canonical URL 可開啟，HTTP 正確導向 HTTPS；`/Yuzora/sitemap.xml` 只列出兩個正式語言網址。
- Hero、三步工作流、三段 feature media、ADE/HERDR boundary、bento 功能矩陣與 download section 可讀。
- 中文／英文切換後，全部 still、poster、video source、placeholder、meta content 與 accessibility labels 正確。
- Light/dark theme 初始值、手動切換與 persistence 正確；mobile 沒有水平捲軸，no-JS 與 reduced motion 不會隱藏主要內容。
- Feature videos 進入 viewport 時播放、離開時 pause；分頁離開後不應持續播放或消耗資源。
- 裝置偵測只推薦支援的平台與架構，主要下載 CTA 指向固定檔名 Release assets。
- 未支援的 mobile、ChromeOS、Linux、ARM／32-bit Windows 不會收到錯誤的桌面下載推薦。
- `/Yuzora/demo/` 與相對 assets 可載入；官網的語言／主題會帶入 Demo，範例 terminal、editor、Git、database 與 appearance 可操作，重新整理恢復範例資料。

截至 2026-09-10，GitHub Pages API 回報 `build_type=workflow`、頁面 URL 為 `http://github.yuuzu.net/Yuzora/`、`https_enforced=false`。外層 Cloudflare 的 HTTPS 導向沿用先前設定，當日未重新驗證 DNS／規則。DNS、Cloudflare 規則、canonical URL 與監控方式應由 maintainer 另行保管；Cloudflare challenge 可能讓單純的無瀏覽器 `curl` smoke test 回傳 403，不能直接等同於頁面部署失敗。

### 功能影片與 still 重製

原始碼位於 `site-remotion/`；render commands 與 composition 規則見 `site-remotion/README.md`，錄製流程見 `site-remotion/RECORDING.md`，實際 media naming 以 `site-remotion/src/Root.tsx`、`site/app.js` 與 `tests/site-page.test.js` 為準。

README 圖片與動畫的素材錄自 `site/demo/`，與 Pages 共用 Remotion compositions。更新錄製後，在 repo root 依序執行 `bun scripts/render-site-media.ts` 與 `bun scripts/render-readme-media.ts`，重新產生六張 README PNG、兩個 21 秒 GIF，並同步 Pages posters。抽查中英文的終端機、Git diff 與 SQL 結果畫面，確認圖片沒有落在對話框或操作過渡幀。

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

ADE/HERDR runtime、remote database 與 terminal/git poster stills 必須使用同語言的真實 Remotion frame；Agent Inspector still 已移除。預覽在 `site-remotion/` 用 `bun run dev`（Remotion Studio，不自動開啟 browser）；`bun run build` 可驗證並產生 Remotion bundle。Browser visual QA 仍需當次明確授權，不能以靜態檢查取代。影片與頁面變更應透過同一個 PR review，merge 後才由 `site.yml` 部署。

## 3. 定期維護

在下列事件後重新查證本文件：

- GitHub Actions workflow 改名或調整 trigger／job。
- Installer targets 或固定檔名變更。
- GitHub Environment、branch protection 或 repository ruleset 變更。
- Pages domain、Cloudflare 或下載頁架構變更。
- 發生 Pages incident。

每次查證至少核對：

```text
.github/workflows/site.yml
scripts/generate-site-pages.mjs
eslint.config.js
package.json
site/index.html
site/styles.css
site/app.js
site/downloads.js
tests/site-page.test.js
tests/site-seo.test.js
tests/site-downloads.test.js
```

更新本文件時，在頁首更新「最後更新」日期，並在 PR 說明實際驗證過的 workflow 或 Pages 證據，以及仍未由自動化強制的人工 gate。
