import {
  AGENT_PRESETS, DEFAULT_AGENT_COMMAND, DEFAULT_AGENT_ID,
  resolveCuratedAgentCommand,
  type AgentCommandIdentity, type AgentCommandMode, type AgentCommandResolution,
  type AgentId, type AgentPreset,
} from "@/lib/agentPresets"
import { cachedBuiltinPiAdapterCommand } from "@/lib/platform"
import type { TerminalProfile, TerminalProfileKind } from "@/lib/types"
import {
  EMPTY_CUSTOM_TERMINAL_PROFILE,
  SYSTEM_TERMINAL_PROFILE,
} from "@/terminal/terminalProfiles"
import type { TerminalImeAnchorMode } from "@/terminal/terminalImePositioning"

export { AGENT_PRESETS, DEFAULT_AGENT_COMMAND }
export type { AgentId, AgentPreset }

/** P5：pi 的 runtime 選擇——builtin（bundle 內 yuzora-pi-acp）為預設，community
 *（bunx pi-acp@latest）保留為一鍵 rollback。只影響 pi preset 的 latest mode。 */
export type PiRuntime = "builtin" | "community"

export const TERMINAL_SETTINGS_STORAGE_KEY = "yuzora:terminal-settings"
export const PREVIEW_SETTINGS_STORAGE_KEY = "yuzora:preview-settings"
export const AGENT_SETTINGS_STORAGE_KEY = "yuzora:agent-settings"
export const LAST_USED_CURATED_AGENT_STORAGE_KEY = "yuzora:last-used-curated-agent"
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

export interface AgentSettings {
  preset: AgentPreset
  command: string
  traceEnabled: boolean
  presetCommands: Record<AgentId, AgentPresetCommandSettings>
  piRuntime: PiRuntime
}

interface AgentPresetCommandSettings {
  mode: AgentCommandMode
  customCommand: string
}

const DEFAULT_PREVIEW_SETTINGS: PreviewSettings = {
  command: "",
  port: "",
}

const DEFAULT_APPEARANCE_SETTINGS: AppearanceSettings = {
  theme: "auto",
}

const VALID_THEME_PREFERENCES: ThemePreference[] = ["light", "dark", "auto"]

const DEFAULT_AGENT_SETTINGS: AgentSettings = {
  preset: "pi",
  command: DEFAULT_AGENT_COMMAND,
  traceEnabled: false,
  presetCommands: {
    pi: { mode: "latest", customCommand: "" },
    claude: { mode: "latest", customCommand: "" },
    codex: { mode: "latest", customCommand: "" },
  },
  piRuntime: "builtin",
}

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
    cwdStrategy: profile.cwdStrategy === "wsl" ? "wsl" : "native",
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

const VALID_PRESETS: AgentPreset[] = ["pi", "claude", "codex", "custom"]
const VALID_COMMAND_MODES: AgentCommandMode[] = ["latest", "custom"]

export function loadAgentSettings(): AgentSettings {
  const settings = readJsonSetting<Partial<AgentSettings>>(AGENT_SETTINGS_STORAGE_KEY, {})
  return {
    preset: VALID_PRESETS.includes(settings.preset as AgentPreset)
      ? settings.preset as AgentPreset
      : DEFAULT_AGENT_SETTINGS.preset,
    command: typeof settings.command === "string" && settings.command.trim()
      ? settings.command.trim()
      : DEFAULT_AGENT_SETTINGS.command,
    traceEnabled: settings.traceEnabled === true,
    presetCommands: normalizePresetCommands(settings.presetCommands),
    piRuntime: settings.piRuntime === "community" ? "community" : DEFAULT_AGENT_SETTINGS.piRuntime,
  }
}

export function resolveAgentCommand(settings = loadAgentSettings()): string {
  return resolveAgentCommandRoute(undefined, settings).command
}

