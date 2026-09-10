import { normalizeTerminalFontFamily, type TerminalFontFamily } from "@/terminal/terminalFonts"
import type { TerminalImeAnchorMode } from "@/terminal/terminalImePositioning"
import {
  DEFAULT_ACCENT_PREFERENCE,
  isAccentPreference,
  type AccentPreference,
} from "@/theme/accent"

export const TERMINAL_SETTINGS_STORAGE_KEY = "yuzora:terminal-settings"
export const APPEARANCE_SETTINGS_STORAGE_KEY = "yuzora:appearance-settings"

export type ThemePreference = "light" | "dark" | "auto"

export interface AppearanceSettings {
  theme: ThemePreference
  accent: AccentPreference
  leftSidebarBackground: boolean
  rightSidebarBackground: boolean
}

export interface TerminalSettings {
  copyOnSelect: boolean
  imeAnchorMode: TerminalImeAnchorMode
  fontSize: number
  fontFamily: TerminalFontFamily
}

const DEFAULT_APPEARANCE_SETTINGS: AppearanceSettings = {
  theme: "auto",
  accent: DEFAULT_ACCENT_PREFERENCE,
  leftSidebarBackground: true,
  rightSidebarBackground: true,
}

const VALID_THEME_PREFERENCES: ThemePreference[] = ["light", "dark", "auto"]

function readJsonSetting<T extends object>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key)
    if (!raw) return fallback
    const parsed = JSON.parse(raw) as Partial<T>
    return { ...fallback, ...parsed }
  } catch {
    return fallback
  }
}

export function writeJsonSetting<T extends object>(key: string, value: T): void {
  try {
    localStorage.setItem(key, JSON.stringify(value))
  } catch {
    /* private mode / quota — keep the in-memory field value only */
  }
}

export function loadTerminalSettings(): TerminalSettings {
  const stored = readJsonSetting<Partial<TerminalSettings>>(TERMINAL_SETTINGS_STORAGE_KEY, {})
  return {
    imeAnchorMode: stored.imeAnchorMode === "tui" ? "tui" : "cursor",
    copyOnSelect: stored.copyOnSelect !== false,
    fontSize: normalizeTerminalFontSize(stored.fontSize),
    fontFamily: normalizeTerminalFontFamily(stored.fontFamily),
  }
}

export const MIN_TERMINAL_FONT_SIZE = 8
export const MAX_TERMINAL_FONT_SIZE = 32
const DEFAULT_TERMINAL_FONT_SIZE = 12

export function normalizeTerminalFontSize(value: unknown): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    return DEFAULT_TERMINAL_FONT_SIZE
  }
  return Math.min(
    MAX_TERMINAL_FONT_SIZE,
    Math.max(MIN_TERMINAL_FONT_SIZE, Math.round(value)),
  )
}

export function loadAppearanceSettings(): AppearanceSettings {
  const settings = readJsonSetting<Partial<AppearanceSettings>>(APPEARANCE_SETTINGS_STORAGE_KEY, {})
  return {
    theme: VALID_THEME_PREFERENCES.includes(settings.theme as ThemePreference)
      ? settings.theme as ThemePreference
      : DEFAULT_APPEARANCE_SETTINGS.theme,
    accent: isAccentPreference(settings.accent)
      ? settings.accent
      : DEFAULT_APPEARANCE_SETTINGS.accent,
    leftSidebarBackground: typeof settings.leftSidebarBackground === "boolean"
      ? settings.leftSidebarBackground
      : DEFAULT_APPEARANCE_SETTINGS.leftSidebarBackground,
    rightSidebarBackground: typeof settings.rightSidebarBackground === "boolean"
      ? settings.rightSidebarBackground
      : DEFAULT_APPEARANCE_SETTINGS.rightSidebarBackground,
  }
}

export function saveAppearanceSettings(settings: AppearanceSettings): void {
  writeJsonSetting(APPEARANCE_SETTINGS_STORAGE_KEY, settings)
}
