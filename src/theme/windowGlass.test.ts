import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

const native = vi.hoisted(() => ({
  isTauri: vi.fn(() => true),
  isMac: vi.fn(() => true),
  isWindows: vi.fn(() => false),
  setEffects: vi.fn(async () => {}),
  clearEffects: vi.fn(async () => {}),
}))

vi.mock("@/lib/platform", () => ({ isTauri: native.isTauri, isMacPlatform: native.isMac, isWindowsPlatform: native.isWindows }))
vi.mock("@tauri-apps/api/window", () => ({
  Effect: { UnderWindowBackground: "underWindowBackground", Acrylic: "acrylic" },
  EffectState: { FollowsWindowActiveState: "followsWindowActiveState" },
  getCurrentWindow: () => ({ setEffects: native.setEffects, clearEffects: native.clearEffects }),
}))

import { applyGlassTint, applyWindowGlass, supportsWindowGlass, windowGlassPlatform } from "./windowGlass"

const root = document.documentElement

function onWindows(platformVersion: string | Error | null) {
  native.isMac.mockReturnValue(false)
  native.isWindows.mockReturnValue(true)
  const userAgentData = platformVersion === null ? undefined : {
    getHighEntropyValues: vi.fn(async () => {
      if (platformVersion instanceof Error) throw platformVersion
      return { platformVersion }
    }),
  }
  Object.defineProperty(navigator, "userAgentData", { configurable: true, value: userAgentData })
}

beforeEach(async () => {
  native.isTauri.mockReturnValue(true)
  native.isMac.mockReturnValue(true)
  native.isWindows.mockReturnValue(false)
  // The module remembers the window's native state: start every test with glass off.
  await applyWindowGlass(false, root)
  vi.clearAllMocks()
  // index.html pins an opaque background before the first paint.
  root.style.backgroundColor = "#fbfaf6"
})

afterEach(() => {
  vi.clearAllMocks()
  root.removeAttribute("style")
  delete root.dataset.glass
  delete (navigator as Navigator & { userAgentData?: unknown }).userAgentData
})

describe("glass platform", () => {
  it("offers glass in the macOS shell", async () => {
    expect(await windowGlassPlatform()).toBe("macos")
    expect(supportsWindowGlass()).toBe(true)
    native.isTauri.mockReturnValue(false)
    expect(await windowGlassPlatform()).toBeNull()
    expect(supportsWindowGlass()).toBe(false)
  })

  it("offers glass on Windows 11 22H2 and later only", async () => {
    for (const [version, platform] of [["15.0.0", "windows"], ["19.0.0", "windows"], ["14.0.0", null], ["10.0.0", null], ["", null]] as const) {
      onWindows(version)
      expect(await windowGlassPlatform()).toBe(platform)
    }
  })

  it("withholds glass when Windows can't report its version", async () => {
    onWindows(null)
    expect(await windowGlassPlatform()).toBeNull()
    onWindows(new Error("hint refused"))
    expect(await windowGlassPlatform()).toBeNull()
  })

  it("never offers glass on Linux", async () => {
    native.isMac.mockReturnValue(false)
    expect(await windowGlassPlatform()).toBeNull()
  })
})

