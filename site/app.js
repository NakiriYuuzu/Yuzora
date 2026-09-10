/* global getComputedStyle, URLSearchParams, document, window, localStorage, matchMedia, IntersectionObserver, requestAnimationFrame, performance, fetch, setInterval, navigator */

import { initDownloadExperience } from "./downloads.js";

/* ============ i18n ============ */
const I18N = {
  "zh-Hant": {
    "nav.demo": "試用 Demo",
    "demo.title": "先玩一下，再一起開工。",
    "demo.description": "打開檔案、試試 Git diff、跑一段示範 SQL，或幫工作面換個顏色。你的 Space 夥伴也在等你。",
    "demo.open": "開啟互動 Demo ↗",
    "demo.note": "使用範例資料，操作只保留在此瀏覽器分頁。",
    "demo.load": "點一下，進入你的示範工作面 →",

    "meta.title": "Yuzora · ADE × HERDR",
    "meta.description": "Yuzora 是融合 Agent Development Environment 與 HERDR runtime 的開源桌面工作面。Spaces、Sessions、Agents、編輯器、終端機、SSH 與資料庫，同一個環境。",
    "meta.ogDescription": "讓 agent 開發，直接運轉在 HERDR。開源桌面 ADE：Spaces、Sessions、Agents 與編輯器、終端機、SSH、資料庫共用同一個工作面。",
    "a11y.skip": "跳到主要內容",
    "a11y.primaryNav": "主導覽",
    "a11y.footerNav": "頁尾連結",
    "nav.features": "功能",
    "nav.boundary": "邊界",
    "nav.brand": "品牌",
    "brand.title": "三個折面，一個 Yuzora。",
    "brand.intro": "銀色、鈷藍與海玻璃綠交會成 Y，像分頭展開的工作，回到同一個安定的中心。",
    "brand.lockup": "標誌與字標",
    "brand.lockupAlt": "Yuzora 品牌標誌與字標",
    "brand.lockupCaption": "獨立的折面，連續的方向。字標保留清楚、開放的閱讀節奏。",
    "brand.geometry": "幾何構成",
    "brand.geometryAlt": "Yuzora 標誌的網格、折線與間距構成",
    "brand.geometryCaption": "256 單位母版、16 單位網格，中央保留 12 單位的呼吸間距。",
    "brand.symbol": "App 圖示與標誌",
    "brand.symbolCaption": "App 圖示、介面標誌與分頁圖示，共用同一組主題色。",
    "brand.mono": "單色也清晰",
    "brand.monoAlt": "Yuzora 單色標誌",
    "brand.monoCaption": "保留折面的留白，深色與淺色背景都能辨識。",
    "brand.theme": "與主題一起變化",
    "brand.themeCaption": "選一個喜歡的顏色。Logo、App 圖示與整個頁面，會一起換上新的表情。",
    "brand.downloadMark": "下載純標誌 SVG",
    "brand.downloadLockup": "下載含文字 SVG",
    "brand.downloadGeometry": "幾何構成 SVG",
    "nav.cta": "下載 Yuzora",
    "nav.theme": "切換深淺色",
    "nav.language": "切換語言",
    "palette.search": "搜尋示範命令",
    "media.adeHerdr": "Yuzora 的 Spaces、Sessions、Agents 與 HERDR 終端工作面",
    "media.remoteDb": "Yuzora 的資料庫查詢、結果與主題設定",
    "media.terminalGit": "Yuzora 的終端機與 Git 工作面",
    "github.stars": "GitHub 星標數：{{count}}",
    "hero.kicker": "開源桌面 ADE · HERDR RUNTIME",
    "hero.h1a": "讓 agent 開發，",
    "hero.h1b": "直接運轉在 ",
    "hero.h1acc": "HERDR",
    "hero.h1c": "。",
    "hero.sub": "Yuzora 把 Spaces、Sessions、Agents 與編輯器、終端機、SSH、資料庫收進同一個桌面工作面；介面之下，是同一套 HERDR runtime。",
    "hero.cta": "下載 Yuzora",
    "hero.alt": "Yuzora 主工作面：Space rail、named Sessions、Agents 與 HERDR terminal pages",
    "strip.label": "以開源技術打造",
    "loop.h2": "三個動作，跑起一個 agent 工作面",
    "loop.s1t": "開一個 Space",
    "loop.s1d": "掛上專案 checkout 或 linked worktree，Space rail 上一格就位。",
    "loop.s2t": "命名 Session，派出 Agents",
    "loop.s2d": "agents 直接跑在 HERDR terminal 上，同一個 Session 集中收納。",
    "loop.s3t": "盯著 Attention 就好",
    "loop.s3d": "需要介入的事自己浮上來，處理完就回到主線。",
    "show.h2": "整條開發動線，收進同一個工作面",
    "show.mainT": "Spaces × Sessions × Agents",
    "show.mainD": "Space rail、named Sessions、Attention 與 BSP terminal pages 同屏運轉；每個 Yuzora page 對應一個 HERDR tab。",
    "show.remoteT": "資料與主題，都在同一個工作面",
    "show.remoteD": "瀏覽資料表、執行 SQL 並查看結果。示範介面使用範例資料，也能即時切換深淺模式與主題色。",
    "show.gitT": "終端機與 git 同步呼吸",
    "show.gitD": "在整合與並排 diff 之間切換，逐行檢視變更，再回到終端機查看專案狀態。",
    "bento.h2": "細節也照顧到了",
    "bento.aD": "搜尋或執行命令，鍵盤不離手。往下打幾個字試試。",
    "bento.aPh": "搜尋或執行命令",
    "bento.bT": "編輯器與搜尋",
    "bento.bD": "CodeMirror 6 分頁編輯、左右分割、語法上色與工作區文字搜尋。",
    "bento.cT": "內嵌瀏覽器",
    "bento.cD": "開啟網站與已執行的服務，支援原生 webview、導覽歷史與遠端連線。",
    "bento.dT": "自動更新",
    "bento.dD": "新版本從 GitHub Releases 自動送達。",
    "bento.eT": "雙語介面",
    "bento.eD": "繁體中文與英文，整個 app 一鍵切換。這個網頁也是。",
    "bento.eBtn": "換成 English",
    "bound.h2a": "介面是投影，",
    "bound.h2b": "runtime 是權威",
    "bound.h2c": "。",
    "bound.p": "ADE surface 負責呈現與指令；狀態、終端與 agents 一律活在 HERDR runtime。關掉視窗，工作照跑；重開視窗，一切接得回來。",
    "bound.flowCmd": "指令 →",
    "bound.flowState": "← 狀態",
    "bound.c1d": "呈現與指令",
    "bound.c2d": "狀態與執行",
    "bound.c1d2": "呈現與指令，關掉也不影響工作",
    "bound.c2d2": "狀態、終端與 agents 的唯一權威",
    "bound.c3t": "明確邊界",
    "bound.c3d": "typed IPC，mutation 依 capability 把關",
    "dl.kicker": "夕空下的 agent development environment",
    "dl.h2": "選你的平台，開始使用。",
    "dl.macD": "僅支援 Apple Silicon（M 系列）",
    "dl.macBtn": "下載 .dmg",
    "dl.winD": "x64 安裝程式",
    "dl.winBtn": "下載 .exe",
    "dl.build": "想自己建置：<code>bun install</code>，然後 <code>bun run tauri:build</code>。",
    "dl.all": "所有版本與更新紀錄",
    "dl.recommended": "適合此裝置",
    "dl.device.macos": "已辨識：macOS · 安裝檔僅適用 Apple Silicon",
    "dl.device.windows": "已辨識：Windows · x64",
    "dl.device.unsupported": "僅支援 macOS 與 Windows 桌面裝置",
    "dl.device.unsupportedArchitecture": "此裝置架構尚未提供安裝檔",
    "dl.device.unknown": "請從下方選擇你的平台",
    "foot.tag": "ADE × HERDR，開源的桌面工作面。",
  },
  "en": {
    "nav.demo": "Try Demo",
    "demo.title": "A little play. A lot of possibility.",
    "demo.description": "Open a file, explore a Git diff, run a sample query, or find your favorite color. Your Space companions are waiting.",
    "demo.open": "Open interactive demo ↗",
    "demo.note": "Sample data. Changes stay in this browser tab.",
    "demo.load": "Click to enter your demo workspace →",

    "meta.title": "Yuzora · ADE × HERDR",
    "meta.description": "Yuzora is an open-source desktop Agent Development Environment powered by the HERDR runtime, with Spaces, Sessions, Agents, editor, terminal, SSH and databases in one workspace.",
    "meta.ogDescription": "Build with agents and run directly on HERDR. Spaces, Sessions, Agents, editor, terminal, SSH and databases share one open-source desktop workspace.",
    "a11y.skip": "Skip to content",
    "a11y.primaryNav": "Primary navigation",
    "a11y.footerNav": "Footer links",
    "nav.features": "Features",
    "nav.boundary": "Boundary",
    "nav.brand": "Brand",
    "brand.title": "Three planes. One Yuzora.",
    "brand.intro": "Silver, cobalt and sea glass meet in a Y: separate paths of work returning to one steady center.",
    "brand.lockup": "Symbol & wordmark",
    "brand.lockupAlt": "Yuzora brand symbol and wordmark",
    "brand.lockupCaption": "Independent planes, a shared direction. Open letterforms keep the name clear and readable.",
    "brand.geometry": "Construction",
    "brand.geometryAlt": "Yuzora symbol construction grid, folds and spacing",
    "brand.geometryCaption": "A 256-unit master on a 16-unit grid, with a 12-unit seam at the center.",
    "brand.symbol": "App icon & symbol",
    "brand.symbolCaption": "App icons, interface symbols and the favicon share the same theme palette.",
    "brand.mono": "Clear in one color",
    "brand.monoAlt": "Yuzora monochrome symbol",
    "brand.monoCaption": "Open spaces between the planes preserve the silhouette on light and dark surfaces.",
    "brand.theme": "In tune with the theme",
    "brand.themeCaption": "Pick your favorite color. The logo, app icon and entire page change together.",
    "brand.downloadMark": "Download symbol SVG",
    "brand.downloadLockup": "Download wordmark SVG",
    "brand.downloadGeometry": "Construction SVG",
    "nav.cta": "Download Yuzora",
    "nav.theme": "Toggle color theme",
    "nav.language": "Switch language",
    "palette.search": "Search demo commands",
    "media.adeHerdr": "Yuzora Spaces, Sessions, Agents and HERDR terminal workspace",
    "media.remoteDb": "Yuzora database queries, results and theme settings",
    "media.terminalGit": "Yuzora terminal and Git workspace",
    "github.stars": "GitHub stars: {{count}}",
    "hero.kicker": "OPEN-SOURCE DESKTOP ADE · HERDR RUNTIME",
    "hero.h1a": "Build with agents.",
    "hero.h1b": "Run on ",
    "hero.h1acc": "HERDR",
    "hero.h1c": ".",
    "hero.sub": "Spaces, Sessions, Agents, editor, terminal, SSH and databases in one desktop workspace, driven by a single HERDR runtime.",
    "hero.cta": "Download Yuzora",
    "hero.alt": "Yuzora workspace: Space rail, named Sessions, Agents and HERDR terminal pages",
    "strip.label": "Built on open source",
    "loop.h2": "Three moves to a running agent workspace",
    "loop.s1t": "Open a Space",
    "loop.s1d": "Point it at a checkout or a linked worktree; it lands on the Space rail.",
    "loop.s2t": "Name a Session, dispatch Agents",
    "loop.s2d": "Agents run on real HERDR terminals, gathered under one named Session.",
    "loop.s3t": "Just watch Attention",
    "loop.s3d": "Anything blocked surfaces on its own; handle it and get back to your main line.",
    "show.h2": "One workspace for the whole dev loop",
    "show.mainT": "Spaces × Sessions × Agents",
    "show.mainD": "Space rail, named Sessions, Attention and BSP terminal pages run side by side; every Yuzora page maps to a HERDR tab.",
    "show.remoteT": "Your data. Your workspace.",
    "show.remoteD": "Browse tables, run SQL and explore the results. This interactive preview uses sample data and lets you change the appearance and accent instantly.",
    "show.gitT": "Terminal and git, in step",
    "show.gitD": "Switch between unified and split diffs, review changes line by line, then return to the terminal to check your project.",
    "bento.h2": "The details are covered",
    "bento.aD": "Search or run any command without leaving the keyboard. Type below to try it.",
    "bento.aPh": "Search or run a command",
    "bento.bT": "Editor and search",
    "bento.bD": "CodeMirror 6 tabs with split view, syntax highlighting and workspace text search.",
    "bento.cT": "Embedded browser",
    "bento.cD": "Open websites and running services with a native webview, navigation history and remote connections.",
    "bento.dT": "Auto-update",
    "bento.dD": "New releases arrive automatically from GitHub Releases.",
    "bento.eT": "Bilingual interface",
    "bento.eD": "Traditional Chinese and English, one click for the whole app. This page too.",
    "bento.eBtn": "切換成中文",
    "bound.h2a": "The interface is a projection. ",
    "bound.h2b": "The runtime is the authority",
    "bound.h2c": ".",
    "bound.p": "The ADE surface renders and commands; state, terminals and agents live in the HERDR runtime. Close the window and work keeps running; reopen it and everything reattaches.",
    "bound.flowCmd": "commands →",
    "bound.flowState": "← state",
    "bound.c1d": "Rendering and commands",
    "bound.c2d": "State and execution",
    "bound.c1d2": "Rendering and commands; closing it never stops work",
    "bound.c2d2": "The single authority for state, terminals and agents",
    "bound.c3t": "Explicit boundary",
    "bound.c3d": "Typed IPC; mutations gated by capability",
    "dl.kicker": "agent development under the evening sky",
    "dl.h2": "Pick your platform and go.",
    "dl.macD": "Apple Silicon (M series) only",
    "dl.macBtn": "Download .dmg",
    "dl.winD": "x64 installer",
    "dl.winBtn": "Download .exe",
    "dl.build": "Build it yourself: <code>bun install</code>, then <code>bun run tauri:build</code>.",
    "dl.all": "All releases and notes",
    "dl.recommended": "Recommended",
    "dl.device.macos": "Detected: macOS · Installer requires Apple Silicon",
    "dl.device.windows": "Detected: Windows · x64",
    "dl.device.unsupported": "Available for macOS and Windows desktop devices",
    "dl.device.unsupportedArchitecture": "No installer is available for this device architecture yet",
    "dl.device.unknown": "Choose your platform below",
    "foot.tag": "ADE × HERDR, an open-source desktop workspace.",
  },
};

