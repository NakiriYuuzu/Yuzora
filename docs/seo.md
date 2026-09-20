# Yuzora 搜尋收錄與曝光維護

正式網址為 `https://github.yuuzu.net/Yuzora/`（繁體中文）及 `https://github.yuuzu.net/Yuzora/en/`（英文）。官網與 GitHub 介紹聚焦真實用途：macOS／Windows 的 AI coding agent workspace、HERDR terminal、Git、SSH／SFTP、WSL、SQL 資料庫與 HTML 預覽。Agent CLI、帳號與專案 Dev Server 由使用者管理。

## 建置與內容

```sh
bun run site:companions
bun run site:seo
bun run demo:build
bun run test tests/site-page.test.js tests/site-seo.test.js tests/site-downloads.test.js
```

- `site/index.html` 是中文來源；`site/i18n.js` 維護中英文文案。新增或修改中文內容時兩處同步，測試會核對 HTML 與字典。
- `scripts/generate-site-pages.mjs` 更新中文頁的 SEO 區段，產生完整的英文 `site/en/index.html` 與 `site/sitemap.xml`。後兩者為忽略的產物，由 CI 與 Pages workflow 建置。
- 兩語言皆提供 title、description、self canonical、互相指向的 `hreflang`、絕對網址的 Open Graph／Twitter 分享圖片及 SoftwareApplication JSON-LD。中文首頁為 `x-default`。
- 不加未經證實的評分、評論、授權條款或不支援的平台。App 支援 Apple Silicon macOS 與 Windows x64；遠端 Linux／Intel macOS Host 不代表有對應的桌面安裝包。
- Demo 是範例資料操作頁，使用 `noindex, follow`，不放入 sitemap。不要透過 robots 禁止爬取 Demo，否則爬蟲無法讀到 noindex。
- 保留 no-JS 靜態文案、真實語言連結與同頁 anchor；相對資源須在 `/Yuzora/` 和 `/Yuzora/en/` 都可載入。
- 修改正式網域時，同步 generator 的 `siteUrl`、測試、README 及 GitHub homepage。不要只改 `og:url`。

## 網域擁有者的收錄步驟

以下步驟需要擁有者的 Search Console／Bing 帳號與網域驗證權限；repo 建置與部署不會自動完成它們。

1. 在 Google Search Console 新增 `https://github.yuuzu.net/Yuzora/` URL-prefix property，依提供的驗證方式完成所有權驗證；若已有網域 property 可直接使用。
2. 提交 `https://github.yuuzu.net/Yuzora/sitemap.xml`。用 URL Inspection 檢查兩個語言頁並要求建立索引。
3. 在 Bing Webmaster Tools 驗證網站（或匯入已驗證的 Search Console property），提交相同 sitemap。
4. 檢查網域根目錄 `https://github.yuuzu.net/robots.txt` 與 Cloudflare 規則：允許搜尋爬蟲存取官網與必要資源，並可加入 `Sitemap: https://github.yuuzu.net/Yuzora/sitemap.xml`。

`/Yuzora/robots.txt` 不會成為這個網域的爬蟲規則。此 repo 只有專案子路徑的 Pages artifact，因此不產生具有誤導效果的 robots 檔案，也不修改其他網站的網域政策。不使用已停用的搜尋引擎 sitemap ping 端點。

## 發布後驗證與觀測

- 檢查中英文頁與 sitemap 回應 200；canonical、hreflang 和社群圖片都指向 HTTPS 正式網址。
- 以 Codex Browser 開啟兩頁、切換語言、導覽錨點及開啟 Demo，確認英文路徑不產生資源 404。GUI 驗證依 AGENTS.md 的本次對話授權規則進行。
- GitHub About 使用可理解的英文用途介紹、HTTPS homepage 與相關 topics；不加入不相關熱門關鍵字。
- 每次發布更新中英文 README 的功能與下載資訊，維持 repo、官網與實際 App 一致。
- 建立索引後以 Search Console 的曝光、點擊、查詢與索引狀態觀測成效；GitHub Traffic 僅供有權限者檢查瀏覽及來源。不為了 SEO 加入未經需求的分析追蹤。

這些設定讓內容可被正確爬取、理解及分享；搜尋引擎決定收錄時間與排名，技術設定不保證排名或特定 rich result。
