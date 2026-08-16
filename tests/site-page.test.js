/* global DOMParser */

import { existsSync, readFileSync } from "node:fs"
import { dirname, resolve } from "node:path"
import process from "node:process"

import { describe, expect, it } from "vitest"

const root = process.cwd()
const htmlSource = readFileSync(resolve(root, "site/index.html"), "utf8")
const cssSource = readFileSync(resolve(root, "site/styles.css"), "utf8")
const appSource = readFileSync(resolve(root, "site/app.js"), "utf8")
const page = new DOMParser().parseFromString(htmlSource, "text/html")

function localReferenceExists(reference, baseFile = "site/index.html") {
  if (!reference || reference.startsWith("#") || /^(?:https?:|data:|mailto:)/.test(reference)) {
    return true
  }
  return existsSync(resolve(root, dirname(baseFile), reference))
}

describe("GitHub Pages product page", () => {
  it("publishes the static stylesheet and one application module", () => {
    expect(page.querySelector('link[href="./styles.css"]')).not.toBeNull()
    expect(page.querySelector('script[type="module"][src="./app.js"]')).not.toBeNull()
    expect(appSource).toContain('from "./downloads.js"')
  })

  it("contains the current product narrative and download surfaces", () => {
    expect(page.querySelectorAll(".loop-card")).toHaveLength(3)
    expect(page.querySelectorAll("video[data-vstem]")).toHaveLength(3)
    expect(page.querySelectorAll(".bento-card")).toHaveLength(5)
    expect(page.querySelectorAll(".flow-node")).toHaveLength(2)
    expect(page.querySelectorAll("[data-platform-download]")).toHaveLength(2)
    for (const id of ["main", "features", "boundary", "download"]) {
      expect(page.getElementById(id), id).not.toBeNull()
    }
  })

  it("keeps no-JavaScript and reduced-motion content readable", () => {
    expect(page.documentElement.classList.contains("no-js")).toBe(true)
    expect(appSource).toContain('classList.remove("no-js")')
    expect(cssSource).toMatch(/\.no-js \.reveal\s*\{[^}]*opacity:\s*1/s)
    expect(cssSource).toContain("@media (prefers-reduced-motion: reduce)")
  })

  it("ships system-aware light and dark themes with readable secondary text", () => {
    expect(htmlSource).toContain('prefers-color-scheme: dark')
    expect(appSource).toContain('const THEME_KEY = "yuzora-theme"')
    expect(cssSource).toContain('html[data-theme="dark"]')
    expect(cssSource).toContain("--muted: #687083")
    expect(cssSource).toContain("--muted: #858ba4")
    expect(page.querySelector("#theme-toggle[data-i18n-aria-label='nav.theme']")).not.toBeNull()
  })

  it("uses the current desktop application logo for favicon and branding", () => {
    expect(page.querySelector('link[rel="icon"][href="assets/yuzora-icon.png"]')).not.toBeNull()
    expect(page.querySelectorAll('img[src="assets/yuzora-icon.png"]')).toHaveLength(2)
    expect(
      readFileSync(resolve(root, "site/assets/yuzora-icon.png")).equals(
        readFileSync(resolve(root, "src-tauri/icons/128x128@2x.png")),
      ),
    ).toBe(true)
  })

  it("provides both localized dictionary entries for every markup key", () => {
    const attributes = [
      "data-i18n",
      "data-i18n-html",
      "data-i18n-alt",
      "data-i18n-placeholder",
      "data-i18n-aria-label",
    ]
    const keys = new Set(
      attributes.flatMap((attribute) =>
        [...page.querySelectorAll(`[${attribute}]`)].map((element) =>
          element.getAttribute(attribute),
        ),
      ),
    )

    for (const key of keys) {
      const escaped = key.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
      expect(appSource.match(new RegExp(`"${escaped}"\\s*:`, "g"))?.length, key).toBe(2)
    }
  })

  it("references only local files present in the Pages artifact", () => {
    const references = [...page.querySelectorAll("[href], [src], [poster]")].flatMap((element) =>
      ["href", "src", "poster"]
        .map((attribute) => element.getAttribute(attribute))
        .filter(Boolean),
    )
    for (const reference of references) {
      expect(localReferenceExists(reference), reference).toBe(true)
    }
  })

  it("ships every dynamically selected zh/en video and still", () => {
    const videoStems = [...page.querySelectorAll("video[data-vstem]")].map(
      (video) => video.dataset.vstem,
    )
    const stillStems = new Set([
      ...[...page.querySelectorAll("img[data-imgstem]")].map((image) => image.dataset.imgstem),
      ...[...page.querySelectorAll("video[data-poster-stem]")].map(
        (video) => video.dataset.posterStem,
      ),
    ])
    expect(videoStems).toEqual(["ade-herdr", "remote-db", "terminal-git"])
    expect([...stillStems]).not.toContain("ade-herdr-inspector")

    for (const lang of ["zh", "en"]) {
      for (const stem of videoStems) {
        expect(existsSync(resolve(root, `site/assets/${stem}-${lang}.mp4`))).toBe(true)
      }
      for (const stem of stillStems) {
        expect(existsSync(resolve(root, `site/assets/${stem}-${lang}.png`))).toBe(true)
      }
    }
  })

  it("provides every statically addressed application element", () => {
    const ids = new Set([...appSource.matchAll(/getElementById\("([^"]+)"\)/g)].map((match) => match[1]))
    for (const id of ids) {
      expect(page.getElementById(id), id).not.toBeNull()
    }
  })

  it("localizes metadata and accessible labels with the selected language", () => {
    for (const key of ["meta.title", "meta.description", "meta.ogDescription"]) {
      const escaped = key.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
      expect(appSource.match(new RegExp(`"${escaped}"\\s*:`, "g"))?.length, key).toBe(2)
    }
    expect(appSource).toContain('meta[name="description"]')
    expect(appSource).toContain('meta[property="og:description"]')
    expect(appSource).toContain('meta[property="og:image"]')
    expect(page.querySelectorAll("video[data-i18n-aria-label]")).toHaveLength(3)
    expect(page.querySelector("#gh-stars[data-i18n-aria-label='github.stars']")).not.toBeNull()
    expect(appSource).toContain('I18N[currentLang]["github.stars"]')
    expect(appSource).toContain('replace("{{count}}", starsNum.textContent)')
    expect(page.querySelector("nav.site-nav[data-i18n-aria-label='a11y.primaryNav']")).not.toBeNull()
    expect(page.querySelector("nav.footer-links[data-i18n-aria-label='a11y.footerNav']")).not.toBeNull()
  })

  it("keeps videos bounded by viewport and document visibility", () => {
    expect(appSource).toContain('dataset.inView = String(entry.isIntersecting)')
    expect(appSource).toContain('document.addEventListener("visibilitychange", syncVideoPlayback)')
    expect(appSource).toContain("!document.hidden")
    expect(appSource).toContain("prefers-reduced-motion: reduce")
  })

  it("keeps the actionable palette result keyboard-operable", () => {
    expect(appSource).toContain('const button = document.createElement("button")')
    expect(appSource).toContain('button.type = "button"')
    expect(appSource).toContain('button.addEventListener("click", toggleLang)')
  })

  it("keeps the platform and interaction contracts wired", () => {
    expect(appSource).toContain("initDownloadExperience(navigator, document)")
    expect(appSource).toContain("IntersectionObserver")
    expect(appSource).toContain("https://api.github.com/repos/NakiriYuuzu/Yuzora")
    expect(page.querySelector("[data-platform-download='macos']")).not.toBeNull()
    expect(page.querySelector("[data-platform-download='windows']")).not.toBeNull()
    expect(page.querySelector("[data-platform-download='linux']")).toBeNull()
  })

  it("removes retired exploded-view, showcase, model-lab, and inspector artifacts", () => {
    const retired = [
      "site/exploded-view.js",
      "site/exploded-view-model.js",
      "site/showcase/index.html",
      "site/claude-opus-4-6-thinking/index.html",
      "site/gemini-3.7-flash-high/index.html",
      "site/gpt-5.6-luna-fast/index.html",
      "site/gpt-5.6-sol-fast/index.html",
      "site/gpt-5.6-terra-fast/index.html",
      "site/grok-4.6/index.html",
      "site/assets/ade-herdr-inspector-zh.png",
      "site/assets/ade-herdr-inspector-en.png",
    ]
    for (const path of retired) {
      expect(existsSync(resolve(root, path)), path).toBe(false)
    }
    expect(htmlSource).not.toMatch(/Agent Inspector|exploded-view|site\/showcase/)
  })
})