/* Mini palette 指令（真實功能，示意互動） */
const MP_COMMANDS = [
  { zh: "新增 HERDR terminal", en: "New HERDR terminal", cat: "Herdr" },
  { zh: "切換 Space", en: "Switch Space", cat: "Spaces" },
  { zh: "Git：暫存變更", en: "Git: stage changes", cat: "Git" },
  { zh: "SSH：連線主機", en: "SSH: connect host", cat: "SSH" },
  { zh: "資料庫：新增連線", en: "Database: new connection", cat: "Database" },
  { zh: "瀏覽器：開啟網址", en: "Browser: open URL", cat: "Browser" },
  { zh: "切換語言", en: "Toggle language", cat: "UI", action: "lang" },
];

const LANG_KEY = "yuzora-lang";
const mediaLang = (lang) => (lang === "en" ? "en" : "zh");
const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
const finePointer = window.matchMedia("(pointer: fine)").matches;

/* Kicker 逐字進場（hyday 手法） */
function waveKicker() {
  if (reduceMotion) return;
  const kicker = document.getElementById("hero-kicker");
  const text = kicker.textContent;
  kicker.textContent = "";
  [...text].forEach((c, i) => {
    if (c === " ") { kicker.appendChild(document.createTextNode(" ")); return; }
    const span = document.createElement("span");
    span.className = "ch";
    span.style.setProperty("--i", i);
    span.textContent = c;
    kicker.appendChild(span);
  });
}

