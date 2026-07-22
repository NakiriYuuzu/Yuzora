import { beforeEach, describe, expect, it } from "vitest"
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"

import {
  AGENT_SETTINGS_STORAGE_KEY,
  SettingsDialog,
} from "@/app/workbench/SettingsDialog"
import { AGENT_VERSION_STORAGE_KEY } from "@/agent/agentVersions"
import type { AgentLatestVersion } from "@/lib/ipc"
import { setCachedBuiltinPiAdapterCommandForTests } from "@/lib/platform"

let agentTraceCalls: boolean[] = []
let latestVersions: AgentLatestVersion[] = []
let latestVersionCalls = 0

function installLocalStorage(): void {
  const store = new Map<string, string>()
  const mock = {
    getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
    setItem: (k: string, v: string) => void store.set(k, String(v)),
    removeItem: (k: string) => void store.delete(k),
    clear: () => store.clear(),
    key: (i: number) => [...store.keys()][i] ?? null,
    get length() {
      return store.size
    },
  }
  Object.defineProperty(globalThis, "localStorage", {
    value: mock,
    configurable: true,
    writable: true,
  })
}

function setupIpc() {
  mockIPC((cmd, args) => {
    const a = (args ?? {}) as Record<string, unknown>
    if (cmd === "agent_set_trace") {
      agentTraceCalls.push(a.enabled as boolean)
    }
    if (cmd === "agent_latest_versions") {
      latestVersionCalls += 1
      return latestVersions
    }
    return undefined
  })
}

function renderDialog() {
  return render(
    <SettingsDialog
      open
      onOpenChange={() => {}}
      theme="light"
      onThemeChange={() => {}}
      initialSection="agent"
    />,
  )
}

beforeEach(() => {
  cleanup()
  clearMocks()
  installLocalStorage()
  localStorage.clear()
  agentTraceCalls = []
  latestVersions = []
  latestVersionCalls = 0
  setupIpc()
})

