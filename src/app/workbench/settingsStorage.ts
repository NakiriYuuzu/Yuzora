import { isWindowsPlatform } from "@/lib/platform"
import type { TerminalProfile, TerminalProfileKind } from "@/lib/types"
import {
  EMPTY_CUSTOM_TERMINAL_PROFILE,
  SYSTEM_TERMINAL_PROFILE,
} from "@/terminal/terminalProfiles"
import type { TerminalImeAnchorMode } from "@/terminal/terminalImePositioning"

export const TERMINAL_SETTINGS_STORAGE_KEY = "yuzora:terminal-settings"
export const PREVIEW_SETTINGS_STORAGE_KEY = "yuzora:preview-settings"
export const APPEARANCE_SETTINGS_STORAGE_KEY = "yuzora:appearance-settings"

export type ThemePreference = "light" | "dark" | "auto"

export interface AppearanceSettings {
  theme: ThemePreference
}

export interface TerminalSettings {
  defaultProfile: TerminalProfile
  customProfile: TerminalProfile
  imeAnchorMode: TerminalImeAnchorMode
  fontSize: number
}

export interface PreviewSettings {
  command: string
  port: string
}

const DEFAULT_PREVIEW_SETTINGS: PreviewSettings = {
  command: "",
  port: "",
}

const DEFAULT_APPEARANCE_SETTINGS: AppearanceSettings = {
  theme: "auto",
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
  type StoredTerminalSettings = Partial<TerminalSettings> & {
    shellPath?: unknown
    shellArgs?: unknown
  }
  const stored = readJsonSetting<StoredTerminalSettings>(TERMINAL_SETTINGS_STORAGE_KEY, {})
  const legacyShell = typeof stored.shellPath === "string" ? stored.shellPath.trim() : ""
  const legacyArgs = typeof stored.shellArgs === "string"
    ? stored.shellArgs.trim().split(/\s+/).filter(Boolean)
    : []
  const hasLegacyProfile = legacyShell.length > 0 || legacyArgs.length > 0
  const legacyProfile: TerminalProfile = {
    ...EMPTY_CUSTOM_TERMINAL_PROFILE,
    shell: legacyShell,
    args: legacyArgs,
  }
  const customProfile = normalizeTerminalProfile(
    stored.customProfile,
    hasLegacyProfile ? legacyProfile : EMPTY_CUSTOM_TERMINAL_PROFILE,
    "custom",
  )
  const defaultProfile = normalizeTerminalProfile(
    stored.defaultProfile,
    hasLegacyProfile ? legacyProfile : SYSTEM_TERMINAL_PROFILE,
  )

  return {
    defaultProfile,
    customProfile,
    imeAnchorMode: stored.imeAnchorMode === "tui" ? "tui" : "cursor",
    fontSize: normalizeTerminalFontSize(stored.fontSize),
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

const TERMINAL_PROFILE_KINDS: TerminalProfileKind[] = [
  "system",
  "cmd",
  "powershell",
  "wsl",
  "custom",
]

function normalizeTerminalProfile(
  value: unknown,
  fallback: TerminalProfile,
  forcedKind?: TerminalProfileKind,
): TerminalProfile {
  if (!value || typeof value !== "object") return { ...fallback, args: [...fallback.args] }
  const profile = value as Partial<TerminalProfile>
  if (
    typeof profile.id !== "string"
    || typeof profile.name !== "string"
    || typeof profile.shell !== "string"
    || !Array.isArray(profile.args)
    || !profile.args.every((arg) => typeof arg === "string")
    || !TERMINAL_PROFILE_KINDS.includes(profile.kind as TerminalProfileKind)
  ) {
    return { ...fallback, args: [...fallback.args] }
  }
  return {
    id: forcedKind === "custom" ? "custom" : profile.id,
    name: profile.name,
    shell: profile.shell.trim(),
    args: [...profile.args],
    kind: forcedKind ?? profile.kind!,
    cwdStrategy:
      profile.cwdStrategy === "wsl" && isWindowsPlatform() ? "wsl" : "native",
  }
}

export function loadPreviewSettings(): PreviewSettings {
  return readJsonSetting(PREVIEW_SETTINGS_STORAGE_KEY, DEFAULT_PREVIEW_SETTINGS)
}

export function loadAppearanceSettings(): AppearanceSettings {
  const settings = readJsonSetting<Partial<AppearanceSettings>>(APPEARANCE_SETTINGS_STORAGE_KEY, {})
  return {
    theme: VALID_THEME_PREFERENCES.includes(settings.theme as ThemePreference)
      ? settings.theme as ThemePreference
      : DEFAULT_APPEARANCE_SETTINGS.theme,
  }
}

export function saveAppearanceSettings(settings: AppearanceSettings): void {
  writeJsonSetting(APPEARANCE_SETTINGS_STORAGE_KEY, settings)
}