function renderPalette(lang, filter) {
  const list = document.getElementById("mp-list");
  const key = lang === "en" ? "en" : "zh";
  const q = (filter || "").trim().toLowerCase();
  const rows = MP_COMMANDS.filter((c) =>
    !q || c.zh.toLowerCase().includes(q) || c.en.toLowerCase().includes(q) || c.cat.toLowerCase().includes(q)
  );
  list.innerHTML = "";
  if (!rows.length) {
    const li = document.createElement("li");
    li.className = "mp-empty";
    li.textContent = lang === "en" ? "No matching command" : "沒有符合的命令";
    list.appendChild(li);
    return;
  }
  rows.slice(0, 4).forEach((c, i) => {
    const li = document.createElement("li");
    if (i === 0) li.classList.add("is-active");
    const name = document.createElement("span");
    name.textContent = c[key];
    const cat = document.createElement("span");
    cat.className = "mp-cat";
    cat.textContent = c.cat;
    if (c.action === "lang") {
      const button = document.createElement("button");
      button.type = "button";
      button.append(name, cat);
      button.addEventListener("click", toggleLang);
      li.classList.add("is-action");
      li.appendChild(button);
    } else {
      li.append(name, cat);
    }
    list.appendChild(li);
  });
}

function applyLang(lang) {
  const dict = I18N[lang] || I18N["zh-Hant"];
  const ml = mediaLang(lang);
  document.documentElement.lang = lang;
  document.title = dict["meta.title"];
  document.querySelector('meta[name="description"]').setAttribute("content", dict["meta.description"]);
  document.querySelector('meta[property="og:title"]').setAttribute("content", dict["meta.title"]);
  document.querySelector('meta[property="og:description"]').setAttribute("content", dict["meta.ogDescription"]);
  document.querySelector('meta[property="og:image"]').setAttribute("content", `assets/ade-herdr-runtime-${ml}.png`);
  document.querySelectorAll("[data-i18n]").forEach((el) => {
    const key = el.getAttribute("data-i18n");
    if (dict[key] != null) el.textContent = dict[key];
  });
  document.querySelectorAll("[data-i18n-html]").forEach((el) => {
    const key = el.getAttribute("data-i18n-html");
    if (dict[key] != null) el.innerHTML = dict[key];
  });
  document.querySelectorAll("[data-i18n-alt]").forEach((el) => {
    const key = el.getAttribute("data-i18n-alt");
    if (dict[key] != null) el.alt = dict[key];
  });
  document.querySelectorAll("[data-i18n-placeholder]").forEach((el) => {
    const key = el.getAttribute("data-i18n-placeholder");
    if (dict[key] != null) el.placeholder = dict[key];
  });
  document.querySelectorAll("[data-i18n-aria-label]").forEach((el) => {
    const key = el.getAttribute("data-i18n-aria-label");
    if (dict[key] != null) {
      const value = key === "github.stars"
        ? dict[key].replace("{{count}}", document.getElementById("gh-count").textContent)
        : dict[key];
      el.setAttribute("aria-label", value);
    }
  });
  document.querySelectorAll("img[data-imgstem]").forEach((img) => {
    img.src = `assets/${img.dataset.imgstem}-${ml}.png`;
  });
  document.querySelectorAll("video[data-vstem]").forEach((video) => {
    const wasPlaying = !video.paused;
    video.poster = `assets/${video.dataset.posterStem}-${ml}.png`;
    video.querySelector("source").src = `assets/${video.dataset.vstem}-${ml}.mp4`;
    video.load();
    if (wasPlaying && !document.hidden) video.play().catch(() => {});
  });
  document.getElementById("lang-toggle").textContent = lang === "en" ? "中文" : "EN";
  updateDemoLinks();
  waveKicker();
  renderPalette(lang, document.getElementById("mp-input").value);
  try { localStorage.setItem(LANG_KEY, lang); } catch { /* storage unavailable */ }
}

