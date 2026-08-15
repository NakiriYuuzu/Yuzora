import { afterEach, beforeEach, describe, expect, it } from "vitest"

import {
  DIALOG_EDGE_MARGIN_PX,
  DIALOG_SIZE_STORAGE_KEY,
  DEFAULT_DIALOG_SIZE_RATIO,
  applyResizeDelta,
  clearDialogSizePreference,
  clampPixelSize,
  defaultDialogSizePreference,
  dialogMinSize,
  dialogSizeBounds,
  loadDialogSizePreference,
  maxDialogSize,
  normalizePreference,
  parseDialogSizeStorage,
  preferenceFromSize,
  saveDialogSizePreference,
  sizeFromPreference,
} from "./dialogSize"

const VIEWPORT = { width: 1000, height: 800 }

function installLocalStorage(): void {
  const values = new Map<string, string>()
  const storage = {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => void values.set(key, String(value)),
    removeItem: (key: string) => void values.delete(key),
    clear: () => values.clear(),
    key: (index: number) => [...values.keys()][index] ?? null,
    get length() {
      return values.size
    },
  }
  Object.defineProperty(globalThis, "localStorage", {
    value: storage,
    configurable: true,
    writable: true,
  })
}

describe("dialogSize math", () => {
  it("defaults to 80% of the viewport", () => {
    const size = sizeFromPreference(defaultDialogSizePreference(), VIEWPORT)
    expect(size).toEqual({
      width: VIEWPORT.width * DEFAULT_DIALOG_SIZE_RATIO,
      height: VIEWPORT.height * DEFAULT_DIALOG_SIZE_RATIO,
    })
  })

  it("clamps to the edge margin without rewriting the preference ratio", () => {
    const preference = { widthRatio: 0.99, heightRatio: 0.99 }
    const size = sizeFromPreference(preference, VIEWPORT)
    expect(size).toEqual(maxDialogSize(VIEWPORT))
    expect(preference).toEqual({ widthRatio: 0.99, heightRatio: 0.99 })
  })

  it("honors minSize while never exceeding the viewport max", () => {
    const tiny = { width: 200, height: 150 }
    const bounds = dialogSizeBounds(tiny, dialogMinSize(320, 240))
    expect(bounds.minWidth).toBe(tiny.width - DIALOG_EDGE_MARGIN_PX * 2)
    expect(bounds.maxWidth).toBe(tiny.width - DIALOG_EDGE_MARGIN_PX * 2)

    const clamped = clampPixelSize({ width: 9999, height: 9999 }, tiny, dialogMinSize(320, 240))
    expect(clamped).toEqual(maxDialogSize(tiny))
  })

  it("converts pixels back to ratios for persistence", () => {
    expect(preferenceFromSize({ width: 500, height: 400 }, VIEWPORT)).toEqual({
      widthRatio: 0.5,
      heightRatio: 0.5,
    })
  })

  it("preserves tiny positive ratios without a 5% floor on extreme viewports", () => {
    const huge = { width: 10_000, height: 10_000 }
    const min = dialogMinSize(280, 180)
    const rendered = sizeFromPreference({ widthRatio: 0.028, heightRatio: 0.018 }, huge, min)
    expect(rendered).toEqual({ width: 280, height: 180 })

    const preference = preferenceFromSize(rendered, huge)
    expect(preference.widthRatio).toBeCloseTo(0.028, 4)
    expect(preference.heightRatio).toBeCloseTo(0.018, 4)
    expect(preference.widthRatio).toBeLessThan(0.05)
    expect(sizeFromPreference(preference, huge, min)).toEqual(rendered)
  })

  it("roundtrips a 280×180 minimum through a 10,000,000 viewport without collapsing to 80%", () => {
    const enormous = { width: 10_000_000, height: 10_000_000 }
    const min = dialogMinSize(280, 180)
    const rendered = sizeFromPreference({ widthRatio: 0.000028, heightRatio: 0.000018 }, enormous, min)
    expect(rendered).toEqual({ width: 280, height: 180 })

    const preference = preferenceFromSize(rendered, enormous)
    expect(preference.widthRatio).toBe(280 / 10_000_000)
    expect(preference.heightRatio).toBe(180 / 10_000_000)
    expect(preference.widthRatio).toBeGreaterThan(0)
    expect(preference.widthRatio).toBeLessThan(0.00005)
    expect(sizeFromPreference(preference, enormous, min)).toEqual({ width: 280, height: 180 })
  })

  it("applies resize deltas with centering-aware doubles left to the caller", () => {
    const next = applyResizeDelta({ width: 400, height: 300 }, { width: 40, height: 20 }, VIEWPORT)
    expect(next).toEqual({ width: 440, height: 320 })
  })
})

