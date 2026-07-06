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
    const command = screen.getByLabelText("自訂 command")

    expect(preset).toHaveValue("pi")
    expect(command).toHaveValue("bunx pi-acp")
    expect(command).toBeDisabled()

    fireEvent.change(preset, { target: { value: "custom" } })
    fireEvent.change(command, { target: { value: "uvx my-acp" } })

    expect(localStorage.getItem(AGENT_SETTINGS_STORAGE_KEY)).toBe(
      JSON.stringify({ preset: "custom", command: "uvx my-acp", traceEnabled: false }),
    )

    cleanup()
    renderDialog()

    expect(screen.getByRole("combobox", { name: "Agent preset" })).toHaveValue("custom")
    expect(screen.getByLabelText("自訂 command")).toHaveValue("uvx my-acp")
    expect(screen.getByLabelText("自訂 command")).toBeEnabled()
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
