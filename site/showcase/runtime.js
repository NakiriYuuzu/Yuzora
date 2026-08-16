import { CONCEPTS, PRODUCTS, langSpan, icon } from "./data.js";
import { renderHeroScene, renderProductVisual } from "./scenes.js";

const root = document.querySelector("#app");
const conceptKey = document.body.dataset.concept || "cinematic";
const concept = CONCEPTS[conceptKey] || CONCEPTS.cinematic;
let language = localStorage.getItem("yuzora-showcase-language") || "zh";
let animeApi = null;
let motionApi = null;
let cleanupFns = [];

function renderProduct(product, index) {
  return `<article class="product-step reveal" id="${product.id}" data-product="${product.id}" data-index="${index}">
    <div class="product-copy">
      <div class="product-number">${product.index}</div>
      <p class="section-kicker">${langSpan(product.kickerZh, product.kickerEn)}</p>
      <h3>${product.title}</h3>
      <p class="product-description">${langSpan(product.copyZh, product.copyEn)}</p>
      <ul>${product.bulletsZh.map((bullet, i) => `<li>${icon("check")}${langSpan(bullet, product.bulletsEn[i])}</li>`).join("")}</ul>
      <a class="text-link magnetic" href="https://github.com/NakiriYuuzu/Yuzora" target="_blank" rel="noreferrer">${langSpan("查看原始碼與實作", "Inspect source and implementation")}${icon("arrow")}</a>
    </div>
    <div class="product-visual-wrap"><div class="product-visual-motion">${renderProductVisual(product)}</div></div>
  </article>`;
}