let currentLang = "zh-Hant";
try {
  const saved = localStorage.getItem(LANG_KEY);
  if (saved === "en" || saved === "zh-Hant") currentLang = saved;
} catch { /* storage unavailable */ }

function toggleLang() {
  currentLang = currentLang === "en" ? "zh-Hant" : "en";
  applyLang(currentLang);
}
document.getElementById("lang-toggle").addEventListener("click", toggleLang);
document.getElementById("lang-demo").addEventListener("click", toggleLang);

if (currentLang !== "zh-Hant") {
  applyLang(currentLang);
} else {
  waveKicker();
  renderPalette(currentLang, "");
}

/* Mini palette 輸入過濾 */
document.getElementById("mp-input").addEventListener("input", (e) => {
  renderPalette(currentLang, e.target.value);
});

/* ============ 技術 marquee（真實 stack logos） ============ */
const STACK = [
  ["tauri", "Tauri"], ["react", "React"], ["rust", "Rust"], ["typescript", "TypeScript"],
  ["bun", "Bun"], ["postgresql", "PostgreSQL"], ["sqlite", "SQLite"], ["git", "Git"],
];
const track = document.getElementById("marquee-track");
[0, 1].forEach((dup) => {
  STACK.forEach(([slug, name]) => {
    const img = document.createElement("img");
    img.src = `https://cdn.simpleicons.org/${slug}`;
    img.alt = name;
    img.loading = "lazy";
    if (dup === 1) img.classList.add("dupe");
    track.appendChild(img);
  });
});

