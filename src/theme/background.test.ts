import { afterEach, describe, expect, it } from "vitest"

import {
  MAX_SAVED_GRADIENTS,
  addSavedGradient,
  applyBackgroundImage,
  applyBackgroundPreference,
  gradientCorners,
  gradientCustomProperties,
  imageVeilPercent,
  isBackgroundSource,
  padColor,
  padPosition,
} from "./background"

afterEach(() => {
  document.documentElement.removeAttribute("style")
  delete document.documentElement.dataset.background
})

describe("gradient corners", () => {
  it("spreads one, two and three colors over the four glows", () => {
    expect(gradientCorners(["#111111"])).toEqual(["#111111", "#111111", "#111111", "#111111"])
    expect(gradientCorners(["#111111", "#333333"])).toEqual(["#111111", "#333333", "#333333", "#111111"])
    expect(gradientCorners(["#000000", "#333333", "#ffffff"])).toEqual(["#000000", "#333333", "#ffffff", "#808080"])
  })
})

describe("gradient custom properties", () => {
  it("derives both theme variants and scales them with intensity", () => {
    const subtle = gradientCustomProperties({ colors: ["#ff0000"], intensity: 0 })
    const vivid = gradientCustomProperties({ colors: ["#ff0000"], intensity: 100 })
    expect(subtle["--yz-glow-a-light"]).toBe("color-mix(in oklab, #ff0000 8%, #f6f5ef)")
    expect(vivid["--yz-glow-a-light"]).toBe("color-mix(in oklab, #ff0000 52%, #f6f5ef)")
    expect(vivid["--yz-glow-a-dark"]).toBe("color-mix(in oklab, #ff0000 36%, #100f14)")
    expect(vivid["--yz-base-light"]).toMatch(/^color-mix\(in oklab, #ff0000 \d+%, #f6f5ef\)$/)
    expect(vivid["--yz-base-dark"]).toMatch(/^color-mix\(in oklab, #ff0000 \d+%, #100f14\)$/)
  })

  it("applies a gradient inline and restores the accent glows when switched back", () => {
    const root = document.documentElement
    applyBackgroundPreference("gradient", { colors: ["#3ddc97", "#46a0ff"], intensity: 50 }, root)
    expect(root.dataset.background).toBe("gradient")
    expect(root.style.getPropertyValue("--yz-glow-b-light")).toContain("#46a0ff")
    expect(root.style.getPropertyValue("--yz-base-dark")).toContain("#3ddc97")

    applyBackgroundPreference("accent", { colors: ["#3ddc97", "#46a0ff"], intensity: 50 }, root)
    expect(root.dataset.background).toBe("accent")
    for (const name of ["--yz-glow-a-light", "--yz-glow-d-dark", "--yz-base-light", "--yz-base-dark"]) {
      expect(root.style.getPropertyValue(name)).toBe("")
    }
  })
})

describe("saved gradients", () => {
  it("puts the newest first, dedupes by colors and caps the palette", () => {
    const first = { colors: ["#111111"], intensity: 50 }
    const second = { colors: ["#222222", "#333333"], intensity: 40 }
    expect(addSavedGradient([first, second], { colors: ["#222222", "#333333"], intensity: 80 }))
      .toEqual([{ colors: ["#222222", "#333333"], intensity: 80 }, first])
    const full = Array.from({ length: MAX_SAVED_GRADIENTS }, (_, i) => ({ colors: [`#0000${String(i).padStart(2, "0")}`], intensity: 50 }))
    const next = addSavedGradient(full, { colors: ["#abcdef"], intensity: 50 })
    expect(next).toHaveLength(MAX_SAVED_GRADIENTS)
    expect(next[0].colors).toEqual(["#abcdef"])
  })
})

describe("color pad", () => {
  it("maps hue across and vividness down, and round-trips dot positions", () => {
    expect(padColor(0, 0)).toBe("#ff0a0a")
    for (const [x, y] of [[0.1, 0.2], [0.5, 0.5], [0.8, 0.9]] as const) {
      const position = padPosition(padColor(x, y))
      expect(position.x).toBeCloseTo(x, 1)
      expect(position.y).toBeCloseTo(y, 1)
    }
  })

  it("places grays at the pastel edge instead of failing", () => {
    expect(padPosition("#7d7d7d")).toEqual({ x: 0, y: 1 })
  })
})

describe("image backdrop", () => {
  it("accepts the three backdrop sources only", () => {
    expect(["accent", "gradient", "image"].every(isBackgroundSource)).toBe(true)
    expect(isBackgroundSource("photo")).toBe(false)
  })

  it("never lets the veil drop below a readable share of the theme base", () => {
    expect(imageVeilPercent(0)).toBe(100)
    expect(imageVeilPercent(70)).toBe(44)
    expect(imageVeilPercent(100)).toBe(20)
    expect(imageVeilPercent(500)).toBe(20)
  })

  it("sets the image and veil, and drops both when the image goes away", () => {
    const root = document.documentElement
    applyBackgroundImage("blob:tauri://localhost/1", 70, root)
    expect(root.style.getPropertyValue("--yz-bg-image")).toBe('url("blob:tauri://localhost/1")')
    expect(root.style.getPropertyValue("--yz-image-veil")).toBe("color-mix(in srgb, var(--yz-tone-base) 44%, transparent)")
    applyBackgroundImage(null, 70, root)
    expect(root.style.getPropertyValue("--yz-bg-image")).toBe("")
    expect(root.style.getPropertyValue("--yz-image-veil")).toBe("")
  })
})