function renderSite() {
  root.innerHTML = `
    <a class="skip-link" href="#main">${langSpan("跳至主要內容", "Skip to main content")}</a>
    <div class="scroll-progress" aria-hidden="true"><span></span></div>
    <header class="site-header">
      <a class="brand magnetic" href="../showcase/" aria-label="Yuzora design showcase">
        <span class="brand-mark">Y</span><span class="brand-name">Yuzora</span><span class="brand-lab">LAB</span>
      </a>
      <button class="menu-toggle" type="button" aria-expanded="false" aria-controls="site-nav" aria-label="開啟導覽">${icon("menu")}</button>
      <nav id="site-nav" class="site-nav" aria-label="主要導覽">
        <a href="#herdr-agents">Herdr</a><a href="#files">Files</a><a href="#git-graph">Git Graph</a><a href="#ssh-sftp">SSH</a><a href="#database">Database</a>
      </nav>
      <div class="header-actions">
        <div class="lang-switch" aria-label="Language"><button type="button" data-set-lang="zh" aria-pressed="true">中</button><button type="button" data-set-lang="en" aria-pressed="false">EN</button></div>
        <a class="header-download magnetic" href="#download">${langSpan("下載", "Download")}${icon("download")}</a>
      </div>
    </header>

    <main id="main">
      <section class="hero" aria-labelledby="hero-title">
        <div class="hero-copy">
          <p class="hero-kicker"><span></span>${langSpan(concept.eyebrowZh, concept.eyebrowEn)}</p>
          <h1 id="hero-title"><span class="hero-line">${langSpan(concept.heroZh[0], concept.heroEn[0])}</span><span class="hero-line hero-line-accent">${langSpan(concept.heroZh[1], concept.heroEn[1])}</span></h1>
          <p class="hero-thesis">${langSpan(concept.thesisZh, concept.thesisEn)}</p>
          <div class="hero-actions"><a class="button button-primary magnetic" href="#herdr-agents">${langSpan("展開產品工作流", "Explore the product flow")}${icon("arrow")}</a><a class="button button-secondary magnetic" href="https://github.com/NakiriYuuzu/Yuzora" target="_blank" rel="noreferrer">${icon("github")}GitHub</a></div>
          <div class="hero-meta"><span>${concept.label}</span><span>${concept.model}</span></div>
        </div>
        <div class="hero-visual">${renderHeroScene(concept.scene)}</div>
        <div class="hero-scroll-hint" aria-hidden="true"><span>SCROLL</span><i></i></div>
      </section>

      <section class="context-section reveal" aria-labelledby="context-title">
        <div class="context-heading"><p class="section-label">00 / CONTEXT</p><h2 id="context-title">${langSpan("工具越多，脈絡越容易失聯。", "More tools should not mean less context.")}</h2></div>
        <div class="context-copy"><p>${langSpan("Agent 在 terminal、檔案在 editor、分支在另一套 Git 工具、遠端主機與資料庫又各自佔一個視窗。Yuzora 的目的不是再加一個工具，而是把它們放回同一個工作面。", "Agents live in terminals, files in editors, branches in a separate Git tool, while remote hosts and databases occupy their own windows. Yuzora is not another tool—it brings the working surfaces back together.")}</p></div>
        <div class="context-marquee" aria-hidden="true"><div><span>HERDR AGENTS</span><span>FILES</span><span>GIT GRAPH</span><span>SSH / SFTP</span><span>DATABASE</span><span>ONE WORKSPACE</span><span>HERDR AGENTS</span><span>FILES</span><span>GIT GRAPH</span></div></div>
      </section>

      <section class="product-story" aria-labelledby="product-title">
        <header class="story-header reveal"><p class="section-label">01 — 05 / PRODUCT SURFACES</p><h2 id="product-title">${langSpan("五個工作面，只有一份上下文。", "Five working surfaces. One shared context.")}</h2><p>${langSpan("向下捲動，讓同一個 workspace 依序接住 agent、程式碼、歷史、遠端與資料。", "Scroll through one workspace as it takes on agents, code, history, remotes and data.")}</p></header>
        <div class="product-orbit-rail" aria-hidden="true"><div class="rail-line"></div>${PRODUCTS.map((product, i) => `<span data-rail-target="${product.id}" style="--i:${i}"><b>${product.index}</b><em>${product.title}</em></span>`).join("")}</div>
        <div class="product-list">${PRODUCTS.map(renderProduct).join("")}</div>
      </section>

      <section class="trust-section reveal" aria-labelledby="trust-title">
        <div class="trust-orbit" aria-hidden="true"><div class="trust-core">LOCAL</div><span style="--a:0deg">credentials</span><span style="--a:90deg">known hosts</span><span style="--a:180deg">source code</span><span style="--a:270deg">permissions</span></div>
        <div class="trust-copy"><p class="section-label">LOCAL-FIRST / EXPLICIT CONTROL</p><h2 id="trust-title">${langSpan("工作留在本機；權限逐項看得見。", "Work stays local. Permission stays visible.")}</h2><p>${langSpan("Yuzora 以 Tauri 打造。連線設定、known hosts 與憑證集中留在本機；agent 的工具呼叫與變更在工作流中明確呈現，讓你先檢查，再決定。", "Yuzora is built with Tauri. Connection settings, known hosts and credentials remain locally managed, while agent tool calls and changes are surfaced in the workflow for review before approval.")}</p><div class="trust-points"><span><b>01</b>${langSpan("本機憑證管理", "Local credential management")}</span><span><b>02</b>${langSpan("逐項權限確認", "Explicit permission checks")}</span><span><b>03</b>${langSpan("變更可檢視", "Reviewable changes")}</span></div></div>
      </section>

      <section class="download-section reveal" id="download" aria-labelledby="download-title">
        <div class="download-heading"><p class="section-label">DOWNLOAD / OPEN SOURCE</p><h2 id="download-title">${langSpan("把工作台帶回你的桌面。", "Bring the workbench to your desktop.")}</h2><p id="download-device-note">${langSpan("選擇你的平台，或前往 Releases 查看所有版本。", "Choose a platform or open Releases for every available build.")}</p></div>
        <div class="download-grid">
          <a class="download-card magnetic" data-platform="macos" href="https://github.com/NakiriYuuzu/Yuzora/releases/latest/download/Yuzora-macos-universal.dmg"><span>macOS</span><b>.dmg · Universal</b><em>Apple Silicon / Intel</em>${icon("download")}</a>
          <a class="download-card magnetic" data-platform="windows" href="https://github.com/NakiriYuuzu/Yuzora/releases/latest/download/Yuzora-windows-x64-setup.exe"><span>Windows</span><b>.exe · x64</b><em>NSIS installer</em>${icon("download")}</a>
        </div>
        <div class="download-footer"><a href="https://github.com/NakiriYuuzu/Yuzora/releases" target="_blank" rel="noreferrer">${langSpan("查看所有 Releases", "View all releases")}${icon("arrow")}</a><span>Open source · Tauri 2 · React · Rust</span></div>
      </section>
    </main>

    <footer class="site-footer"><div><span class="brand-mark">Y</span><strong>Yuzora</strong></div><p>${langSpan("把 agents、遠端與資料，收進同一張桌面。", "Agents, remotes and data—inside one desktop.")}</p><div><a href="../showcase/">Design Lab</a><a href="https://github.com/NakiriYuuzu/Yuzora" target="_blank" rel="noreferrer">GitHub</a></div></footer>
  `;

  applyLanguage(language);
  bindInteractions();
  initNativeMotion();
  initLibraryMotion();
  initPlatformHint();
}

