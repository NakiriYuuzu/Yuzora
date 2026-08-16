/* global document, window, localStorage, matchMedia, IntersectionObserver, requestAnimationFrame, performance, fetch, setInterval, navigator */

import { initDownloadExperience } from "./downloads.js";

/* ============ i18n ============ */
const I18N = {
  "zh-Hant": {
    "meta.title": "Yuzora · ADE × HERDR",
    "meta.description": "Yuzora 是融合 Agent Development Environment 與 HERDR runtime 的開源桌面工作面。Spaces、Sessions、Agents、編輯器、終端機、SSH 與資料庫，同一個環境。",
    "meta.ogDescription": "讓 agent 開發，直接運轉在 HERDR。開源桌面 ADE：Spaces、Sessions、Agents 與編輯器、終端機、SSH、資料庫共用同一個工作面。",
    "a11y.skip": "跳到主要內容",
    "a11y.primaryNav": "主導覽",
    "a11y.footerNav": "頁尾連結",
    "nav.features": "功能",
    "nav.boundary": "邊界",
    "nav.cta": "下載 Yuzora",
    "nav.theme": "切換深淺色",
    "nav.language": "切換語言",
    "palette.search": "搜尋示範命令",
    "media.adeHerdr": "Yuzora 的 Spaces、Sessions、Agents 與 HERDR 終端工作面",
    "media.remoteDb": "Yuzora 的 SSH、SFTP 與資料庫工作面",
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
    "show.remoteT": "遠端與資料庫，也在同一格",
    "show.remoteD": "SSH shell 與 SFTP 雙欄傳輸；SQLite、PostgreSQL 與 MSSQL 連線、物件樹與 SQL 查詢，結果直接分頁呈現。",
    "show.gitT": "終端機與 git 同步呼吸",
    "show.gitD": "終端機跑 cherry-pick 與 push，git graph 即時長出新節點，main 與 origin/main 的 refs 跟著移動。",
    "bento.h2": "細節也照顧到了",
    "bento.aD": "搜尋或執行命令，鍵盤不離手。往下打幾個字試試。",
    "bento.aPh": "搜尋或執行命令",
    "bento.bT": "編輯器與 LSP",
    "bento.bD": "CodeMirror 6 分頁編輯、左右分割；跳至定義與工作區符號搜尋。",
    "bento.cT": "內嵌 Preview",
    "bento.cD": "啟動或接上 dev server，原生 webview 直接疊在面板上。",
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
    "dl.macD": "Apple Silicon 與 Intel 通用映像檔",
    "dl.macBtn": "下載 .dmg",
    "dl.winD": "x64 安裝程式",
    "dl.winBtn": "下載 .exe",
    "dl.build": "想自己建置：<code>bun install</code>，然後 <code>bun run tauri:build</code>。",
    "dl.all": "所有版本與更新紀錄",
    "dl.recommended": "適合此裝置",
    "dl.device.macos": "已辨識：macOS · Universal",
    "dl.device.windows": "已辨識：Windows · x64",
    "dl.device.unsupported": "僅支援 macOS 與 Windows 桌面裝置",
    "dl.device.unsupportedArchitecture": "此裝置架構尚未提供安裝檔",
    "dl.device.unknown": "請從下方選擇你的平台",
    "foot.tag": "ADE × HERDR，開源的桌面工作面。",
  },
  "en": {
    "meta.title": "Yuzora · ADE × HERDR",
    "meta.description": "Yuzora is an open-source desktop Agent Development Environment powered by the HERDR runtime, with Spaces, Sessions, Agents, editor, terminal, SSH and databases in one workspace.",
    "meta.ogDescription": "Build with agents and run directly on HERDR. Spaces, Sessions, Agents, editor, terminal, SSH and databases share one open-source desktop workspace.",
    "a11y.skip": "Skip to content",
    "a11y.primaryNav": "Primary navigation",
    "a11y.footerNav": "Footer links",
    "nav.features": "Features",
    "nav.boundary": "Boundary",
    "nav.cta": "Download Yuzora",
    "nav.theme": "Toggle color theme",
    "nav.language": "Switch language",
    "palette.search": "Search demo commands",
    "media.adeHerdr": "Yuzora Spaces, Sessions, Agents and HERDR terminal workspace",
    "media.remoteDb": "Yuzora SSH, SFTP and database workspace",
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
    "show.remoteT": "Remote and databases, same surface",
    "show.remoteD": "SSH shells and dual-pane SFTP; SQLite, PostgreSQL and MSSQL connections with object trees, SQL queries and paged results.",
    "show.gitT": "Terminal and git, in step",
    "show.gitD": "Run a cherry-pick and push in the terminal; the git graph grows a new node and the main and origin/main refs move in step.",
    "bento.h2": "The details are covered",
    "bento.aD": "Search or run any command without leaving the keyboard. Type below to try it.",
    "bento.aPh": "Search or run a command",
    "bento.bT": "Editor and LSP",
    "bento.bD": "CodeMirror 6 tabs with split view; go to definition and workspace symbol search.",
    "bento.cT": "Embedded Preview",
    "bento.cD": "Start or attach a dev server; a native webview overlays the panel.",
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
    "dl.macD": "Universal image for Apple Silicon and Intel",
    "dl.macBtn": "Download .dmg",
    "dl.winD": "x64 installer",
    "dl.winBtn": "Download .exe",
    "dl.build": "Build it yourself: <code>bun install</code>, then <code>bun run tauri:build</code>.",
    "dl.all": "All releases and notes",
    "dl.recommended": "Recommended",
    "dl.device.macos": "Detected: macOS · Universal",
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
  { zh: "Preview：啟動 dev server", en: "Preview: start dev server", cat: "Preview" },
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
