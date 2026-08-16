/* global document, getComputedStyle, localStorage, navigator, window */

import {
  PHASE_STOPS,
  activateExclusiveMedia,
  clamp,
  mediaForPhase,
  resolvePhase,
  sampleTrack,
} from "./exploded-view-model.js";

const root = document.documentElement;
root.classList.remove("no-js");
root.classList.add("js");

const motionQuery = window.matchMedia("(prefers-reduced-motion: reduce)");
const desktopQuery = window.matchMedia("(min-width: 1024px)");
let reducedMotion = motionQuery.matches;
let currentLang = "zh";
let activePhase = -1;
let activeMedia = null;
let framePending = false;

root.classList.toggle("reduced-motion", reducedMotion);

const I18N = {
  zh: {
    "meta.title": "Yuzora — ADE × HERDR",
    "meta.description":
      "Yuzora 將 Agent Development Environment 與 HERDR execution runtime 組成同一個桌面工作面。",
    skip: "跳到主要內容",
    "aria.primaryNav": "主要導覽",
    "aria.home": "Yuzora 首頁",
    "aria.language": "語言",
    "aria.explodedStory": "Yuzora 產品拆解視圖",
    "aria.chapterIndex": "產品拆解章節",
    "nav.story": "拆解",
    "nav.download": "下載",
    "hero.eyebrow": "OPEN-SOURCE ADE / HERDR RUNTIME",
    "hero.title": "一個環境，<br /><em>一套 runtime。</em>",
    "hero.lede": "Spaces、Agents 與終端頁面在同一個桌面工作面運轉。",
    "hero.download": "下載 Yuzora",
    "hero.source": "查看原始碼",
    "hero.caption":
      "Yuzora 將 HERDR Spaces、named Sessions 與 BSP terminal pages 投影成一個可操作工作面。",
    "story.eyebrow": "EXPLODED VIEW / SCROLL STUDY",
    "story.title": "拆開工作面，<br />看見 runtime 的邊界。",
    "story.lede":
      "每一層都來自真實 Yuzora 畫面。向下捲動，從 Space Rail 一路看到 HERDR terminal core。",
    "phase.locked.title": "完整工作面",
    "phase.locked.body": "ADE 與 HERDR 保持各自邊界，卻在同一個桌面表面運轉。",
    "phase.spaces.title": "Space Rail",
    "phase.spaces.body":
      "最左側 rail 投影 selected named Session 的 HERDR Spaces。",
    "phase.runtime.title": "Sessions 與 Agents",
    "phase.runtime.body":
      "ADE sidebar 整理 named Sessions、Attention 與 Agents 狀態。",
    "phase.core.title": "HERDR terminal core",
    "phase.core.body":
      "每個 page 對應 HERDR tab；BSP panes 遞迴呈現，Inspector 維持唯讀。",
    "phase.remote.title": "SSH 與資料庫",
    "phase.remote.body": "遠端檔案與 SQL 結果留在同一個可追蹤工作面。",
    "phase.git.title": "Terminal 與 Git",
    "phase.git.body": "編輯、歷史、diff 與 shell 不需要切換到另一個工具。",
    "phase.final.title": "一個環境，一套 runtime。",
    "phase.final.body":
      "Yuzora 負責 ADE 工作面；HERDR 負責 execution 與 terminal runtime。",
    "phase.final.lock": "ADE × HERDR / LOCKED",
    "media.active": "ACTIVE SURFACE",
    "proof.title": "介面是投影，runtime 是權威。",
    "proof.body":
      "Yuzora 組織可見工作面；HERDR 保持 named Session、Space、tab 與 terminal topology。",
    "proof.ade.title": "ADE surface",
    "proof.ade.body": "導覽、頁面、遠端與資料工具",
    "proof.herdr.body": "Spaces、tabs、BSP panes 與 terminal state",
    "proof.boundary.title": "Explicit boundary",
    "proof.boundary.body": "只釋放 Yuzora connector，不暗示終止 runtime",
    "visual.runtime": "Yuzora ADE 與 HERDR runtime 工作面",
    "visual.inspector": "Yuzora 唯讀 Agent Inspector",
    "visual.remoteDb": "Yuzora SSH 與資料庫工作面",
    "visual.terminalGit": "Yuzora Terminal 與 Git 工作面",
    "video.adeHerdr": "ADE 與 HERDR runtime 示範",
    "video.remoteDb": "SSH 與資料庫功能示範",
    "video.terminalGit": "Terminal 與 Git 功能示範",
    "dl.title": "選你的平台，開始使用。",
    "dl.lede": "所有版本由 GitHub Actions 建置並發佈於 GitHub Releases。",
    "dl.device.macos": "已辨識：macOS · Universal",
    "dl.device.windows": "已辨識：Windows · x64",
    "dl.device.unsupported": "僅支援 macOS 與 Windows 桌面裝置",
    "dl.device.unsupportedArchitecture": "此裝置架構尚未提供安裝檔",
    "dl.device.unknown": "請從下方選擇你的平台",
    "dl.recommended": "適合此裝置",
    "dl.other": "其他安裝格式與歷史版本見",
    "dl.note": "從原始碼建置：",
    "footer.tagline": "夕空下的 agent development environment",
    "footer.source": "原始碼",
    "footer.issues": "回報問題",
    "footer.releases": "所有版本",
  },
  en: {
    "meta.title": "Yuzora — ADE × HERDR",
    "meta.description":
      "Yuzora combines an Agent Development Environment with the HERDR execution runtime on one desktop surface.",
    skip: "Skip to main content",
    "aria.primaryNav": "Primary navigation",
    "aria.home": "Yuzora home",
    "aria.language": "Language",
    "aria.explodedStory": "Yuzora exploded product view",
    "aria.chapterIndex": "Exploded view chapters",
    "nav.story": "Exploded view",
    "nav.download": "Download",
    "hero.eyebrow": "OPEN-SOURCE ADE / HERDR RUNTIME",
    "hero.title": "One environment.<br /><em>One runtime.</em>",
    "hero.lede":
      "Spaces, agents and terminal pages operate on one desktop surface.",
    "hero.download": "Download Yuzora",
    "hero.source": "View source",
    "hero.caption":
      "Yuzora projects HERDR Spaces, named Sessions and BSP terminal pages into one operable surface.",
    "story.eyebrow": "EXPLODED VIEW / SCROLL STUDY",
    "story.title": "Pull the surface apart.<br />See the runtime boundary.",
    "story.lede":
      "Every layer comes from the real Yuzora interface. Scroll from the Space Rail to the HERDR terminal core.",
    "phase.locked.title": "The complete surface",
    "phase.locked.body":
      "ADE and HERDR keep distinct boundaries while operating on the same desktop surface.",
    "phase.spaces.title": "Space Rail",
    "phase.spaces.body":
      "The far-left rail projects HERDR Spaces from the selected named Session.",
    "phase.runtime.title": "Sessions and Agents",
    "phase.runtime.body":
      "The ADE sidebar organizes named Sessions, Attention and Agent state.",
    "phase.core.title": "HERDR terminal core",
    "phase.core.body":
      "Each page maps to a HERDR tab; BSP panes render recursively and Inspector stays read-only.",
    "phase.remote.title": "SSH and databases",
    "phase.remote.body":
      "Remote files and SQL results stay on the same traceable work surface.",
    "phase.git.title": "Terminal and Git",
    "phase.git.body":
      "Editing, history, diffs and shell work no longer require another tool window.",
    "phase.final.title": "One environment. One runtime.",
    "phase.final.body":
      "Yuzora owns the ADE surface; HERDR owns execution and terminal runtime.",
    "phase.final.lock": "ADE × HERDR / LOCKED",
    "media.active": "ACTIVE SURFACE",
    "proof.title": "The interface projects. The runtime remains authoritative.",
    "proof.body":
      "Yuzora organizes the visible surface; HERDR retains named Session, Space, tab and terminal topology.",
    "proof.ade.title": "ADE surface",
    "proof.ade.body": "Navigation, pages, remotes and data tools",
    "proof.herdr.body": "Spaces, tabs, BSP panes and terminal state",
    "proof.boundary.title": "Explicit boundary",
    "proof.boundary.body":
      "Release Yuzora connectors without implying runtime termination",
    "visual.runtime": "Yuzora ADE and HERDR runtime work surface",
    "visual.inspector": "Yuzora read-only Agent Inspector",
    "visual.remoteDb": "Yuzora SSH and database work surface",
    "visual.terminalGit": "Yuzora Terminal and Git work surface",
    "video.adeHerdr": "ADE and HERDR runtime demo",
    "video.remoteDb": "SSH and database feature demo",
    "video.terminalGit": "Terminal and Git feature demo",
    "dl.title": "Pick your platform and get started.",
    "dl.lede":
      "Every build is produced by GitHub Actions and published on GitHub Releases.",
    "dl.device.macos": "Detected: macOS · Universal",
    "dl.device.windows": "Detected: Windows · x64",
    "dl.device.unsupported": "Available for macOS and Windows desktop devices",
    "dl.device.unsupportedArchitecture":
      "No installer is available for this device architecture yet",
    "dl.device.unknown": "Choose your platform below",
    "dl.recommended": "Recommended",
    "dl.other": "Other installer formats and past versions live on",
    "dl.note": "Build from source:",
    "footer.tagline": "agent development under the evening sky",
    "footer.source": "Source",
    "footer.issues": "Issues",
    "footer.releases": "Releases",
  },
};

