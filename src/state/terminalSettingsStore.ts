import { create } from "zustand"

import {
  TERMINAL_SETTINGS_STORAGE_KEY,
  loadTerminalSettings,
  normalizeTerminalFontSize,
  writeJsonSetting,
  type TerminalSettings,
} from "@/app/workbench/settingsStorage"

interface TerminalSettingsStore extends TerminalSettings {
  update: (patch: Partial<TerminalSettings>) => void
}

export const useTerminalSettingsStore = create<TerminalSettingsStore>()((set, get) => ({
  ...loadTerminalSettings(),
  update: (patch) => {
    const next: TerminalSettings = {
      defaultProfile: patch.defaultProfile ?? get().defaultProfile,
      customProfile: patch.customProfile ?? get().customProfile,
      imeAnchorMode: patch.imeAnchorMode ?? get().imeAnchorMode,
      fontSize: normalizeTerminalFontSize(patch.fontSize ?? get().fontSize),
    }
    set(next)
    writeJsonSetting(TERMINAL_SETTINGS_STORAGE_KEY, next)
  },
}))

export function reloadTerminalSettingsStore(): void {
  useTerminalSettingsStore.setState(loadTerminalSettings())
}
