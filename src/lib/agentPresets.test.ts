import { beforeEach, describe, expect, it } from "vitest"
import {
  AGENT_SETTINGS_STORAGE_KEY,
  LAST_USED_CURATED_AGENT_STORAGE_KEY,
  loadLastUsedCuratedAgent,
  rememberLastUsedCuratedAgent,
  resolvePrewarmAgentId,
  writeJsonSetting,
} from "@/app/workbench/settingsStorage"
import {
  AGENT_PRESETS, AGENT_VISUALS, CUSTOM_AGENT_VISUAL, agentDisplayName,
  agentPresetForCommand, commandForAgent, commandForPreset,
  resolveCuratedAgentCommand,
} from "./agentPresets"

function installLocalStorage(): void {
  const store = new Map<string, string>()
  Object.defineProperty(globalThis, "localStorage", {
    value: {
      getItem: (key: string) => store.get(key) ?? null,
      setItem: (key: string, value: string) => void store.set(key, String(value)),
      removeItem: (key: string) => void store.delete(key),
      clear: () => store.clear(),
      key: (index: number) => [...store.keys()][index] ?? null,
      get length() { return store.size },
    },
    configurable: true,
    writable: true,
  })
}

describe("agentPresets", () => {
  beforeEach(() => {
    installLocalStorage()
    localStorage.clear()
  })

  it("exposes pi, claude, codex in display order with exact verified defaults", () => {
    expect(AGENT_PRESETS.map((a) => a.id)).toEqual(["pi", "claude", "codex"])
    expect(commandForAgent("pi")).toBe("bunx pi-acp@0.0.31")
    expect(commandForAgent("claude")).toBe("bunx @agentclientprotocol/claude-agent-acp@0.58.1")
    expect(commandForAgent("codex")).toBe("bunx @agentclientprotocol/codex-acp@1.1.2")
    expect(AGENT_PRESETS.map((agent) => agent.verifiedCommand).some((command) => command.includes("@latest"))).toBe(false)
  })

  it("resolves verified, explicitly selected latest, and untrusted custom modes", () => {
    expect(resolveCuratedAgentCommand("codex", "verified")).toEqual({
      selectedPreset: "codex",
      commandMode: "verified",
      command: "bunx @agentclientprotocol/codex-acp@1.1.2",
      trustedAgentId: "codex",
    })
    expect(resolveCuratedAgentCommand("codex", "latest")).toEqual({
      selectedPreset: "codex",
      commandMode: "latest",
      command: "bunx @agentclientprotocol/codex-acp@latest",
      trustedAgentId: "codex",
    })
    expect(resolveCuratedAgentCommand("codex", "custom", "uvx wrapped-codex")).toEqual({
      selectedPreset: "codex",
      commandMode: "custom",
      command: "uvx wrapped-codex",
      trustedAgentId: null,
    })
  })

  it("uses the same 'Pi' label in AGENT_PRESETS and AGENT_VISUALS (no drift)", () => {
    expect(AGENT_PRESETS.find((a) => a.id === "pi")?.label).toBe("Pi")
    expect(AGENT_VISUALS.pi.label).toBe("Pi")
  })

  it("AGENT_VISUALS carries a glyph + brand color/soft var per known agentId", () => {
    expect(AGENT_VISUALS).toEqual({
      pi: { label: "Pi", glyph: "π", colorVar: "var(--agent-pi)", softVar: "var(--agent-pi-soft)" },
      claude: { label: "Claude", glyph: "C", colorVar: "var(--agent-claude)", softVar: "var(--agent-claude-soft)" },
      codex: { label: "Codex", glyph: "X", colorVar: "var(--agent-codex)", softVar: "var(--agent-codex-soft)" },
    })
    expect(CUSTOM_AGENT_VISUAL).toEqual({ colorVar: "var(--agent-custom)", softVar: "var(--agent-custom-soft)" })
  })

  it("agentDisplayName: known agentId → preset label; custom/undefined → agentLabel, else fallback", () => {
    expect(agentDisplayName("codex", "ignored", "Agent")).toBe("Codex")
    expect(agentDisplayName("custom", "My Custom Agent", "Agent")).toBe("My Custom Agent")
    expect(agentDisplayName(undefined, "  ", "Agent")).toBe("Agent")
  })

  it("resolves preset → command, custom → the custom text (empty custom falls back to pi)", () => {
    expect(commandForPreset("claude", "ignored")).toBe("bunx @agentclientprotocol/claude-agent-acp@0.58.1")
    expect(commandForPreset("custom", "uvx my-acp")).toBe("uvx my-acp")
    expect(commandForPreset("custom", "  ")).toBe("bunx pi-acp@0.0.31")
  })

  it("reverse-maps built-in verified/latest commands for labelling; unknown → custom", () => {
    expect(agentPresetForCommand("bunx @agentclientprotocol/codex-acp@latest")).toBe("codex")
    expect(agentPresetForCommand("bunx pi-acp@0.0.31")).toBe("pi")
    expect(agentPresetForCommand("uvx something-unknown")).toBe("custom")
  })

  it("resolves a trusted prewarm agent from last-used, Settings, then Pi", () => {
    writeJsonSetting(AGENT_SETTINGS_STORAGE_KEY, {
      preset: "claude",
      command: "ignored",
      traceEnabled: false,
      presetCommands: {
        pi: { mode: "verified", customCommand: "" },
        claude: { mode: "latest", customCommand: "" },
        codex: { mode: "verified", customCommand: "" },
      },
    })
    expect(resolvePrewarmAgentId()).toBe("claude")

    writeJsonSetting(AGENT_SETTINGS_STORAGE_KEY, {
      preset: "custom",
      command: "secret custom command",
      traceEnabled: false,
      presetCommands: {
        pi: { mode: "verified", customCommand: "" },
        claude: { mode: "verified", customCommand: "" },
        codex: { mode: "verified", customCommand: "" },
      },
    })
    expect(resolvePrewarmAgentId()).toBe("pi")

    rememberLastUsedCuratedAgent({
      selectedPreset: "codex",
      commandMode: "latest",
      trustedAgentId: "codex",
    })
    expect(resolvePrewarmAgentId()).toBe("codex")
  })

  it("never records or resolves Custom command modes for background prepare", () => {
    rememberLastUsedCuratedAgent({
      selectedPreset: "codex",
      commandMode: "custom",
      trustedAgentId: null,
    })
    expect(loadLastUsedCuratedAgent()).toBeNull()
    expect(localStorage.getItem(LAST_USED_CURATED_AGENT_STORAGE_KEY)).toBeNull()

    writeJsonSetting(AGENT_SETTINGS_STORAGE_KEY, {
      preset: "pi",
      command: "ignored",
      traceEnabled: false,
      presetCommands: {
        pi: { mode: "custom", customCommand: "contains-a-secret" },
        claude: { mode: "verified", customCommand: "" },
        codex: { mode: "verified", customCommand: "" },
      },
    })
    expect(resolvePrewarmAgentId()).toBeNull()
  })

  it("skips an unusable last-used Custom route and continues the trusted fallback chain", () => {
    localStorage.setItem(LAST_USED_CURATED_AGENT_STORAGE_KEY, "codex")
    writeJsonSetting(AGENT_SETTINGS_STORAGE_KEY, {
      preset: "claude",
      command: "ignored",
      traceEnabled: false,
      presetCommands: {
        pi: { mode: "verified", customCommand: "" },
        claude: { mode: "latest", customCommand: "" },
        codex: { mode: "custom", customCommand: "secret wrapper" },
      },
    })

    expect(resolvePrewarmAgentId()).toBe("claude")
  })
})