const VISUALS = {
  runtime: (lang) => `assets/ade-herdr-runtime-${lang}.png`,
  inspector: (lang) => `assets/ade-herdr-inspector-${lang}.png`,
  "remote-db": (lang) => `assets/remote-db-${lang}.png`,
  "terminal-git": (lang) => `assets/terminal-git-${lang}.png`,
};

function pauseAllLiveVideos() {
  document
    .querySelectorAll("[data-live-video]")
    .forEach((video) => video.pause());
}

function updateLocalizedMedia(lang) {
  document.querySelectorAll("[data-visual]").forEach((image) => {
    const resolver = VISUALS[image.dataset.visual];
    if (resolver) image.src = resolver(lang);
  });

  document.querySelectorAll("video[data-video]").forEach((video) => {
    video.pause();
    const source = video.querySelector("source");
    const nextSource = `assets/${video.dataset.video}-${lang}.mp4`;
    const posterName =
      video.dataset.video === "ade-herdr"
        ? `ade-herdr-runtime-${lang}.png`
        : `${video.dataset.video}-${lang}.png`;
    video.poster = `assets/${posterName}`;
    if (source?.getAttribute("src") !== nextSource) {
      source?.setAttribute("src", nextSource);
      video.load();
    }
  });
}

function setLang(lang) {
  currentLang = I18N[lang] ? lang : "zh";
  const dict = I18N[currentLang];
  document.querySelectorAll("[data-i18n]").forEach((element) => {
    const value = dict[element.dataset.i18n];
    if (value !== undefined) element.textContent = value;
  });
  document.querySelectorAll("[data-i18n-html]").forEach((element) => {
    const value = dict[element.dataset.i18nHtml];
    if (value !== undefined) element.innerHTML = value;
  });
  document.querySelectorAll("[data-i18n-alt]").forEach((element) => {
    const value = dict[element.dataset.i18nAlt];
    if (value !== undefined) element.setAttribute("alt", value);
  });
  document.querySelectorAll("[data-i18n-aria-label]").forEach((element) => {
    const value = dict[element.dataset.i18nAriaLabel];
    if (value !== undefined) element.setAttribute("aria-label", value);
  });
  document.querySelectorAll("[data-lang]").forEach((button) => {
    button.setAttribute(
      "aria-pressed",
      String(button.dataset.lang === currentLang),
    );
  });
  document.documentElement.lang = currentLang === "zh" ? "zh-Hant" : "en";
  document.title = dict["meta.title"];
  document
    .querySelector('meta[name="description"]')
    ?.setAttribute("content", dict["meta.description"]);
  document
    .querySelector('meta[property="og:description"]')
    ?.setAttribute("content", dict["meta.description"]);
  document
    .querySelector('meta[property="og:image"]')
    ?.setAttribute("content", VISUALS.runtime(currentLang));
  activeMedia = null;
  updateLocalizedMedia(currentLang);
  try {
    localStorage.setItem("yuzora-lang", currentLang);
  } catch {
    // Storage may be unavailable in hardened or private browsing contexts.
  }
  scheduleStoryUpdate();
}

