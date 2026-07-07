export const TERMINAL_SETTINGS_STORAGE_KEY = "yuzora:terminal-settings"
export const PREVIEW_SETTINGS_STORAGE_KEY = "yuzora:preview-settings"
export const AGENT_SETTINGS_STORAGE_KEY = "yuzora:agent-settings"
// pin 版本：bunx 不 pin 會每次 re-resolve latest（網路解析 hang＋版本漂移，
// 2026-07-06 的 EPIPE crash 即升版當下引入），診斷見 docs/html logger 分析報告
export const DEFAULT_AGENT_COMMAND = "bunx pi-acp@0.0.31"

export type AgentPreset = "pi" | "custom"

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
}

const DEFAULT_TERMINAL_SETTINGS: TerminalSettings = {
  shellPath: "",
  shellArgs: "",
}

const DEFAULT_PREVIEW_SETTINGS: PreviewSettings = {
  command: "",
  port: "",
}

const DEFAULT_AGENT_SETTINGS: AgentSettings = {
  preset: "pi",
  command: DEFAULT_AGENT_COMMAND,
  traceEnabled: false,
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

export function loadAgentSettings(): AgentSettings {
  const settings = readJsonSetting(AGENT_SETTINGS_STORAGE_KEY, DEFAULT_AGENT_SETTINGS)
  return {
    preset: settings.preset === "custom" ? "custom" : "pi",
    command: settings.command.trim() || DEFAULT_AGENT_COMMAND,
    traceEnabled: settings.traceEnabled === true,
  }
}

export function resolveAgentCommand(settings = loadAgentSettings()): string {
  if (settings.preset === "pi") return DEFAULT_AGENT_COMMAND
  return settings.command.trim() || DEFAULT_AGENT_COMMAND
}
