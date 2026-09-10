import { create } from "zustand"
import { normalizeTerminalFontFamily } from "@/terminal/terminalFonts"

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
      imeAnchorMode: patch.imeAnchorMode ?? get().imeAnchorMode,
      fontSize: normalizeTerminalFontSize(patch.fontSize ?? get().fontSize),
      fontFamily: normalizeTerminalFontFamily(patch.fontFamily ?? get().fontFamily),
    }
    set(next)
    writeJsonSetting(TERMINAL_SETTINGS_STORAGE_KEY, next)
  },
}))

export function reloadTerminalSettingsStore(): void {
  useTerminalSettingsStore.setState(loadTerminalSettings())
}