/* ============ Dark / Light mode ============ */
const THEME_KEY = "yuzora-theme";
const themeMeta = document.querySelector('meta[name="theme-color"]');
function applyTheme(theme) {
  document.documentElement.setAttribute("data-theme", theme);
  updateBrandFavicon();
  if (themeMeta) themeMeta.setAttribute("content", theme === "dark" ? "#12141f" : "#ffffff");
}
applyTheme(document.documentElement.getAttribute("data-theme") === "dark" ? "dark" : "light");
document.getElementById("theme-toggle").addEventListener("click", () => {
  const next = document.documentElement.getAttribute("data-theme") === "dark" ? "light" : "dark";
  applyTheme(next);
  try { localStorage.setItem(THEME_KEY, next); } catch { /* storage unavailable */ }
});
matchMedia("(prefers-color-scheme: dark)").addEventListener("change", (e) => {
  let saved = null;
  try { saved = localStorage.getItem(THEME_KEY); } catch { /* storage unavailable */ }
  if (!saved) applyTheme(e.matches ? "dark" : "light");
});

/* ============ Nav scrolled state（IO sentinel，不用 scroll listener） ============ */
const nav = document.querySelector(".site-nav");
new IntersectionObserver(([entry]) => {
  nav.classList.toggle("is-scrolled", !entry.isIntersecting);
}).observe(document.getElementById("nav-sentinel"));

