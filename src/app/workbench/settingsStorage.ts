import { normalizeTerminalFontFamily, type TerminalFontFamily } from "@/terminal/terminalFonts"
import type { TerminalImeAnchorMode } from "@/terminal/terminalImePositioning"
import {
  DEFAULT_ACCENT_PREFERENCE,
  isAccentPreference,
  type AccentPreference,
} from "@/theme/accent"
import {
  DEFAULT_BACKGROUND_GRADIENT,
  DEFAULT_GLASS_TINT,
  DEFAULT_IMAGE_INTENSITY,
  MAX_SAVED_GRADIENTS,
  addSavedGradient,
  isBackgroundSource,
  normalizeBackgroundGradient,
  normalizeGlassTint,
  normalizeGradientIntensity,
  type BackgroundGradient,
  type BackgroundSource,
} from "@/theme/background"

export const TERMINAL_SETTINGS_STORAGE_KEY = "yuzora:terminal-settings"
export const APPEARANCE_SETTINGS_STORAGE_KEY = "yuzora:appearance-settings"

export type ThemePreference = "light" | "dark" | "auto"

export interface AppearanceSettings {
  theme: ThemePreference
  accent: AccentPreference
  leftSidebarBackground: boolean
  rightSidebarBackground: boolean
  botAnimations: boolean
  backgroundSource: BackgroundSource
  backgroundGradient: BackgroundGradient
  savedGradients: BackgroundGradient[]
  /** 0–100, how strongly the stored image shows through the theme veil. */
  backgroundImageIntensity: number
  /** Bumped on every image import so the shell reloads it; 0 = no image stored. */
  backgroundImageVersion: number
  glass: boolean
  glassTint: number
}

export type BackgroundAppearance = Pick<
  AppearanceSettings,
  | "backgroundSource" | "backgroundGradient" | "savedGradients"
  | "backgroundImageIntensity" | "backgroundImageVersion" | "glass" | "glassTint"
>

export interface TerminalSettings {
  copyOnSelect: boolean
  imeAnchorMode: TerminalImeAnchorMode
  fontSize: number
  fontFamily: TerminalFontFamily
}

export const DEFAULT_BACKGROUND_APPEARANCE: BackgroundAppearance = {
  backgroundSource: "accent",
  backgroundGradient: DEFAULT_BACKGROUND_GRADIENT,
  savedGradients: [],
  backgroundImageIntensity: DEFAULT_IMAGE_INTENSITY,
  backgroundImageVersion: 0,
  glass: false,
  glassTint: DEFAULT_GLASS_TINT,
}

const DEFAULT_APPEARANCE_SETTINGS: AppearanceSettings = {
  theme: "auto",
  accent: DEFAULT_ACCENT_PREFERENCE,
  leftSidebarBackground: true,
  rightSidebarBackground: true,
  botAnimations: false,
  ...DEFAULT_BACKGROUND_APPEARANCE,
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
    botAnimations: typeof settings.botAnimations === "boolean"
      ? settings.botAnimations
      : defaultBotAnimationsEnabled(),
    backgroundSource: isBackgroundSource(settings.backgroundSource) ? settings.backgroundSource : "accent",
    backgroundGradient: normalizeBackgroundGradient(settings.backgroundGradient)
      ?? DEFAULT_APPEARANCE_SETTINGS.backgroundGradient,
    savedGradients: Array.isArray(settings.savedGradients)
      ? settings.savedGradients
        .map(normalizeBackgroundGradient)
        .filter((gradient): gradient is BackgroundGradient => gradient !== null)
        // Hand-edited storage may repeat colors; swatches are keyed by them.
        .reduceRight<BackgroundGradient[]>(addSavedGradient, [])
        .slice(0, MAX_SAVED_GRADIENTS)
      : DEFAULT_APPEARANCE_SETTINGS.savedGradients,
    backgroundImageIntensity: typeof settings.backgroundImageIntensity === "number"
      ? normalizeGradientIntensity(settings.backgroundImageIntensity)
      : DEFAULT_BACKGROUND_APPEARANCE.backgroundImageIntensity,
    backgroundImageVersion: typeof settings.backgroundImageVersion === "number" && Number.isSafeInteger(settings.backgroundImageVersion) && settings.backgroundImageVersion > 0
      ? settings.backgroundImageVersion
      : 0,
    glass: settings.glass === true,
    glassTint: normalizeGlassTint(settings.glassTint),
  }
}

function defaultBotAnimationsEnabled(): boolean {
  if (typeof navigator === "undefined" || !(navigator.hardwareConcurrency > 4)) return false
  const memory = (navigator as Navigator & { deviceMemory?: number }).deviceMemory
  if (typeof memory === "number" && memory <= 4) return false
  return typeof window !== "undefined" && !window.matchMedia?.("(prefers-reduced-motion: reduce)").matches
}

export function saveAppearanceSettings(settings: AppearanceSettings): void {
  writeJsonSetting(APPEARANCE_SETTINGS_STORAGE_KEY, settings)
}