export function resolveAgentCommandRoute(
  agentId?: AgentId,
  settings = loadAgentSettings(),
): AgentCommandResolution {
  const selectedPreset = agentId ?? settings.preset
  if (selectedPreset === "custom") {
    return {
      selectedPreset,
      commandMode: "custom",
      command: settings.command.trim() || DEFAULT_AGENT_COMMAND,
      trustedAgentId: null,
    }
  }
  const preference = settings.presetCommands[selectedPreset]
  // P5：pi＋latest＋builtin runtime → bundle 內 adapter。builtin 一樣是 curated
  //（trustedAgentId 維持 "pi"，prewarm／last-used 通道不變）；cache 未 ready
  //（dev server、非 Tauri、resource 缺失）時退回 community command。custom mode
  // 不受 runtime 選擇影響。agentRouter 契約不動——runtime 差異只反映在 command
  // 字串，command+cwd keying 天然隔離兩個 runtime 的子行程與 session。
  if (selectedPreset === "pi" && preference.mode === "latest" && settings.piRuntime === "builtin") {
    const builtin = cachedBuiltinPiAdapterCommand()
    if (builtin) {
      return {
        selectedPreset,
        commandMode: "latest",
        command: builtin,
        trustedAgentId: "pi",
      }
    }
  }
  return resolveCuratedAgentCommand(
    selectedPreset,
    preference.mode,
    preference.customCommand,
  )
}

export function loadLastUsedCuratedAgent(): AgentId | null {
  try {
    const value = localStorage.getItem(LAST_USED_CURATED_AGENT_STORAGE_KEY)
    return value === "pi" || value === "claude" || value === "codex" ? value : null
  } catch {
    return null
  }
}

// Only a successful session/new result can supply this trusted identity. Custom
// commands never enter the last-used channel, including custom mode selected
// from a branded preset.
export function rememberLastUsedCuratedAgent(identity: AgentCommandIdentity | undefined): void {
  const agentId = identity?.commandMode === "custom" ? null : identity?.trustedAgentId
  if (!agentId) return
  try {
    localStorage.setItem(LAST_USED_CURATED_AGENT_STORAGE_KEY, agentId)
  } catch {
    // private mode / quota — resolver will use Settings instead
  }
}

// Resolve only identities that remain curated under today's Settings. If no
// successful session has been recorded, prefer the selected trusted preset and
// then the latest/default Pi route. A branded preset in Custom mode is still
// untrusted and therefore produces no prewarm candidate.
export function resolvePrewarmAgentId(settings = loadAgentSettings()): AgentId | null {
  const lastUsed = loadLastUsedCuratedAgent()
  if (lastUsed) {
    if (resolveAgentCommandRoute(lastUsed, settings).trustedAgentId) return lastUsed
  }
  const selected = resolveAgentCommandRoute(undefined, settings)
  if (selected.trustedAgentId) return selected.trustedAgentId
  const fallback = resolveAgentCommandRoute(DEFAULT_AGENT_ID, settings)
  return fallback.trustedAgentId ? DEFAULT_AGENT_ID : null
}

// AgentPickerPopover 的「自訂 command…」流用：把 picker 內填的 command 存為
// 全域 custom preset（保留既有 traceEnabled），讓後續省略 agentId 的
// newSession(cwd) 走 resolveAgentCommand() 解出同一條 command。
export function saveCustomAgentCommand(command: string): AgentSettings {
  const next: AgentSettings = { ...loadAgentSettings(), preset: "custom", command: command.trim() }
  writeJsonSetting(AGENT_SETTINGS_STORAGE_KEY, next)
  return next
}

function normalizePresetCommands(value: unknown): AgentSettings["presetCommands"] {
  const record = value && typeof value === "object" ? value as Record<string, unknown> : {}
  return {
    pi: normalizePresetCommand(record.pi),
    claude: normalizePresetCommand(record.claude),
    codex: normalizePresetCommand(record.codex),
  }
}

function normalizePresetCommand(value: unknown): AgentPresetCommandSettings {
  const record = value && typeof value === "object" ? value as Record<string, unknown> : {}
  // `verified` is the pre-latest legacy value. Treat it (and any unknown mode)
  // as latest without writing during load; the next Settings change persists
  // the normalized envelope.
  const mode = VALID_COMMAND_MODES.includes(record.mode as AgentCommandMode)
    ? record.mode as AgentCommandMode
    : "latest"
  return {
    mode,
    customCommand: typeof record.customCommand === "string" ? record.customCommand : "",
  }
}