describe("window glass", () => {
  it("starts without glass and sends no native request", async () => {
    await applyWindowGlass(false, root)
    expect(native.setEffects).not.toHaveBeenCalled()
    expect(native.clearEffects).not.toHaveBeenCalled()
    expect(root.style.backgroundColor).toBe("rgb(251, 250, 246)")
  })

  it("clears the pinned background for vibrancy and drops it when turned off", async () => {
    await applyWindowGlass(true, root)
    expect(native.setEffects).toHaveBeenCalledWith({ effects: ["underWindowBackground"], state: "followsWindowActiveState" })
    expect(root.dataset.glass).toBe("true")
    expect(root.style.backgroundColor).toBe("transparent")

    await applyWindowGlass(true, root)
    expect(native.setEffects).toHaveBeenCalledTimes(1)

    await applyWindowGlass(false, root)
    expect(native.clearEffects).toHaveBeenCalledTimes(1)
    expect(root.dataset.glass).toBeUndefined()
    expect(root.style.backgroundColor).toBe("")
  })

  it("uses system acrylic on supported Windows", async () => {
    onWindows("15.0.0")
    await applyWindowGlass(true, root)
    expect(native.setEffects).toHaveBeenCalledWith({ effects: ["acrylic"] })
    expect(root.dataset.glass).toBe("true")
  })

  it("treats a missing preference as off", async () => {
    await applyWindowGlass(undefined as unknown as boolean, root)
    expect(native.setEffects).not.toHaveBeenCalled()
    expect(native.clearEffects).not.toHaveBeenCalled()
    expect(root.style.backgroundColor).toBe("rgb(251, 250, 246)")
  })

  it("ignores the preference where glass is unsupported", async () => {
    onWindows("14.0.0")
    await applyWindowGlass(true, root)
    expect(native.setEffects).not.toHaveBeenCalled()
    expect(root.dataset.glass).toBeUndefined()
    expect(root.style.backgroundColor).toBe("rgb(251, 250, 246)")
  })

  it("falls back to the opaque window when vibrancy is refused", async () => {
    native.setEffects.mockRejectedValueOnce(new Error("not allowed"))
    await expect(applyWindowGlass(true, root)).resolves.toBeUndefined()
    expect(root.dataset.glass).toBeUndefined()
    expect(root.style.backgroundColor).toBe("")
  })

  it("lets only the newest toggle reach the window while the platform is detected", async () => {
    const stale = applyWindowGlass(true, root)
    await applyWindowGlass(false, root)
    await stale
    expect(native.setEffects).not.toHaveBeenCalled()
    expect(root.dataset.glass).toBeUndefined()
  })

  it("keeps a newer glass toggle when an older request is refused late", async () => {
    let refuse: (error: Error) => void = () => {}
    native.setEffects.mockImplementationOnce(() => new Promise<void>((_, reject) => { refuse = reject }))
    const first = applyWindowGlass(true, root)
    await vi.waitFor(() => expect(native.setEffects).toHaveBeenCalledTimes(1))
    const off = applyWindowGlass(false, root)
    const on = applyWindowGlass(true, root)
    refuse(new Error("late"))
    await Promise.all([first, off, on])
    expect(native.setEffects).toHaveBeenCalledTimes(2)
    expect(root.dataset.glass).toBe("true")
    expect(root.style.backgroundColor).toBe("transparent")
  })

  it("still clears an applied effect when a later detection fails", async () => {
    onWindows("15.0.0")
    await applyWindowGlass(true, root)
    expect(native.setEffects).toHaveBeenCalledTimes(1)
    onWindows(new Error("UA-CH unavailable"))
    await applyWindowGlass(false, root)
    expect(native.clearEffects).toHaveBeenCalledTimes(1)
    expect(root.dataset.glass).toBeUndefined()
  })

  it("applies native effect changes one at a time in request order", async () => {
    let finish: () => void = () => {}
    native.setEffects.mockImplementationOnce(() => new Promise<void>(resolve => { finish = resolve }))
    const on = applyWindowGlass(true, root)
    await vi.waitFor(() => expect(native.setEffects).toHaveBeenCalledTimes(1))
    const off = applyWindowGlass(false, root)
    await new Promise(resolve => setTimeout(resolve, 0))
    // A slow enable must not be overtaken by the newer disable.
    expect(native.clearEffects).not.toHaveBeenCalled()
    finish()
    await Promise.all([on, off])
    expect(native.clearEffects).toHaveBeenCalledTimes(1)
    expect(native.setEffects.mock.invocationCallOrder[0]).toBeLessThan(native.clearEffects.mock.invocationCallOrder[0])
    expect(root.dataset.glass).toBeUndefined()
  })

  it("maps the tint percentage onto the backdrop opacity", () => {
    applyGlassTint(35, root)
    expect(root.style.getPropertyValue("--yz-glass-tint")).toBe("0.35")
    applyGlassTint(400, root)
    expect(root.style.getPropertyValue("--yz-glass-tint")).toBe("1")
  })
})
