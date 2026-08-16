/* global DOMParser, document */

import { readFileSync } from "node:fs"
import { resolve } from "node:path"
import process from "node:process"

import { describe, expect, it, vi } from "vitest"

import {
  applyDownloadTarget,
  detectDownloadTarget,
  initDownloadExperience,
  resolveDownloadTarget,
} from "../site/downloads.js"

describe("GitHub Pages platform download selection", () => {
  it("selects the universal DMG for macOS", () => {
    const target = resolveDownloadTarget({
      userAgentData: { platform: "macOS" },
      userAgent: "Mozilla/5.0",
    })

    expect(target).toMatchObject({
      status: "supported",
      platform: "macos",
      url: "https://github.com/NakiriYuuzu/Yuzora/releases/latest/download/Yuzora-macos-universal.dmg",
    })
  })

  it("selects the x64 installer for Windows", () => {
    expect(resolveDownloadTarget({ userAgentData: { platform: "Windows" } })).toMatchObject({
      status: "supported",
      platform: "windows",
      url: "https://github.com/NakiriYuuzu/Yuzora/releases/latest/download/Yuzora-windows-x64-setup.exe",
    })
  })

  it.each([
    {
      name: "Android",
      navigator: {
        userAgentData: { mobile: true, platform: "Android" },
        userAgent: "Mozilla/5.0 (Linux; Android 16)",
      },
    },
    {
      name: "ChromeOS",
      navigator: { userAgent: "Mozilla/5.0 (X11; CrOS x86_64 16093.45.0)" },
    },
    {
      name: "iPadOS",
      navigator: { platform: "MacIntel", maxTouchPoints: 5 },
    },
    {
      name: "Linux desktop",
      navigator: { platform: "Linux x86_64" },
    },
  ])("does not offer a desktop installer to $name", ({ navigator }) => {
    expect(resolveDownloadTarget(navigator)).toMatchObject({
      status: "unsupported",
      platform: null,
      url: null,
    })
  })

  it("does not offer the x64 installer to Windows on ARM", () => {
    expect(
      resolveDownloadTarget({
        userAgentData: { platform: "Windows", architecture: "arm", bitness: "64" },
      }),
    ).toMatchObject({
      status: "unsupported-architecture",
      platform: "windows",
      url: null,
    })
  })

  it("uses high-entropy architecture data before choosing an installer", async () => {
    const getHighEntropyValues = vi.fn().mockResolvedValue({
      architecture: "arm",
      bitness: "64",
    })

    const target = await detectDownloadTarget({
      userAgentData: {
        platform: "Windows",
        mobile: false,
        getHighEntropyValues,
      },
    })

    expect(getHighEntropyValues).toHaveBeenCalledWith(["architecture", "bitness"])
    expect(target).toMatchObject({
      status: "unsupported-architecture",
      platform: "windows",
      url: null,
    })
  })

  it("falls back to platform detection when high-entropy data is unavailable", async () => {
    const target = await detectDownloadTarget({
      userAgentData: {
        platform: "Windows",
        mobile: false,
        getHighEntropyValues: vi.fn().mockRejectedValue(new Error("not available")),
      },
    })

    expect(target).toMatchObject({
      status: "supported",
      platform: "windows",
      url: "https://github.com/NakiriYuuzu/Yuzora/releases/latest/download/Yuzora-windows-x64-setup.exe",
    })
  })

  it("turns the primary CTA into the detected installer and marks one recommended row", () => {
    document.body.innerHTML = `
      <a id="primary-download" href="#download">Download</a>
      <span id="download-device-note" data-status="detecting">
        <span data-device-message="macos">macOS</span>
        <span data-device-message="unknown">Unknown</span>
      </span>
      <a data-platform-download="macos"><span data-recommended-badge hidden>Recommended</span></a>
      <a data-platform-download="windows"><span data-recommended-badge hidden>Recommended</span></a>
    `

    const target = resolveDownloadTarget({ userAgentData: { platform: "macOS" } })
    applyDownloadTarget(target, document)

    expect(document.querySelector("#primary-download")?.getAttribute("href")).toBe(
      "https://github.com/NakiriYuuzu/Yuzora/releases/latest/download/Yuzora-macos-universal.dmg",
    )
    expect(document.querySelector("[data-platform-download='macos']")).toHaveClass(
      "is-recommended",
    )
    expect(
      document.querySelector("[data-platform-download='macos']")?.getAttribute("aria-current"),
    ).toBe("true")
    expect(document.querySelector("[data-platform-download='windows']")).not.toHaveClass(
      "is-recommended",
    )
    expect(document.querySelector("[data-device-message='macos']")).not.toHaveAttribute("hidden")
    expect(document.querySelector("[data-device-message='unknown']")).toHaveAttribute("hidden")
  })

  it("initializes the download experience from the browser navigator", async () => {
    document.body.innerHTML = `
      <a id="primary-download" href="#download">Download</a>
      <span id="download-device-note">
        <span data-device-message="windows">Windows</span>
        <span data-device-message="unknown">Unknown</span>
      </span>
      <a data-platform-download="windows"><span data-recommended-badge hidden>Recommended</span></a>
    `

    await initDownloadExperience({ userAgentData: { platform: "Windows" } }, document)

    expect(document.querySelector("#primary-download")?.getAttribute("href")).toBe(
      "https://github.com/NakiriYuuzu/Yuzora/releases/latest/download/Yuzora-windows-x64-setup.exe",
    )
    expect(document.querySelector("[data-device-message='windows']")).not.toHaveAttribute("hidden")
  })

  it("wires the published Pages markup to the platform-aware download flow", async () => {
    const html = readFileSync(resolve(process.cwd(), "site/index.html"), "utf8")
    const page = new DOMParser().parseFromString(html, "text/html")

    await initDownloadExperience({ userAgentData: { platform: "Windows" } }, page)

    expect(page.querySelector("#primary-download")?.getAttribute("href")).toBe(
      "https://github.com/NakiriYuuzu/Yuzora/releases/latest/download/Yuzora-windows-x64-setup.exe",
    )
    expect(
      page.querySelector("[data-platform-download='windows']")?.classList.contains("is-recommended"),
    ).toBe(true)
    expect(page.querySelector("[data-device-message='windows']")?.hasAttribute("hidden")).toBe(false)
    expect(page.querySelector("[data-platform-download='linux']")).toBeNull()
    expect(page.querySelector('script[type="module"][src="./app.js"]')).not.toBeNull()
    expect(readFileSync(resolve(process.cwd(), "site/app.js"), "utf8")).toContain(
      'from "./downloads.js"',
    )
  })

  it("keeps manual platform selection usable before JavaScript runs", () => {
    const html = readFileSync(resolve(process.cwd(), "site/index.html"), "utf8")
    const page = new DOMParser().parseFromString(html, "text/html")

    expect(page.querySelector("#primary-download")?.getAttribute("href")).toBe("#download")
    expect(page.querySelector("#download-device-note")?.getAttribute("data-status")).toBe("unknown")
    expect(page.querySelector("[data-device-message='unknown']")?.hasAttribute("hidden")).toBe(false)
  })
})
