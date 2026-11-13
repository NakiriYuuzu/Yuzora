# Yuzora 官網部署與運維

> 適用範圍：官網（GitHub Pages）的建置、部署與素材重製。App 的打包、簽章、發布與更新見 [`docs/beta2/release.md`](beta2/release.md)；搜尋收錄見 [seo.md](seo.md)。
> 最後更新：2026-11-13（Tauri 版移除，官網的測試與部署改由 `site.yml` 負責；產品頁改成介紹新 App 的靜態截圖，不再有影片與互動 Demo）。`Deploy Pages` job 尚未在 `main` 上實際執行過。
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
- Deploy artifact 是完整 `site/` 目錄：`index.html`、`styles.css`、`app.js`、`i18n.js`、`downloads.js`、`assets/`，以及建置時產生的 `en/` 與 `sitemap.xml`。
- `site.yml` 的 `Site (test)` 依序執行 `bun install --frozen-lockfile --ignore-scripts`、`bun run lint`、`bun run site:seo` 與 `bun run test`，使用 Bun `1.4.2`。`lint` 是 eslint 的 recommended 規則，範圍是 `site/*.js`、`scripts/*.mjs` 與 `tests/*.js`（`eslint.config.js`）。觸發路徑是 `site/**`、`tests/*.js`、`scripts/generate-site-pages.mjs`、`eslint.config.js`、根目錄兩份 CHANGELOG、`package.json`、`bun.lock` 與 workflow 本身。
- push 到 `main`，或從 Actions 在 `main` 上手動 dispatch `Site` 時，`Site (test)` 把它剛建置並測過的 `site/` 上傳成 Pages artifact，再由 `Deploy Pages` job 部署。只改 `app/` 的 push 不會重新部署官網。
- 這個 gate 只驗證 lint 與測試通過，仍不取代瀏覽器 smoke test。
- 官網保持靜態 ES module，不得在發布頁面引用 `node_modules` runtime path。

### 產品頁維護邊界

- 產品頁是靜態 HTML／CSS／ES module；`site:seo` 產生可獨立索引的中英文頁面；`app.js` 負責語言連結、light/dark theme、section reveal、active navigation、GitHub star badge 與 command palette，平台下載仍由 `downloads.js` 負責。
- 中文首頁與 `/en/` 各有 canonical、hreflang、社群預覽及 SoftwareApplication JSON-LD。語言切換使用真實連結，不以 localStorage 覆寫網址的語言。新增 markup key 時，`site/i18n.js` 的 `zh-Hant` 與 `en` dictionaries 必須同時提供，中文 HTML 也須同步；`site:seo` 在建置時轉換英文截圖與可及性標籤。詳見 [SEO 維護與搜尋收錄](seo.md)。
- Theme 遵循系統偏好並保存至 `yuzora-theme`；no-JS、mobile 與 `prefers-reduced-motion` 必須保持內容可讀，不得依賴動畫才能看見主要資訊。
- Hero、四段 feature media、ADE/HERDR boundary、bento 功能矩陣與 download section 是現行資訊架構；已移除的影片、互動 Demo、Exploded View、Agent Inspector still 與 model showcase 不得重新被 Pages 引用（`tests/site-page.test.js` 會擋 `<video>` 與 Demo 連結）。
- `#primary-download`、`#download-device-note`、platform rows 與 recommended badges 是 `downloads.js` 的穩定 contract。
- `index.html` 載入 `app.js` 的 `?v=` 隨檔案更新：這些檔案會被快取數小時，舊的 `downloads.js` 會指向新版 release 沒有的檔名。
- 現行無 GSAP 或其他 Pages runtime dependency；不要以 smooth-scroll library 取代 native scrolling。

### 產品頁 smoke test

- 中英文 canonical URL 可開啟，HTTP 正確導向 HTTPS；`/Yuzora/sitemap.xml` 只列出兩個正式語言網址。
- Hero、三步工作流、四段 feature media、ADE/HERDR boundary、bento 功能矩陣與 download section 可讀。
- 中文／英文切換後，全部截圖、placeholder、meta content 與 accessibility labels 正確。
- Light/dark theme 初始值、手動切換與 persistence 正確；mobile 沒有水平捲軸，no-JS 與 reduced motion 不會隱藏主要內容。
- 裝置偵測只推薦支援的平台與架構，主要下載 CTA 指向固定檔名 Release assets。
- 未支援的 mobile、ChromeOS、Linux、ARM／32-bit Windows 不會收到錯誤的桌面下載推薦。

