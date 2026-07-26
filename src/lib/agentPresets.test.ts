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
  AGENT_PRESETS, AGENT_VISUALS, CUSTOM_AGENT_VISUAL, DEFAULT_AGENT_COMMAND,
  PINNED_ADAPTER_VERSIONS, agentDisplayName, agentPresetForCommand,
  commandForAgent, commandForPreset, pinnedAdapterCommand,
  resolveCuratedAgentCommand, resolveRuntimeCommand,
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

  it("exposes pi, claude, codex in display order with release-pinned defaults", () => {
    expect(AGENT_PRESETS.map((a) => a.id)).toEqual(["pi", "claude", "codex"])
    expect(commandForAgent("pi")).toBe("bunx pi-acp@0.0.32")
    expect(commandForAgent("claude")).toBe("bunx @agentclientprotocol/claude-agent-acp@0.62.0")
    expect(commandForAgent("codex")).toBe("bunx @agentclientprotocol/codex-acp@1.1.7")
  })

  // #37 AC 第 1 條：release build 必須用可重現、可稽核的 adapter 版本——curated
  // preset 不得在 runtime 解析 npm dist-tag。
  it("never spawns a curated preset through the @latest dist-tag", () => {
    for (const agent of AGENT_PRESETS) {
      expect(agent.latestCommand).not.toContain("@latest")
      expect(agent.latestCommand).toContain(`@${PINNED_ADAPTER_VERSIONS[agent.id]}`)
    }
  })

  // 版本升級：釘選常數是唯一權威來源，preset command 與 spawn fallback 都由它
  // 組出；升級後舊版本的指令字串不再被認成 curated preset。
  it("derives every curated command from PINNED_ADAPTER_VERSIONS", () => {
    for (const agent of AGENT_PRESETS) {
      expect(agent.latestCommand).toBe(pinnedAdapterCommand(agent.id))
    }
    expect(DEFAULT_AGENT_COMMAND).toBe(pinnedAdapterCommand("pi"))
    expect(agentPresetForCommand(pinnedAdapterCommand("claude"))).toBe("claude")
    expect(agentPresetForCommand("bunx @agentclientprotocol/claude-agent-acp@0.61.0")).toBe("custom")
  })

  it("resolves trusted latest and untrusted custom modes", () => {
    expect(resolveCuratedAgentCommand("codex", "latest")).toEqual({
      selectedPreset: "codex",
      commandMode: "latest",
      command: "bunx @agentclientprotocol/codex-acp@1.1.7",
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
    expect(commandForPreset("claude", "ignored")).toBe("bunx @agentclientprotocol/claude-agent-acp@0.62.0")
    expect(commandForPreset("custom", "uvx my-acp")).toBe("uvx my-acp")
    expect(commandForPreset("custom", "  ")).toBe("bunx pi-acp@0.0.32")
  })

  it("reverse-maps built-in latest commands for labelling; removed pins and unknown commands are custom", () => {
    expect(agentPresetForCommand("bunx @agentclientprotocol/codex-acp@1.1.7")).toBe("codex")
    expect(agentPresetForCommand("bunx pi-acp@0.0.31")).toBe("custom")
    expect(agentPresetForCommand("uvx something-unknown")).toBe("custom")
  })

  it("resolves a trusted prewarm agent from last-used, Settings, then Pi", () => {
    writeJsonSetting(AGENT_SETTINGS_STORAGE_KEY, {
      preset: "claude",
      command: "ignored",
      traceEnabled: false,
      presetCommands: {
        pi: { mode: "latest", customCommand: "" },
        claude: { mode: "latest", customCommand: "" },
        codex: { mode: "latest", customCommand: "" },
      },
    })
    expect(resolvePrewarmAgentId()).toBe("claude")

    writeJsonSetting(AGENT_SETTINGS_STORAGE_KEY, {
      preset: "custom",
      command: "secret custom command",
      traceEnabled: false,
      presetCommands: {
        pi: { mode: "latest", customCommand: "" },
        claude: { mode: "latest", customCommand: "" },
        codex: { mode: "latest", customCommand: "" },
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
        claude: { mode: "latest", customCommand: "" },
        codex: { mode: "latest", customCommand: "" },
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
        pi: { mode: "latest", customCommand: "" },
        claude: { mode: "latest", customCommand: "" },
        codex: { mode: "custom", customCommand: "secret wrapper" },
      },
    })

    expect(resolvePrewarmAgentId()).toBe("claude")
  })
})

describe("resolveRuntimeCommand (#15 runtime fallback)", () => {
  const all = { bunx: true, deno: true, node: true, npx: true }

  it("keeps a bunx command unchanged when bunx is available", () => {
    expect(resolveRuntimeCommand("bunx pi-acp@latest", all)).toEqual({
      kind: "unchanged",
      command: "bunx pi-acp@latest",
    })
  })

  it("rewrites bunx to npx -y when bunx is missing and npx exists", () => {
    expect(
      resolveRuntimeCommand("bunx pi-acp@latest", { ...all, bunx: false }),
    ).toEqual({
      kind: "fallback",
      command: "npx -y pi-acp@latest",
      runtime: "node",
    })
    expect(
      resolveRuntimeCommand(
        "bunx @agentclientprotocol/claude-agent-acp@latest",
        { ...all, bunx: false, deno: false },
      ),
    ).toEqual({
      kind: "fallback",
      command: "npx -y @agentclientprotocol/claude-agent-acp@latest",
      runtime: "node",
    })
  })

  it("does not fall back to deno (unverified adapter compatibility)", () => {
    // bun 缺席、deno 存在但 npx 缺席：deno 順位仍刻意跳過——不得產生 deno 指令。
    // #15 起結果從含糊的 unavailable 收斂為顯性的 unsupported-runtime：斷言變嚴
    //（多驗 kind 與 runtime 標籤），「不 fallback 到 deno」的意圖不變。
    const resolution = resolveRuntimeCommand("bunx pi-acp@latest", {
      bunx: false, deno: true, node: false, npx: false,
    })
    expect(resolution).toEqual({
      kind: "unsupported-runtime",
      command: "bunx pi-acp@latest",
      runtime: "deno",
    })
    expect(resolution.command).not.toContain("deno")
  })

  it("keeps npx ahead of the deno notice when both are present", () => {
    // deno 存在不得攔截既有的 node fallback——優先級仍是 bun → node。
    expect(
      resolveRuntimeCommand("bunx pi-acp@latest", {
        bunx: false, deno: true, node: true, npx: true,
      }),
    ).toEqual({
      kind: "fallback",
      command: "npx -y pi-acp@latest",
      runtime: "node",
    })
  })

  it("passes non-bunx commands through untouched regardless of runtimes", () => {
    const none = { bunx: false, deno: false, node: false, npx: false }
    expect(resolveRuntimeCommand("node my-agent.js", none)).toEqual({
      kind: "unchanged",
      command: "node my-agent.js",
    })
  })

  it("reports unavailable when neither bunx nor npx exists", () => {
    expect(
      resolveRuntimeCommand("bunx pi-acp@latest", {
        bunx: false, deno: false, node: true, npx: false,
      }),
    ).toEqual({ kind: "unavailable", command: "bunx pi-acp@latest" })
  })
})