function applyLanguage(nextLanguage) {
  language = nextLanguage;
  localStorage.setItem("yuzora-showcase-language", language);
  document.documentElement.lang = language === "zh" ? "zh-Hant" : "en";
  document.querySelectorAll("[data-lang]").forEach((element) => {
    element.hidden = element.dataset.lang !== language;
  });
  document.querySelectorAll("[data-set-lang]").forEach((button) => {
    button.setAttribute("aria-pressed", button.dataset.setLang === language ? "true" : "false");
  });
}

function bindInteractions() {
  cleanupFns.forEach((fn) => fn());
  cleanupFns = [];

  document.querySelectorAll("[data-set-lang]").forEach((button) => {
    const handler = () => applyLanguage(button.dataset.setLang);
    button.addEventListener("click", handler);
    cleanupFns.push(() => button.removeEventListener("click", handler));
  });

  const menuButton = document.querySelector(".menu-toggle");
  const nav = document.querySelector(".site-nav");
  const toggleMenu = () => {
    const open = menuButton.getAttribute("aria-expanded") === "true";
    menuButton.setAttribute("aria-expanded", String(!open));
    menuButton.innerHTML = icon(open ? "menu" : "close");
    nav.classList.toggle("is-open", !open);
    document.body.classList.toggle("menu-open", !open);
  };
  menuButton?.addEventListener("click", toggleMenu);
  cleanupFns.push(() => menuButton?.removeEventListener("click", toggleMenu));
  nav?.querySelectorAll("a").forEach((link) => link.addEventListener("click", () => {
    if (nav.classList.contains("is-open")) toggleMenu();
  }));

  document.querySelectorAll('a[href^="#"]').forEach((anchor) => {
    anchor.addEventListener("click", (event) => {
      const target = document.querySelector(anchor.getAttribute("href"));
      if (!target) return;
      if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
      event.preventDefault();
      target.scrollIntoView({ behavior: "smooth", block: "start" });
    });
  });

  if (window.matchMedia("(pointer:fine)").matches && !window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
    document.querySelectorAll(".magnetic").forEach((element) => {
      const move = (event) => {
        const rect = element.getBoundingClientRect();
        element.style.setProperty("--mx", `${(event.clientX - rect.left - rect.width / 2) * 0.12}px`);
        element.style.setProperty("--my", `${(event.clientY - rect.top - rect.height / 2) * 0.12}px`);
      };
      const leave = () => {
        element.style.setProperty("--mx", "0px");
        element.style.setProperty("--my", "0px");
      };
      element.addEventListener("pointermove", move);
      element.addEventListener("pointerleave", leave);
      cleanupFns.push(() => { element.removeEventListener("pointermove", move); element.removeEventListener("pointerleave", leave); });
    });
  }
}

function initNativeMotion() {
  const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  document.documentElement.classList.add("js");
  requestAnimationFrame(() => document.documentElement.classList.add("is-ready"));

  const progress = document.querySelector(".scroll-progress span");
  const onScroll = () => {
    const max = document.documentElement.scrollHeight - window.innerHeight;
    const value = max > 0 ? window.scrollY / max : 0;
    document.documentElement.style.setProperty("--page-progress", value.toFixed(4));
    if (progress) progress.style.transform = `scaleX(${value})`;
  };
  onScroll();
  window.addEventListener("scroll", onScroll, { passive: true });
  cleanupFns.push(() => window.removeEventListener("scroll", onScroll));

  const revealObserver = new IntersectionObserver((entries) => {
    entries.forEach((entry) => {
      if (entry.isIntersecting) entry.target.classList.add("in-view");
    });
  }, { rootMargin: "0px 0px -12%", threshold: 0.12 });
  document.querySelectorAll(".reveal").forEach((element) => revealObserver.observe(element));
  cleanupFns.push(() => revealObserver.disconnect());

  const productObserver = new IntersectionObserver((entries) => {
    const visible = entries.filter((entry) => entry.isIntersecting).sort((a, b) => b.intersectionRatio - a.intersectionRatio)[0];
    if (!visible) return;
    const id = visible.target.dataset.product;
    document.body.dataset.activeProduct = id;
    document.querySelectorAll("[data-rail-target], [data-orbit-target]").forEach((node) => {
      const match = node.dataset.railTarget === id || node.dataset.orbitTarget === id;
      node.classList.toggle("is-active", match);
    });
  }, { threshold: [0.2, 0.45, 0.7], rootMargin: "-25% 0px -35%" });
  document.querySelectorAll(".product-step").forEach((section) => productObserver.observe(section));
  cleanupFns.push(() => productObserver.disconnect());

  const videoObserver = new IntersectionObserver((entries) => {
    entries.forEach((entry) => {
      const video = entry.target;
      if (entry.isIntersecting) {
        if (!video.src && video.dataset.src) video.src = video.dataset.src;
        video.play().catch(() => {});
      } else {
        video.pause();
      }
    });
  }, { rootMargin: "180px 0px", threshold: 0.15 });
  document.querySelectorAll("video[data-src]").forEach((video) => videoObserver.observe(video));
  cleanupFns.push(() => videoObserver.disconnect());

  if (!reduceMotion) initOrbitLoop();
}

