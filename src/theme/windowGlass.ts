import { Effect, EffectState, getCurrentWindow, type Effects } from "@tauri-apps/api/window"
import { isMacPlatform, isTauri, isWindowsPlatform } from "@/lib/platform"
import { normalizeGlassTint } from "@/theme/background"

export type GlassPlatform = "macos" | "windows"

// Both need the platform window's transparent webview (tauri.{macos,windows}.conf.json).
function glassEffects(platform: GlassPlatform): Effects {
  return platform === "macos"
    // Chosen over sidebar (too opaque in light mode) and Liquid Glass in a side-by-side look.
    ? { effects: [Effect.UnderWindowBackground], state: EffectState.FollowsWindowActiveState }
    // Tauri applies only the first listed Windows effect and drops its error, so
    // there is no fallback to list here — unsupported builds never get glass.
    : { effects: [Effect.Acrylic] }
}

// UA-CH platformVersion 15 = Windows 11 22H2 (build 22621): the first release
// whose system acrylic backdrop doesn't lag while dragging or resizing.
const MIN_WINDOWS_PLATFORM_VERSION = 15

type UserAgentData = { getHighEntropyValues(hints: string[]): Promise<{ platformVersion?: string }> }

let lastDetected: GlassPlatform | null = null

export async function windowGlassPlatform(): Promise<GlassPlatform | null> {
  lastDetected = !isTauri() ? null
    : isMacPlatform() ? "macos"
      : isWindowsPlatform() && await windowsSupportsAcrylic() ? "windows"
        : null
  return lastDetected
}

/** Last detection result for synchronous callers (settings search); AppShell detects on mount. */
export function supportsWindowGlass(): boolean {
  return lastDetected !== null
}

async function windowsSupportsAcrylic(): Promise<boolean> {
  const data = (navigator as Navigator & { userAgentData?: UserAgentData }).userAgentData
  if (!data) return false
  try {
    const { platformVersion } = await data.getHighEntropyValues(["platformVersion"])
    return Number.parseInt(platformVersion ?? "", 10) >= MIN_WINDOWS_PLATFORM_VERSION
  } catch {
    return false
  }
}

let glassRequest = 0
/** The latest toggle's wish; queued native steps apply whatever it is when they run. */
let desired: { active: boolean; platform: GlassPlatform } | null = null
/** What the window's native effects are now. */
let nativeActive = false
let nativeSteps = 0
let nativeQueue: Promise<void> = Promise.resolve()

/**
 * Lets the blurred desktop show through the window backdrop and sidebars.
 * Editors and terminals keep their opaque surfaces. index.html pins an
 * opaque html background before first paint; glass clears it, and leaving
 * glass drops the inline color so body's opaque background shows again.
 * Startup without glass sends no IPC; a call superseded by a newer toggle
 * while detecting the platform does nothing.
 */
export async function applyWindowGlass(enabled: boolean, root: HTMLElement = document.documentElement): Promise<void> {
  const request = ++glassRequest
  const platform = await windowGlassPlatform()
  if (request !== glassRequest) return
  const active = enabled === true && platform !== null
  if (active) {
    root.dataset.glass = "true"
    root.style.backgroundColor = "transparent"
  } else if (root.dataset.glass === "true") {
    leaveGlass(root)
  }
  if (!platform) {
    // Detection can fail later (a UA-CH query on Windows): an effect already on
    // the window must still go, using the platform it was applied with.
    if (desired?.active) {
      desired = { ...desired, active: false }
      await syncNative(root)
    }
    return
  }
  desired = { active, platform }
  await syncNative(root)
}

/**
 * Native calls run one at a time and each applies the latest wish, so a slow
 * call can never land after a newer toggle and leave the window out of step.
 */
function syncNative(root: HTMLElement): Promise<void> {
  const step = ++nativeSteps
  const run = nativeQueue.then(async () => {
    const want = desired
    if (!want || want.active === nativeActive) return
    try {
      if (want.active) await getCurrentWindow().setEffects(glassEffects(want.platform))
      else await getCurrentWindow().clearEffects()
      nativeActive = want.active
    } catch {
      // Without native blur the faded backdrop would show the raw desktop, so
      // a refused enable falls back to the opaque window — unless a newer step
      // is queued and will try again.
      if (want.active && step === nativeSteps) {
        desired = { ...want, active: false }
        leaveGlass(root)
      }
    }
  })
  nativeQueue = run.catch(() => undefined)
  return run
}

function leaveGlass(root: HTMLElement): void {
  delete root.dataset.glass
  root.style.removeProperty("background-color")
}

/** How much of the gradient/accent backdrop covers the native glass (0–100). */
export function applyGlassTint(tint: number, root: HTMLElement = document.documentElement): void {
  root.style.setProperty("--yz-glass-tint", String(normalizeGlassTint(tint) / 100))
}