截至 2026-09-10，GitHub Pages API 回報 `build_type=workflow`、頁面 URL 為 `http://github.yuuzu.net/Yuzora/`、`https_enforced=false`。外層 Cloudflare 的 HTTPS 導向沿用先前設定，當日未重新驗證 DNS／規則。DNS、Cloudflare 規則、canonical URL 與監控方式應由 maintainer 另行保管；Cloudflare challenge 可能讓單純的無瀏覽器 `curl` smoke test 回傳 403，不能直接等同於頁面部署失敗。

### 產品頁截圖

Hero 與四段 feature 都是 App 的靜態截圖，中英文各一張：`site/assets/yuzora-workspace-{zh,en}.png`（Hero，也是社群預覽圖）與 `feature-{spaces-agents,files,database,git}-{zh,en}.png`。尺寸一律 1600×1025，與 `<img>` 的 `width`／`height` 及 `og:image:width`／`height` 相同（`tests/site-page.test.js`、`tests/site-seo.test.js` 會讀 PNG 檔頭比對）。

現有的截圖拍自 2026-11-03 的 beta.2 build（介面當時有中文），早於右側 dock 與之後的版面調整；重拍前，畫面與目前的 App 不完全相同。

重拍時照 AGENTS.md 的隔離規則執行 `herdr-app`，不得碰到真實的 app home、herdr-core session 或 repo：

1. `env -i` 只帶 `HOME`、`USER`、`TMPDIR`、`TERM`、`PATH` 與 `LANG`，另設暫存的 `YUZORA_APP_HOME`、`HC_SESSION`、`XDG_STATE_HOME`／`XDG_CONFIG_HOME`／`XDG_RUNTIME_DIR`，以及 `HERDR_APP_NO_SOUND=1`、`YUZORA_DB_SECRETS=memory`。
2. 素材全部放在暫存目錄：一個有分支、tag、merge 與已暫存／未暫存／未追蹤變更的 Git repo，一個 SQLite 資料庫，再加兩個空 repo 當其他 Space。作者、email 與資料都用虛構內容。
3. 啟動前在 app home 寫入 `shared.json` 的 `"frame.welcomed": "skipped"`，以及 `yuzora/settings.json` 的 `git.diffMode: "unified"`（窄 pane 的並排 diff 只看得到舊版）與 SQLite 連線。
4. 新 home 第一次啟動會出現「已更新到新版」提示：先 `do space.new` 建好三個 Space（argv 用假 agent，`printf '\033]7501;state=<working|blocked|done>:app=<claude|codex|pi>\033\\'` 後 `sleep`），`do app.quit` 後再啟動一次。
5. 側欄會顯示本機 hostname。第二次啟動後，只在這個隔離 home 的 `mods/yuzora-sidebar-workspace/main.js` 把 `this.host = hostname();` 換成固定名稱，等熱載入後再拍。檔案樹的資料夾要點開才會展開；Files 截圖同樣只在隔離 home 的 `mods/yuzora-sidebar-files/main.js` 把 `this.open` 的初始值設成 `["src"]`。
6. 用 `do space.select`、`do pane.split --mod <mod> --props …`、`do layout.select` 排出畫面，等幾秒讓資料載完後 `shot <png>`。
7. 拍完 `do app.quit`，並對該隔離 `HC_SESSION` 執行 `hc server stop`。
8. `shot` 輸出 2560×1640，以 `sips -Z 1600` 縮成 1600×1025 後覆寫 `site/assets/`。逐張檢查：沒有提示 toast、沒有真實 hostname、使用者名稱或路徑。

App 的介面從 beta.3 起只有英文，中英文頁要不要各放一張由維護者決定。改動畫面內容時同步 `site/index.html` 的 `alt` 與 `site/i18n.js` 的 `media.*`／`hero.alt`。Browser visual QA 仍需當次明確授權，不能以靜態檢查取代。

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
