/* global getComputedStyle, URLSearchParams, document, window, localStorage, matchMedia, IntersectionObserver, requestAnimationFrame, performance, fetch, setInterval, navigator */

import { initDownloadExperience } from "./downloads.js";

/* ============ i18n ============ */
import { I18N } from "./i18n.js";

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

// Each language has its own crawlable, fully rendered URL. Do not override it
// with a saved preference: shared links and search results must stay consistent.
const currentLang = document.documentElement.lang === "en" ? "en" : "zh-Hant";
const languageLink = document.getElementById("lang-toggle");
languageLink.href += window.location.hash;
window.addEventListener("hashchange", () => {
  languageLink.hash = window.location.hash;
});
function toggleLang() {
  window.location.assign(languageLink.href);
}
document.getElementById("lang-demo").addEventListener("click", toggleLang);
waveKicker();
renderPalette(currentLang, "");

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
  document.querySelectorAll("[data-demo-link]").forEach(link => { link.href = `${currentLang === "en" ? "../" : ""}demo/?${params}`; });
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
