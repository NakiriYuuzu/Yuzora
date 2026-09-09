import { create } from "zustand"

const STORAGE = "yuzora.runtime.preferences.v1"
function loadWslEnabled(): boolean {
  try { return JSON.parse(window.localStorage.getItem(STORAGE) ?? "{}").wslEnabled === true }
  catch { return false }
}

export const useRuntimePreferencesStore = create<{
  wslEnabled: boolean
  setWslEnabled: (enabled: boolean) => void
}>((set) => ({
  wslEnabled: loadWslEnabled(),
  setWslEnabled(enabled) {
    window.localStorage.setItem(STORAGE, JSON.stringify({ wslEnabled: enabled }))
    set({ wslEnabled: enabled })
  }
}))