describe("dialogSize storage", () => {
  beforeEach(() => {
    installLocalStorage()
  })

  afterEach(() => {
    localStorage.clear()
  })

  it("loads the default preference when storage is empty", () => {
    expect(loadDialogSizePreference("settings")).toEqual(defaultDialogSizePreference())
  })

  it("persists and reloads per-modal preferences independently", () => {
    saveDialogSizePreference("settings", { widthRatio: 0.7, heightRatio: 0.6 })
    saveDialogSizePreference("command-palette", { widthRatio: 0.5, heightRatio: 0.4 })

    expect(loadDialogSizePreference("settings")).toEqual({
      widthRatio: 0.7,
      heightRatio: 0.6,
    })
    expect(loadDialogSizePreference("command-palette")).toEqual({
      widthRatio: 0.5,
      heightRatio: 0.4,
    })
    expect(loadDialogSizePreference("git-diff")).toEqual(defaultDialogSizePreference())
  })

  it("persists and reopens 280×180 at a 10,000,000 viewport without rewriting to 80%", () => {
    const enormous = { width: 10_000_000, height: 10_000_000 }
    const min = dialogMinSize(280, 180)
    const preference = preferenceFromSize({ width: 280, height: 180 }, enormous)
    // Use a real resizable Dialog ID — Alert/app-dialog is intentionally excluded.
    saveDialogSizePreference("settings", preference)

    const reloaded = loadDialogSizePreference("settings")
    expect(reloaded.widthRatio).toBe(280 / 10_000_000)
    expect(reloaded.heightRatio).toBe(180 / 10_000_000)
    expect(sizeFromPreference(reloaded, enormous, min)).toEqual({ width: 280, height: 180 })
  })

  it("reset clears only the requested modal id", () => {
    saveDialogSizePreference("settings", { widthRatio: 0.7, heightRatio: 0.6 })
    saveDialogSizePreference("askpass", { widthRatio: 0.55, heightRatio: 0.45 })
    clearDialogSizePreference("settings")

    expect(loadDialogSizePreference("settings")).toEqual(defaultDialogSizePreference())
    expect(loadDialogSizePreference("askpass")).toEqual({
      widthRatio: 0.55,
      heightRatio: 0.45,
    })
  })

  it("falls back on corrupt storage payloads", () => {
    localStorage.setItem(DIALOG_SIZE_STORAGE_KEY, "{not-json")
    expect(parseDialogSizeStorage(localStorage.getItem(DIALOG_SIZE_STORAGE_KEY))).toEqual({
      version: 1,
      sizes: {},
    })
    expect(loadDialogSizePreference("settings")).toEqual(defaultDialogSizePreference())

    localStorage.setItem(
      DIALOG_SIZE_STORAGE_KEY,
      JSON.stringify({
        version: 2,
        sizes: { settings: { widthRatio: 0.5, heightRatio: 0.5 } },
      }),
    )
    expect(loadDialogSizePreference("settings")).toEqual(defaultDialogSizePreference())

    localStorage.setItem(
      DIALOG_SIZE_STORAGE_KEY,
      JSON.stringify({
        version: 1,
        sizes: {
          settings: { widthRatio: Number.NaN, heightRatio: -1 },
          "not-a-real-id": { widthRatio: 0.4, heightRatio: 0.4 },
          "git-diff": { widthRatio: 0.66, heightRatio: 0.55 },
        },
      }),
    )
    expect(loadDialogSizePreference("settings")).toEqual(defaultDialogSizePreference())
    expect(loadDialogSizePreference("git-diff")).toEqual({
      widthRatio: 0.66,
      heightRatio: 0.55,
    })
  })

  it("normalizes partial preference objects", () => {
    expect(normalizePreference({ widthRatio: 0.42 })).toEqual({
      widthRatio: 0.42,
      heightRatio: DEFAULT_DIALOG_SIZE_RATIO,
    })
  })
})