/* ============ Scrollspy ============ */
const spyLinks = document.querySelectorAll("[data-spy]");
const spyIO = new IntersectionObserver((entries) => {
  entries.forEach((entry) => {
    const link = document.querySelector(`[data-spy="${entry.target.id}"]`);
    if (link) link.classList.toggle("is-active", entry.isIntersecting);
  });
}, { rootMargin: "-35% 0px -55% 0px" });
spyLinks.forEach((link) => {
  const target = document.getElementById(link.dataset.spy);
  if (target) spyIO.observe(target);
});

/* ============ Scroll reveal ============ */
if (!reduceMotion) {
  const revealIO = new IntersectionObserver((entries) => {
    entries.forEach((entry) => {
      if (entry.isIntersecting) {
        entry.target.classList.add("is-in");
        revealIO.unobserve(entry.target);
      }
    });
  }, { threshold: 0.18 });
  document.querySelectorAll(".reveal").forEach((el) => revealIO.observe(el));
} else {
  document.querySelectorAll(".reveal").forEach((el) => el.classList.add("is-in"));
}

/* ============ Hero 滑鼠視差（tilt + float tags） ============ */
if (finePointer && !reduceMotion) {
  const stage = document.getElementById("hero-stage");
  const shot = document.getElementById("hero-shot");
  const tags = stage.querySelectorAll(".float-tag");
  let raf = null;
  let px = 0, py = 0;
  const render = () => {
    raf = null;
    shot.style.transform = `rotateX(${py * -2.4}deg) rotateY(${px * 3}deg)`;
    tags.forEach((tag) => {
      const depth = Number(tag.dataset.depth) || 20;
      tag.style.translate = `${px * depth}px ${py * depth}px`;
    });
  };
  stage.addEventListener("pointermove", (e) => {
    const rect = stage.getBoundingClientRect();
    px = (e.clientX - rect.left) / rect.width - 0.5;
    py = (e.clientY - rect.top) / rect.height - 0.5;
    if (!raf) raf = requestAnimationFrame(render);
  });
  stage.addEventListener("pointerleave", () => {
    px = 0; py = 0;
    if (!raf) raf = requestAnimationFrame(render);
  });
}