document.querySelectorAll("[data-lang]").forEach((button) => {
  button.addEventListener("click", () => setLang(button.dataset.lang));
});

let savedLang = null;
try {
  savedLang = localStorage.getItem("yuzora-lang");
} catch {
  // Keep language detection functional when storage is unavailable.
}
setLang(
  savedLang ||
    ((navigator.language || "").toLowerCase().startsWith("zh") ? "zh" : "en"),
);

const story = document.querySelector("[data-story]");
const storyLive = document.querySelector(".story-live");
const storyFallback = document.querySelector("[data-story-fallback]");
const layerElements = Object.fromEntries(
  [...document.querySelectorAll("[data-layer]")].map((element) => [
    element.dataset.layer,
    element,
  ]),
);
const phaseCopies = [...document.querySelectorAll("[data-phase-copy]")];
const phaseIndexes = [...document.querySelectorAll("[data-phase-index]")];
const connectors = [...document.querySelectorAll("[data-connector]")];
const phaseCounter = document.querySelector("[data-phase-counter]");
const progressBar = document.querySelector("[data-progress-bar]");
const lockMark = document.querySelector("[data-lock-mark]");
const mediaCode = document.querySelector("[data-media-code]");
const liveVideos = [...document.querySelectorAll("[data-live-video]")];

