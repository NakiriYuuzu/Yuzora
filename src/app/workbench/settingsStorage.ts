import {
  AGENT_PRESETS, DEFAULT_AGENT_COMMAND, DEFAULT_AGENT_ID, agentPresetForCommand,
  resolveCuratedAgentCommand,
  type AgentCommandIdentity, type AgentCommandMode, type AgentCommandResolution,
  type AgentId, type AgentPreset,
} from "@/lib/agentPresets"

export { AGENT_PRESETS, DEFAULT_AGENT_COMMAND, agentPresetForCommand }
export type { AgentId, AgentPreset }

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
  shellPath: string
  shellArgs: string
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
}

export interface AgentPresetCommandSettings {
  mode: AgentCommandMode
  customCommand: string
}

const DEFAULT_TERMINAL_SETTINGS: TerminalSettings = {
  shellPath: "",
  shellArgs: "",
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
    pi: { mode: "verified", customCommand: "" },
    claude: { mode: "verified", customCommand: "" },
    codex: { mode: "verified", customCommand: "" },
  },
}

export function readJsonSetting<T extends object>(key: string, fallback: T): T {
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
  return readJsonSetting(TERMINAL_SETTINGS_STORAGE_KEY, DEFAULT_TERMINAL_SETTINGS)
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
const VALID_COMMAND_MODES: AgentCommandMode[] = ["verified", "latest", "custom"]

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
// then the verified/default Pi route. A branded preset in Custom mode is still
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
  const mode = VALID_COMMAND_MODES.includes(record.mode as AgentCommandMode)
    ? record.mode as AgentCommandMode
    : "verified"
  return {
    mode,
    customCommand: typeof record.customCommand === "string" ? record.customCommand : "",
  }
}
