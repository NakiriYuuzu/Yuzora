import { create } from "zustand"
import { runtimePreferencesGet, runtimePreferencesSet } from "@/lib/runtimePreferencesIpc"

/** Legacy browser-side copy; the Rust side is the source of truth since v0.0.18. */
export const LEGACY_RUNTIME_PREFERENCES_KEY = "yuzora.runtime.preferences.v1"

function legacyWslEnabled(): boolean {
  try { return JSON.parse(window.localStorage.getItem(LEGACY_RUNTIME_PREFERENCES_KEY) ?? "{}").wslEnabled === true }
  catch { return false }
}

function clearLegacy() {
  try { window.localStorage.removeItem(LEGACY_RUNTIME_PREFERENCES_KEY) } catch { /* storage unavailable */ }
}

interface RuntimePreferencesState {
  wslEnabled: boolean
  hydrated: boolean
  /** Set once the user saved a value; an older in-flight hydrate must not overwrite it. */
  userSet: boolean
  hydrate: () => Promise<void>
  setWslEnabled: (enabled: boolean) => Promise<void>
}

let hydrating: Promise<void> | null = null

export const useRuntimePreferencesStore = create<RuntimePreferencesState>((set, get) => ({
  wslEnabled: false,
  hydrated: false,
  userSet: false,
  hydrate() {
    if (get().hydrated) return Promise.resolve()
    hydrating ??= (async () => {
      try {
        let { wslEnabled } = await runtimePreferencesGet()
        // One-time migration of the former localStorage flag; keep it on failure so the next launch retries.
        if (!wslEnabled && !get().userSet && legacyWslEnabled()) {
          try {
            wslEnabled = (await runtimePreferencesSet(true)).wslEnabled
            clearLegacy()
          } catch (error) {
            console.warn("runtime preference migration failed", error)
          }
        }
        set(get().userSet ? { hydrated: true } : { wslEnabled, hydrated: true })
      } catch (error) {
        // Older backends lack the command: treat WSL as disabled without blocking startup.
        console.warn("runtime preferences unavailable", error)
        set(get().userSet ? { hydrated: true } : { wslEnabled: false, hydrated: true })
      } finally {
        hydrating = null
      }
    })()
    return hydrating
  },
  async setWslEnabled(enabled) {
    const saved = await runtimePreferencesSet(enabled)
    set({ wslEnabled: saved.wslEnabled, userSet: true })
  }
}))