function initOrbitLoop() {
  let raf = 0;
  const start = performance.now();
  const tick = (now) => {
    const elapsed = (now - start) / 1000;
    document.documentElement.style.setProperty("--orbit-time", `${elapsed}`);
    raf = requestAnimationFrame(tick);
  };
  raf = requestAnimationFrame(tick);
  cleanupFns.push(() => cancelAnimationFrame(raf));

  const scene = document.querySelector(".hero-scene-motion");
  if (!scene || !window.matchMedia("(pointer:fine)").matches) return;
  const move = (event) => {
    const rect = scene.getBoundingClientRect();
    const x = ((event.clientX - rect.left) / rect.width - 0.5) * 2;
    const y = ((event.clientY - rect.top) / rect.height - 0.5) * 2;
    scene.style.setProperty("--pointer-x", x.toFixed(3));
    scene.style.setProperty("--pointer-y", y.toFixed(3));
  };
  scene.addEventListener("pointermove", move);
  cleanupFns.push(() => scene.removeEventListener("pointermove", move));
}

async function initLibraryMotion() {
  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  try {
    const [animeModule, motionModule] = await Promise.all([
      import("https://cdn.jsdelivr.net/npm/animejs@4.5.0/+esm"),
      import("https://cdn.jsdelivr.net/npm/motion@12.43.0/+esm"),
    ]);
    animeApi = animeModule;
    motionApi = motionModule;
  } catch (error) {
    document.documentElement.classList.add("motion-fallback");
    return;
  }

  const { animate: animeAnimate, stagger } = animeApi;
  animeAnimate(".hero-kicker, .hero-line, .hero-thesis, .hero-actions, .hero-meta", {
    opacity: [0, 1],
    y: [28, 0],
    duration: 920,
    delay: stagger(95),
    ease: "outExpo",
  });
  animeAnimate(".float-token, .module-tile, .map-node, .editorial-note", {
    opacity: [0, 1],
    scale: [0.86, 1],
    duration: 860,
    delay: stagger(70, { start: 350, from: "center" }),
    ease: "outBack(1.4)",
  });
  animeAnimate(".route, .orbit-trace path", {
    strokeDashoffset: [520, 0],
    opacity: [0.15, 1],
    duration: 1800,
    delay: stagger(180, { start: 350 }),
    ease: "inOutCubic",
  });

  const { animate: motionAnimate, inView, scroll } = motionApi;
  inView(".product-step", (element) => {
    const copy = element.querySelector(".product-copy");
    const visual = element.querySelector(".product-visual-motion");
    motionAnimate(copy, { opacity: [0.35, 1], y: [36, 0] }, { duration: 0.75, ease: [0.22, 1, 0.36, 1] });
    motionAnimate(visual, { opacity: [0.45, 1], scale: [0.965, 1] }, { duration: 0.9, ease: [0.22, 1, 0.36, 1] });
  }, { margin: "0px 0px -18%" });

  const heroScene = document.querySelector(".hero-scene-motion");
  const hero = document.querySelector(".hero");
  if (heroScene && hero) {
    const heroAnimation = motionAnimate(heroScene, { y: [0, 70], scale: [1, 0.94], opacity: [1, 0.45] }, { ease: "linear" });
    scroll(heroAnimation, { target: hero, offset: ["start start", "end start"] });
  }

  const marquee = document.querySelector(".context-marquee > div");
  if (marquee) {
    const marqueeAnimation = motionAnimate(marquee, { x: ["0%", "-34%"] }, { ease: "linear" });
    scroll(marqueeAnimation, { target: document.querySelector(".context-section"), offset: ["start end", "end start"] });
  }
}

function initPlatformHint() {
  const platform = `${navigator.userAgentData?.platform || ""} ${navigator.platform || ""} ${navigator.userAgent || ""}`.toLowerCase();
  let target = null;
  if (/mac/.test(platform) && !/iphone|ipad/.test(platform)) target = "macos";
  else if (/win/.test(platform)) target = "windows";
  document.querySelectorAll("[data-platform]").forEach((card) => card.classList.toggle("is-recommended", card.dataset.platform === target));
}

renderSite();
