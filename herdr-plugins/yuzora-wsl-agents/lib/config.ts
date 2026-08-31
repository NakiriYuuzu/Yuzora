import { CONFIG_SCHEMA_VERSION, ENABLED_AGENT } from "./constants"
import { assertSafeDistroName } from "./distro-list"
import type { LinuxCwdPolicy } from "./cwd"

const ALLOWED_KEYS = new Set([
  "schemaVersion",
  "defaultDistro",
  "distros",
  "enabledAgents",
  "linuxCwdPolicy"
])

export type PluginConfig = {
  schemaVersion: number
  defaultDistro: string | null
  distros: string[]
  enabledAgents: string[]
  linuxCwdPolicy: LinuxCwdPolicy
}

export const DEFAULT_PLUGIN_CONFIG: PluginConfig = {
  schemaVersion: CONFIG_SCHEMA_VERSION,
  defaultDistro: null,
  distros: [],
  enabledAgents: [ENABLED_AGENT],
  linuxCwdPolicy: "workspace"
}

export function parsePluginConfig(raw: unknown): PluginConfig {
  if (raw === null || raw === undefined) return { ...DEFAULT_PLUGIN_CONFIG }
  if (typeof raw !== "object" || Array.isArray(raw)) {
    throw new Error("plugin config must be a JSON object")
  }
  const record = raw as Record<string, unknown>
  for (const key of Object.keys(record)) {
    if (!ALLOWED_KEYS.has(key)) {
      throw new Error(`unknown plugin config key: ${key}`)
    }
  }
  const schemaVersion = record.schemaVersion ?? CONFIG_SCHEMA_VERSION
  if (schemaVersion !== CONFIG_SCHEMA_VERSION) {
    throw new Error(`unsupported config schemaVersion: ${String(schemaVersion)}`)
  }
  const defaultDistro =
    record.defaultDistro === null || record.defaultDistro === undefined
      ? null
      : assertSafeDistroName(String(record.defaultDistro))
  const distros = Array.isArray(record.distros)
    ? record.distros.map((value) => assertSafeDistroName(String(value)))
    : []
  const enabledAgents = Array.isArray(record.enabledAgents)
    ? record.enabledAgents.map((value) => String(value))
    : [ENABLED_AGENT]
  if (enabledAgents.length !== 1 || enabledAgents[0] !== ENABLED_AGENT) {
    throw new Error("enabledAgents must be exactly [\"pi\"] in this MVP")
  }
  const linuxCwdPolicy = record.linuxCwdPolicy ?? "workspace"
  if (linuxCwdPolicy !== "workspace" && linuxCwdPolicy !== "home") {
    throw new Error(`invalid linuxCwdPolicy: ${String(linuxCwdPolicy)}`)
  }
  return {
    schemaVersion: CONFIG_SCHEMA_VERSION,
    defaultDistro,
    distros,
    enabledAgents,
    linuxCwdPolicy
  }
}

export function resolveTargetDistros(
  config: PluginConfig,
  inventory: string[]
): string[] {
  const wanted = config.distros.length > 0
    ? config.distros
    : config.defaultDistro
      ? [config.defaultDistro]
      : []
  if (wanted.length === 0) {
    if (inventory.length === 0) throw new Error("no WSL distros are installed")
    return []
  }
  return wanted.map((name) => {
    const match = inventory.find((item) => item.toLowerCase() === name.toLowerCase())
    if (!match) {
      throw new Error(`WSL distro not installed: ${JSON.stringify(name)}`)
    }
    return match
  })
}

/** Pane launch uses defaultDistro only; adapter install still uses `distros`. */
export function resolveLaunchDistro(
  config: PluginConfig,
  inventory: string[]
): string | null {
  const wanted = config.defaultDistro ?? config.distros[0] ?? null
  if (!wanted) return null
  const match = inventory.find((item) => item.toLowerCase() === wanted.toLowerCase())
  if (!match) {
    throw new Error(`WSL distro not installed: ${JSON.stringify(wanted)}`)
  }
  return match
}
