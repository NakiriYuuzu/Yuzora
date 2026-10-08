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
  /** Set synchronously when the user saves, before the write is queued: a pending migration yields to it. */
  userIntent: boolean
  hydrate: () => Promise<void>
  setWslEnabled: (enabled: boolean) => Promise<void>
}

let hydrating: Promise<void> | null = null
// Preference writes run one at a time, so an older migration write cannot land after a user save.
let writes: Promise<unknown> = Promise.resolve()
function enqueueWrite<T>(task: () => Promise<T>): Promise<T> {
  const run = writes.then(task)
  writes = run.catch(() => undefined)
  return run
}

export const useRuntimePreferencesStore = create<RuntimePreferencesState>((set, get) => ({
  wslEnabled: false,
  hydrated: false,
  userSet: false,
  userIntent: false,
  hydrate() {
    if (get().hydrated) return Promise.resolve()
    hydrating ??= (async () => {
      try {
        let { wslEnabled } = await runtimePreferencesGet()
        // One-time migration of the former localStorage flag; keep it on failure so the next launch retries.
        if (!wslEnabled && !get().userSet && legacyWslEnabled()) {
          try {
            // Re-check at write time: a user save queued meanwhile wins.
            const migrated = await enqueueWrite(async () => {
              if (get().userIntent || get().userSet) return null
              const saved = await runtimePreferencesSet(true)
              clearLegacy()
              return saved.wslEnabled
            })
            if (migrated !== null) wslEnabled = migrated
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
    set({ userIntent: true })
    const saved = await enqueueWrite(() => runtimePreferencesSet(enabled))
    // The backend now holds the user's choice: the legacy flag must never migrate over it later.
    clearLegacy()
    set({ wslEnabled: saved.wslEnabled, userSet: true })
  }
}))