const tracks = {
  base: [
    [0, 0, 1, 0, 1],
    [0, 0, 0.98, 0, 0.34],
    [0, 0, 0.96, 0, 0.24],
    [0, 0, 0.94, 0, 0.18],
    [0, 0, 0.94, 0, 0.16],
    [0, 0, 0.94, 0, 0.14],
    [0, 0, 1.025, 0, 0],
  ],
  rail: [
    [0, 0, 1, 0, 0],
    [-34, -14, 1.08, -2, 1],
    [-27, -10, 1.03, -1, 1],
    [-18, -7, 1, 0, 0.72],
    [-22, -5, 0.98, 0, 0.42],
    [-20, -4, 0.98, 0, 0.34],
    [-2.5, 0, 1.025, 0, 0],
  ],
  sessions: [
    [0, 0, 1, 0, 0],
    [8, -3, 0.98, 0, 0.5],
    [31, -18, 1.06, 1, 1],
    [24, -13, 1, 0, 0.72],
    [18, -10, 0.98, 0, 0.4],
    [18, -9, 0.98, 0, 0.32],
    [-1, 0, 1.025, 0, 0],
  ],
  attention: [
    [0, 0, 1, 0, 0],
    [8, 5, 0.98, 0, 0.4],
    [34, 20, 1.06, -1, 1],
    [27, 15, 1, 0, 0.78],
    [20, 15, 0.98, 0, 0.44],
    [20, 14, 0.98, 0, 0.34],
    [-1, 1.5, 1.025, 0, 0],
  ],
  core: [
    [0, 0, 1, 0, 0],
    [5, 3, 0.98, 0, 0.48],
    [-3, 4, 0.97, 0, 0.64],
    [-8, 0, 1.08, 0, 1],
    [-4, -1, 0.99, 0, 0.48],
    [-4, -1, 0.99, 0, 0.4],
    [1, 0, 1.025, 0, 0],
  ],
  inspector: [
    [0, 0, 0.84, 0, 0],
    [0, 0, 0.84, 0, 0],
    [22, 8, 0.76, 1, 0.18],
    [29, 20, 0.75, 1, 1],
    [22, 14, 0.72, 0, 0.35],
    [22, 14, 0.72, 0, 0.25],
    [0, 0, 0.9, 0, 0],
  ],
  media: [
    [0, 0, 0.72, 0, 0],
    [0, 0, 0.72, 0, 0],
    [32, 18, 0.62, 1, 1],
    [34, 24, 0.6, 1, 1],
    [-34, 25, 0.62, -1, 1],
    [35, 24, 0.62, 1, 1],
    [0, 0, 0.72, 0, 0],
  ],
  final: [
    [0, 0, 0.96, 0, 0],
    [0, 0, 0.96, 0, 0],
    [0, 0, 0.96, 0, 0],
    [0, 0, 0.96, 0, 0],
    [0, 0, 0.96, 0, 0],
    [0, 0, 0.9, 0, 0],
    [0, 0, 1, 0, 1],
  ],
};

const mediaCodes = {
  "ade-herdr": "MEDIA / RUNTIME",
  "remote-db": "MEDIA / REMOTE DATA",
  "terminal-git": "MEDIA / TERMINAL + GIT",
};

function applyTrack(element, values) {
  if (!element) return;
  const [x, y, scale, rotate, opacity] = values;
  element.style.transform = `translate3d(${x}%, ${y}%, 0) scale(${scale}) rotate(${rotate}deg)`;
  element.style.opacity = String(opacity);
}

function setActiveMedia(mediaName, storyIsActive) {
  const nextMedia = storyIsActive ? mediaName : null;
  if (activeMedia === nextMedia) return;
  activeMedia = nextMedia;
  void activateExclusiveMedia(liveVideos, activeMedia, {
    shouldPlay: !reducedMotion && !document.hidden,
  });
  if (mediaCode && activeMedia) mediaCode.textContent = mediaCodes[activeMedia];
}