/* ============ Bento spotlight ============ */
if (finePointer) {
  document.querySelectorAll(".bento-card").forEach((card) => {
    card.addEventListener("pointermove", (e) => {
      const rect = card.getBoundingClientRect();
      card.style.setProperty("--mx", `${e.clientX - rect.left}px`);
      card.style.setProperty("--my", `${e.clientY - rect.top}px`);
    });
  });
}

/* ============ GitHub stars 實時 ============ */
const starsWrap = document.getElementById("gh-stars");
const starsNum = document.getElementById("gh-count");
let shownStars = 0;
const fmtStars = (n) => (n >= 1000 ? (n / 1000).toFixed(1).replace(/\.0$/, "") + "k" : String(n));
function updateStarsLabel() {
  starsWrap.setAttribute(
    "aria-label",
    I18N[currentLang]["github.stars"].replace("{{count}}", starsNum.textContent),
  );
}
function countTo(target) {
  if (reduceMotion) {
    shownStars = target;
    starsNum.textContent = fmtStars(target);
    updateStarsLabel();
    return;
  }
  const from = shownStars;
  const t0 = performance.now();
  const dur = 900;
  const step = (now) => {
    const p = Math.min(1, (now - t0) / dur);
    const eased = 1 - Math.pow(1 - p, 3);
    starsNum.textContent = fmtStars(Math.round(from + (target - from) * eased));
    if (p < 1) requestAnimationFrame(step);
    else {
      shownStars = target;
      updateStarsLabel();
    }
  };
  requestAnimationFrame(step);
}
async function refreshStars() {
  try {
    const r = await fetch("https://api.github.com/repos/NakiriYuuzu/Yuzora", {
      headers: { Accept: "application/vnd.github+json" },
    });
    if (!r.ok) return;
    const data = await r.json();
    if (typeof data.stargazers_count !== "number") return;
    starsWrap.classList.add("is-on");
    if (data.stargazers_count !== shownStars) countTo(data.stargazers_count);
  } catch { /* badge remains hidden while GitHub is unavailable */ }
}
refreshStars();
setInterval(refreshStars, 60000);

/* ============ 磁性 CTA（primary 按鈕輕微吸附游標） ============ */
if (finePointer && !reduceMotion) {
  document.querySelectorAll(".btn-primary, .nav-cta").forEach((btn) => {
    let magRaf = null;
    let mx = 0, my = 0;
    const renderMag = () => {
      magRaf = null;
      btn.style.translate = `${mx}px ${my}px`;
    };
    btn.addEventListener("pointermove", (e) => {
      const rect = btn.getBoundingClientRect();
      mx = ((e.clientX - rect.left) / rect.width - 0.5) * 8;
      my = ((e.clientY - rect.top) / rect.height - 0.5) * 6;
      if (!magRaf) magRaf = requestAnimationFrame(renderMag);
    });
    btn.addEventListener("pointerleave", () => {
      mx = 0; my = 0;
      if (!magRaf) magRaf = requestAnimationFrame(renderMag);
    });
  });
}

