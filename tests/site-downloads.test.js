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

  it.each([
    {
      name: "Windows",
      navigator: { userAgentData: { platform: "Windows" } },
      platform: "windows",
      url: "https://github.com/NakiriYuuzu/Yuzora/releases/latest/download/Yuzora-windows-x64-setup.exe",
    },
    {
      name: "Linux",
      navigator: { platform: "Linux x86_64" },
      platform: "linux",
      url: "https://github.com/NakiriYuuzu/Yuzora/releases/latest/download/Yuzora-linux-x86_64.AppImage",
    },
  ])("selects the primary installer for $name", ({ navigator, platform, url }) => {
    expect(resolveDownloadTarget(navigator)).toMatchObject({
      status: "supported",
      platform,
      url,
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
  ])("does not offer a desktop installer to $name", ({ navigator }) => {
    expect(resolveDownloadTarget(navigator)).toMatchObject({
      status: "unsupported",
      platform: null,
      url: null,
    })
  })

  it.each([
    {
      name: "Windows on ARM",
      navigator: {
        userAgentData: { platform: "Windows", architecture: "arm", bitness: "64" },
      },
      platform: "windows",
    },
    {
      name: "32-bit Linux",
      navigator: {
        userAgentData: { platform: "Linux", architecture: "x86", bitness: "32" },
      },
      platform: "linux",
    },
  ])("does not offer an incompatible x64 installer to $name", ({ navigator, platform }) => {
    expect(resolveDownloadTarget(navigator)).toMatchObject({
      status: "unsupported-architecture",
      platform,
      url: null,
    })
  })

  it("uses legacy navigator fields to reject a known ARM Linux device", () => {
    expect(resolveDownloadTarget({ platform: "Linux armv8l" })).toMatchObject({
      status: "unsupported-architecture",
      platform: "linux",
      url: null,
    })
  })

  it("uses legacy navigator fields to reject a known 32-bit Linux device", () => {
    expect(resolveDownloadTarget({ platform: "Linux i686" })).toMatchObject({
      status: "unsupported-architecture",
      platform: "linux",
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
      <a data-platform-download="linux"><span data-recommended-badge hidden>Recommended</span></a>
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

    await initDownloadExperience({ userAgentData: { platform: "Linux" } }, page)

    expect(page.querySelector("#primary-download")?.getAttribute("href")).toBe(
      "https://github.com/NakiriYuuzu/Yuzora/releases/latest/download/Yuzora-linux-x86_64.AppImage",
    )
    expect(
      page.querySelector("[data-platform-download='linux']")?.classList.contains("is-recommended"),
    ).toBe(true)
    expect(page.querySelector("[data-device-message='linux']")?.hasAttribute("hidden")).toBe(false)
    expect(
      [...page.querySelectorAll('script[type="module"]')].some((script) =>
        script.textContent?.includes('from "./downloads.js"'),
      ),
    ).toBe(true)
  })

  it("keeps manual platform selection usable before JavaScript runs", () => {
    const html = readFileSync(resolve(process.cwd(), "site/index.html"), "utf8")
    const page = new DOMParser().parseFromString(html, "text/html")

    expect(page.querySelector("#primary-download")?.getAttribute("href")).toBe("#download")
    expect(page.querySelector("#download-device-note")?.getAttribute("data-status")).toBe("unknown")
    expect(page.querySelector("[data-device-message='unknown']")?.hasAttribute("hidden")).toBe(false)
  })
})