function renderPhase(phase, progress, storyIsActive) {
  if (activePhase !== phase) {
    activePhase = phase;
    phaseCopies.forEach((copy, index) => {
      const isActive = index === phase;
      copy.classList.toggle("is-active", isActive);
      copy.setAttribute("aria-hidden", String(!isActive));
    });
    phaseIndexes.forEach((item, index) => {
      const isActive = index === phase;
      item.classList.toggle("is-active", isActive);
      const button = item.querySelector("button");
      if (isActive) button?.setAttribute("aria-current", "step");
      else button?.removeAttribute("aria-current");
    });
    connectors.forEach((connector) =>
      connector.classList.toggle(
        "is-active",
        Number(connector.dataset.connector) === phase,
      ),
    );
  }
  if (phaseCounter)
    phaseCounter.textContent = `${String(phase + 1).padStart(2, "0")} / 07`;
  if (progressBar) progressBar.style.transform = `scaleX(${progress})`;
  connectors.forEach((connector) => {
    connector.style.opacity = connector.classList.contains("is-active")
      ? "1"
      : "0";
  });
  if (lockMark) {
    const lockOpacity = phase === 6 ? clamp((progress - 0.91) / 0.07) : 0;
    lockMark.style.opacity = String(lockOpacity);
    lockMark.style.transform = `translateY(${(1 - lockOpacity) * 8}px)`;
  }
  setActiveMedia(mediaForPhase(phase), storyIsActive);
}

function updateStory() {
  framePending = false;
  if (!story) return;
  const liveMode = desktopQuery.matches && !reducedMotion;
  storyLive?.setAttribute("aria-hidden", String(!liveMode));
  storyFallback?.setAttribute("aria-hidden", String(liveMode));
  if (!liveMode) {
    pauseAllLiveVideos();
    activeMedia = null;
    return;
  }

  const navHeight =
    Number.parseFloat(getComputedStyle(root).getPropertyValue("--nav-h")) || 68;
  const rect = story.getBoundingClientRect();
  const viewportHeight = window.innerHeight - navHeight;
  const scrollRange = Math.max(story.offsetHeight - viewportHeight, 1);
  const progress = clamp((navHeight - rect.top) / scrollRange);
  const storyIsActive =
    rect.top <= navHeight && rect.bottom >= window.innerHeight;
  Object.entries(tracks).forEach(([name, track]) =>
    applyTrack(layerElements[name], sampleTrack(track, progress)),
  );
  renderPhase(resolvePhase(progress), progress, storyIsActive);
}

function scheduleStoryUpdate() {
  if (framePending) return;
  framePending = true;
  window.requestAnimationFrame(updateStory);
}

window.addEventListener("scroll", scheduleStoryUpdate, { passive: true });
window.addEventListener("resize", scheduleStoryUpdate, { passive: true });
document.addEventListener("visibilitychange", () => {
  if (document.hidden) pauseAllLiveVideos();
  else {
    activeMedia = null;
    scheduleStoryUpdate();
  }
});

document.querySelectorAll("[data-phase-jump]").forEach((button) => {
  button.addEventListener("click", () => {
    if (!story || !desktopQuery.matches || reducedMotion) return;
    const phase = Number(button.dataset.phaseJump);
    const navHeight =
      Number.parseFloat(getComputedStyle(root).getPropertyValue("--nav-h")) ||
      68;
    const viewportHeight = window.innerHeight - navHeight;
    const scrollRange = Math.max(story.offsetHeight - viewportHeight, 1);
    window.scrollTo({
      top: story.offsetTop - navHeight + PHASE_STOPS[phase] * scrollRange,
      behavior: "smooth",
    });
  });
});

function handleMotionChange(event) {
  reducedMotion = event.matches;
  root.classList.toggle("reduced-motion", reducedMotion);
  pauseAllLiveVideos();
  activeMedia = null;
  scheduleStoryUpdate();
}

motionQuery.addEventListener?.("change", handleMotionChange);
desktopQuery.addEventListener?.("change", scheduleStoryUpdate);
document.fonts?.ready.then(scheduleStoryUpdate).catch(() => {});
document.querySelectorAll("video").forEach((video) => {
  video.addEventListener("loadedmetadata", scheduleStoryUpdate, { once: true });
  if (video.hasAttribute("data-live-video")) {
    video.addEventListener("canplay", () => {
      if (
        video.dataset.media === activeMedia &&
        !reducedMotion &&
        !document.hidden
      ) {
        video.play().catch(() => {});
      }
    });
  }
});
document
  .querySelectorAll("img")
  .forEach((image) =>
    image.addEventListener("load", scheduleStoryUpdate, { once: true }),
  );
scheduleStoryUpdate();