/* ============ Video autoplay in view ============ */
const videos = document.querySelectorAll("video[data-vstem]");
function syncVideoPlayback() {
  videos.forEach((video) => {
    if (!reduceMotion && !document.hidden && video.dataset.inView === "true") {
      video.play().catch(() => {});
    } else {
      video.pause();
    }
  });
}
if (reduceMotion) {
  videos.forEach((video) => { video.controls = true; });
} else {
  const videoIO = new IntersectionObserver((entries) => {
    entries.forEach((entry) => {
      entry.target.dataset.inView = String(entry.isIntersecting);
    });
    syncVideoPlayback();
  }, { threshold: 0.35 });
  videos.forEach((video) => videoIO.observe(video));
}
document.addEventListener("visibilitychange", syncVideoPlayback);

/* ============ 平台偵測下載（downloads.js 契約） ============ */
document.documentElement.classList.remove("no-js");
void initDownloadExperience(navigator, document);

/* Brand colors use the same five choices as the desktop app. */
function updateBrandFavicon() {
  const source = document.getElementById("yuzora-mark");
  if (!source) return;
  const style = getComputedStyle(document.documentElement);
  const paths = source.innerHTML.replace(/var\((--[\w-]+),\s*[^)]+\)/g, (_, token) => style.getPropertyValue(token).trim());
  document.querySelector('link[rel="icon"]').href = `data:image/svg+xml,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256">${paths}</svg>`)}`;
}
function updateDemoLinks() {
  const params = new URLSearchParams({ lang: document.documentElement.lang === "en" ? "en" : "zh-TW", accent: document.documentElement.dataset.accent || "blue", theme: document.documentElement.dataset.theme || "light" });
  document.querySelectorAll("[data-demo-link]").forEach(link => { link.href = `demo/?${params}`; });
}
function applyBrandAccent(name) {
  const colors = {lime:["#86b81f","134,184,31","#5f8c1e"],blue:["#2f6bff","47,107,255","#2456cc"],violet:["#7b5bff","123,91,255","#5d3fd3"],coral:["#ff6b54","255,107,84","#c0562f"],amber:["#e0a11f","224,161,31","#a8690f"]};
  const color = colors[name] || colors.blue;
  const root = document.documentElement;
  root.dataset.accent = colors[name] ? name : "blue";
  root.style.setProperty("--accent",color[0]);
  root.style.setProperty("--accent-rgb",color[1]);
  root.style.setProperty("--accent-ink",color[2]);
  document.querySelectorAll("[data-brand-accent]").forEach(button => button.setAttribute("aria-pressed",String(button.dataset.brandAccent === root.dataset.accent)));
  updateBrandFavicon();
  updateDemoLinks();
}
let savedAccent = "blue";
try { savedAccent = localStorage.getItem("yuzora-accent") || "blue"; } catch { /* private browsing */ }
applyBrandAccent(savedAccent);
document.querySelectorAll("[data-brand-accent]").forEach(button => button.addEventListener("click", () => {
  applyBrandAccent(button.dataset.brandAccent);
  try { localStorage.setItem("yuzora-accent", button.dataset.brandAccent); } catch { /* private browsing */ }
}));
document.getElementById("theme-toggle").addEventListener("click", updateDemoLinks);
document.getElementById("demo-launch").addEventListener("click", () => {
  const frame = document.createElement("iframe");
  frame.src = document.querySelector("[data-demo-link]").href;
  frame.title = "Yuzora interactive demo";
  frame.className = "demo-frame";
  document.getElementById("demo-frame-host").appendChild(frame);
  document.getElementById("demo-launch").hidden = true;
});
