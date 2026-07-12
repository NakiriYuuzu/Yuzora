# Design System: Yuzora Product Page (gh-pages)

> 適用範圍：`site/` 下的 GitHub Pages 產品 landing page 與 `site-remotion/` 功能影片。
> **最高原則：一切視覺以 app 本體為準（`src/styles.css` 的 tokens）——不自創品牌色、不隨意編排 UI。**
> 2026-07-10 修訂：廢除初版自創的「暮橙」方向，全面對齊 IDE 實際樣式。

## 1. Visual Theme & Atmosphere

產品頁是「產品本身的延伸」：暖紙色、glass 表面、四角淡彩 mesh 漸層、單一 lime accent——
與打開 app 第一眼看到的完全同源。頁面像把 workbench 攤在桌上介紹它自己。

- **Density 4、Variance 7（不對稱 Hero／zig-zag features）、Motion 5（CSS 微動效）。**
- 所有 UI 示意（hero mock、功能影片）都是 workbench 的忠實重現，不是風格化插畫。

## 2. Color Palette & Roles（來源：src/styles.css light theme）

- **Paper 1** `#fbfaf6` — 基底紙色（`--paper-1`）。
- **Paper 0** `#ffffff` — 卡片／panel 填色（`--paper-0`）。
- **Ink 0/1/2/3/4** `#1b1a17 / #2e2c28 / #57534b / #8a857a / #b6b0a3` — 文字層級。
- **Line 1** `rgba(27,26,23,0.10)` — 1px 結構線與描邊。
- **Lime Accent** `#86b81f`（`--yz-accent`）— 按鈕填色、狀態點、icon 高亮；
  **文字型 accent 一律用** `#5f8c1e`（`--yz-accent-ink`）確保對比。
- **Mesh 背景** `--yz-bg`：四角 radial（左上 `#e9f4c8`、右上 `#cdeed3`、右下 `#c4dbf6`、左下 `#f6e6c8`）＋ `#f6f5ef` 底。頁面 body 與影片畫布同用。
- **Terminal 米色系** `--term-bg #f1ede4`、`--term-bar #e8e2d7`、fg `#46433c`——terminal 是淺米色，**不是深色**。
- **品牌漸層**：logo 方塊 `--grad-sunrise`（`#ffb23e→#ff6b54→#e0539b`）；avatar `--grad-dusk`（`#7b5bff→#2f6bff`）。
- 深色只用於 footer（dark `--paper-1` `#1a191f`）。

## 3. Typography Rules（與 app 同源）

- **Wordmark／session 標題：Newsreader serif**，weight 600、`-0.01em`，小寫「yuzora」。
- **標題／內文：Hanken Grotesk**（中文 fallback PingFang TC／Noto Sans TC）。
- **Mono：JetBrains Mono** — 指令、檔名、平台標籤、status bar、⌘K chips。
- app 內 IDE 密度字級 9–13px；mock 與影片內依真實比例呈現。
- Banned：Inter、generic serif 當內文、任何非上述字體。

## 4. UI 重現守則（mock 與影片）

hero mock 與功能影片必須含以下真實結構，樣式值以 scout spec／`src/styles.css` 為準：

- **佈局**：左 activity rail（60px；按鈕 38×32 r10 白底）→ glass sidebar 卡（r20、`rgba(251,250,246,0.55)`、
  serif 標題＋mono 路徑＋搜尋框＋5 mode tabs＋SECTION label）→ 主 panel 卡
  （r20、白底、`--shadow-lg`）→ 底部 status bar（30px、glass、mono 11.5px、lime dot＋branch）。
- **AgentZone**：session header 帶 agent 色 13% 漸層（claude `#c0562f`）＋ ACP 紫 pill `#5b3fd1`；
  user 氣泡右對齊 `--yz-active` 底 r14（tail 角 5px）；tool block r12＋左 3px lime bar；
  diff 卡 mint-soft；權限卡 amber-soft＋「允許」lime chip；composer 38px field＋ink 送出鈕。
- **Database**：Run 按鈕 lime h26 r8；結果表 mono 12px、表頭 paper-1、格線 `line-1/60`。
- **Git／Terminal**：commit 卡（branch pill＋amber 變更數 pill）；M badge 藍 `#2456cc`／`#e7eeff`；
  tab pill h30 r9 active `--yz-active`；terminal drawer 米色 bar＋`#c0562f` hash＋`#1f8a5b` 成功行。
- 動畫節奏沿用 app：`--ease-out cubic-bezier(0.22,1,0.36,1)`、yzrowin（表格列 translateX -5px）、
  yzblink（step-end 游標）、yzpulse（狀態點）。

## 5. Feature Videos（Remotion，`site-remotion/`）

- 每支影片 = **完整 workbench 畫面**（1280×800 = 真實視窗尺寸，1:1 樣式），非浮動小卡。
- 共用 `Workbench.tsx` 外框；三支腳本：agentzone（對話→diff→權限→cargo check）、
  remote-db（query→Run→表格列進場）、terminal-git（git log→cherry-pick）。
- **全部依語言出雙版**（UI chrome 含文字）：`<name>-zh.mp4` / `<name>-en.mp4`，共 6 支。
- 渲染 `--scale=2`（2560×1600）保清晰；片尾內容淡出、chrome 不動，loop 接回乾淨狀態。
- 頁面呈現：影片即「app 截錄」，r14 圓角＋`line-1` 描邊＋`--shadow-lg`。
- 重渲染：`cd site-remotion && bunx remotion render <id>-<lang> ../site/assets/<id>-<lang>.mp4 --scale=2`。

## 6. Layout / Motion / Responsive（頁面）

- `max-width 1200px`；Hero 45/55 不對稱、mock 桌機右 bleed；features zig-zag 5fr/7fr。
- hero grid item 需 `min-width: 0`（mock 內 pre 的 min-content 會撐寬 grid）。
- 進場 reveal：opacity＋translateY(16px)、IntersectionObserver、`--ease-out`。
- 影片進 viewport 才播、離開暫停；`prefers-reduced-motion` 停用 autoplay 與動效。
- `<768px`：單欄、mock 隱藏 sidebar、body `background-attachment: scroll`（iOS）。
- CTA：ink 底 paper 字（app primary button 樣式）；hover translateY(-1px)。

## 7. i18n（繁中／英文）

- HTML 原始碼繁中為 source of truth；`data-i18n` / `data-i18n-html` ＋頁尾 `I18N` 字典。
- 切換時同步 `<html lang>`、title、meta description、**與全部三支影片的 src**（`assets/<name>-<lang>.mp4`）。
- 初始語言：`localStorage` → `navigator.language`。

## 8. Content Rules

- 不捏造數據；下載連結固定 `releases/latest`；不放 emoji／「Scroll to explore」；
  禁 AI 濫調（無縫、賦能、Elevate…）。
- 影片內指令、程式碼、commit 訊息須為真實可信的內容（取自專案實際樣貌）。

## 9. Anti-Patterns (Banned)

- **自創品牌色**（含已廢除的暮橙 #C2551E）、深色 terminal 示意、發明不存在的 UI 佈局。
- 純黑、Inter、generic serif、emoji、霓虹光暈、custom cursor。
- 置中 Hero、三等分卡片橫排、元素重疊、假統計、`LABEL // YEAR`。
- 圓形 spinner；Unsplash 熱連結（示意一律 CSS/SVG/Remotion 重現）。