describe("SettingsDialog agent section", () => {
  it("renders pi as the default preset and persists a custom command", () => {
    renderDialog()

    expect(screen.getByRole("heading", { name: "Agent" })).toBeInTheDocument()
    const preset = screen.getByRole("combobox", { name: "Agent preset" })
    const command = screen.getByLabelText("Command")

    expect(preset).toHaveValue("pi")
    expect(screen.getByRole("combobox", { name: "Command mode" })).toHaveValue("latest")
    expect(screen.queryByRole("option", { name: "Verified" })).not.toBeInTheDocument()
    expect(command).toHaveValue("bunx pi-acp@latest")
    expect(command).toBeDisabled()

    fireEvent.change(preset, { target: { value: "custom" } })
    fireEvent.change(command, { target: { value: "uvx my-acp" } })

    expect(JSON.parse(localStorage.getItem(AGENT_SETTINGS_STORAGE_KEY) ?? "{}")).toMatchObject({
      preset: "custom",
      command: "uvx my-acp",
      traceEnabled: false,
    })

    cleanup()
    renderDialog()

    expect(screen.getByRole("combobox", { name: "Agent preset" })).toHaveValue("custom")
    expect(screen.getByLabelText("Command")).toHaveValue("uvx my-acp")
    expect(screen.getByLabelText("Command")).toBeEnabled()
    expect(screen.queryByRole("combobox", { name: "Command mode" })).not.toBeInTheDocument()
  })

  it("persists independent latest/custom modes for curated presets", () => {
    renderDialog()
    const preset = screen.getByRole("combobox", { name: "Agent preset" })
    const command = screen.getByLabelText("Command")
    fireEvent.change(preset, { target: { value: "claude" } })
    const mode = screen.getByRole("combobox", { name: "Command mode" })
    expect(mode).toHaveValue("latest")
    expect(command).toHaveValue("bunx @agentclientprotocol/claude-agent-acp@latest")
    expect(command).toBeDisabled()

    fireEvent.change(preset, { target: { value: "codex" } })
    expect(mode).toHaveValue("latest")
    expect(command).toHaveValue("bunx @agentclientprotocol/codex-acp@latest")
    fireEvent.change(mode, { target: { value: "custom" } })
    expect(command).toBeEnabled()
    fireEvent.change(command, { target: { value: "uvx wrapped-codex" } })

    fireEvent.change(preset, { target: { value: "claude" } })
    expect(mode).toHaveValue("latest")
    expect(command).toHaveValue("bunx @agentclientprotocol/claude-agent-acp@latest")
    fireEvent.change(preset, { target: { value: "codex" } })
    expect(mode).toHaveValue("custom")
    expect(command).toHaveValue("uvx wrapped-codex")

    const stored = JSON.parse(localStorage.getItem(AGENT_SETTINGS_STORAGE_KEY) ?? "{}")
    expect(stored.presetCommands).toMatchObject({
      claude: { mode: "latest" },
      codex: { mode: "custom", customCommand: "uvx wrapped-codex" },
    })
  })

  // P5：pi 專屬的雙 runtime 選擇器——只在 Pi＋latest 顯示；builtin 為預設，
  // 切 community 立即持久化；jsdom（非 Tauri）builtin cache 為 null，
  // effective command 誠實顯示會退回的社群 command。
  it("shows the Pi runtime selector only for Pi latest mode and persists the choice", () => {
    setCachedBuiltinPiAdapterCommandForTests('node "/App/adapters/yuzora-pi-acp/index.mjs"')
    try {
      renderDialog()
      const runtime = screen.getByRole("combobox", { name: "Pi runtime" })
      expect(runtime).toHaveValue("builtin")
      expect(screen.getByLabelText("Command")).toHaveValue('node "/App/adapters/yuzora-pi-acp/index.mjs"')

      fireEvent.change(runtime, { target: { value: "community" } })
      expect(screen.getByLabelText("Command")).toHaveValue("bunx pi-acp@latest")
      expect(JSON.parse(localStorage.getItem(AGENT_SETTINGS_STORAGE_KEY) ?? "{}")).toMatchObject({
        piRuntime: "community",
      })

      fireEvent.change(screen.getByRole("combobox", { name: "Agent preset" }), { target: { value: "claude" } })
      expect(screen.queryByRole("combobox", { name: "Pi runtime" })).not.toBeInTheDocument()

      fireEvent.change(screen.getByRole("combobox", { name: "Agent preset" }), { target: { value: "pi" } })
      fireEvent.change(screen.getByRole("combobox", { name: "Command mode" }), { target: { value: "custom" } })
      expect(screen.queryByRole("combobox", { name: "Pi runtime" })).not.toBeInTheDocument()
    } finally {
      setCachedBuiltinPiAdapterCommandForTests(null)
    }
  })

  it("restores the custom command after switching away and back to custom preset", () => {
    renderDialog()
    const preset = screen.getByRole("combobox", { name: "Agent preset" })
    const command = screen.getByLabelText("Command")

    fireEvent.change(preset, { target: { value: "custom" } })
    fireEvent.change(command, { target: { value: "uvx my-acp" } })
    fireEvent.change(preset, { target: { value: "claude" } })
    fireEvent.change(preset, { target: { value: "custom" } })

    expect(screen.getByLabelText("Command")).toHaveValue("uvx my-acp")
    expect(JSON.parse(localStorage.getItem(AGENT_SETTINGS_STORAGE_KEY) ?? "{}")).toMatchObject({
      preset: "custom",
      command: "uvx my-acp",
      traceEnabled: false,
    })
  })

  it("loads the legacy settings envelope and migrates verified modes to latest", () => {
    localStorage.setItem(
      AGENT_SETTINGS_STORAGE_KEY,
      JSON.stringify({
        preset: "codex",
        command: "",
        traceEnabled: true,
        presetCommands: { codex: { mode: "verified", customCommand: "" } },
      }),
    )

    renderDialog()

    expect(screen.getByRole("combobox", { name: "Agent preset" })).toHaveValue("codex")
    expect(screen.getByRole("combobox", { name: "Command mode" })).toHaveValue("latest")
    expect(screen.getByLabelText("Command")).toHaveValue("bunx @agentclientprotocol/codex-acp@latest")
    expect(screen.getByRole("switch", { name: "ACP trace" })).toBeChecked()
  })

  it("persists ACP trace and calls agent_set_trace", async () => {
    renderDialog()

    const trace = screen.getByRole("switch", { name: "ACP trace" })
    expect(trace).not.toBeChecked()

    fireEvent.click(trace)

    await waitFor(() => expect(agentTraceCalls).toEqual([true]))
    expect(JSON.parse(localStorage.getItem(AGENT_SETTINGS_STORAGE_KEY) ?? "{}")).toMatchObject({
      traceEnabled: true,
    })
  })

  it("shows an ACP update only when the last successful version differs from registry latest", async () => {
    localStorage.setItem(AGENT_VERSION_STORAGE_KEY, JSON.stringify({ pi: "0.0.31" }))
    latestVersions = [{ agentId: "pi", version: "0.0.32" }]

    renderDialog()

    expect(await screen.findByRole("status")).toHaveTextContent("ACP update available: v0.0.31 → v0.0.32")
    expect(screen.getByRole("status")).toHaveTextContent("Restart the agent to use the latest version")
  })

  it("does not claim an ACP update when the version is current or unknown", async () => {
    localStorage.setItem(AGENT_VERSION_STORAGE_KEY, JSON.stringify({ pi: "v0.0.32" }))
    latestVersions = [{ agentId: "pi", version: "0.0.32" }]

    renderDialog()

    await waitFor(() => expect(latestVersionCalls).toBe(1))
    expect(screen.queryByRole("status")).not.toBeInTheDocument()
  })
})
