import { beforeEach, describe, expect, it } from "vitest"
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"

import {
  AGENT_SETTINGS_STORAGE_KEY,
  SettingsDialog,
} from "@/app/workbench/SettingsDialog"

let agentTraceCalls: boolean[] = []

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
  setupIpc()
})

describe("SettingsDialog agent section", () => {
  it("renders pi as the default preset and persists a custom command", () => {
    renderDialog()

    expect(screen.getByRole("heading", { name: "Agent" })).toBeInTheDocument()
    const preset = screen.getByRole("combobox", { name: "Agent preset" })
    const command = screen.getByLabelText("Command")

    expect(preset).toHaveValue("pi")
    expect(screen.getByRole("combobox", { name: "Command mode" })).toHaveValue("verified")
    expect(command).toHaveValue("bunx pi-acp@0.0.31")
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

  it("persists independent verified/latest/custom modes for curated presets", () => {
    renderDialog()
    const preset = screen.getByRole("combobox", { name: "Agent preset" })
    const command = screen.getByLabelText("Command")
    fireEvent.change(preset, { target: { value: "claude" } })
    const mode = screen.getByRole("combobox", { name: "Command mode" })
    expect(mode).toHaveValue("verified")
    expect(command).toHaveValue("bunx @agentclientprotocol/claude-agent-acp@0.58.1")
    expect(command).toBeDisabled()

    fireEvent.change(mode, { target: { value: "latest" } })
    expect(command).toHaveValue("bunx @agentclientprotocol/claude-agent-acp@latest")
    expect(command).toBeDisabled()

    fireEvent.change(preset, { target: { value: "codex" } })
    expect(mode).toHaveValue("verified")
    expect(command).toHaveValue("bunx @agentclientprotocol/codex-acp@1.1.2")
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

  it("loads the legacy settings envelope and migrates curated presets to verified defaults", () => {
    localStorage.setItem(
      AGENT_SETTINGS_STORAGE_KEY,
      JSON.stringify({ preset: "codex", command: "", traceEnabled: true }),
    )

    renderDialog()

    expect(screen.getByRole("combobox", { name: "Agent preset" })).toHaveValue("codex")
    expect(screen.getByRole("combobox", { name: "Command mode" })).toHaveValue("verified")
    expect(screen.getByLabelText("Command")).toHaveValue("bunx @agentclientprotocol/codex-acp@1.1.2")
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
})
