import { afterAll, beforeEach, describe, expect, it, vi } from "vitest"
import { fireEvent, render, screen, waitFor } from "@testing-library/react"

import { reloadTerminalSettingsStore } from "@/state/terminalSettingsStore"
import { loadTerminalSettings } from "./settingsStorage"
import { TerminalSection } from "./TerminalSection"

const ipcMock = vi.hoisted(() => ({
  ptyListProfiles: vi.fn(async () => [
    {
      id: "wsl:Ubuntu",
      name: "WSL: Ubuntu",
      shell: "C:\\Windows\\System32\\wsl.exe",
      args: ["--distribution", "Ubuntu"],
      kind: "wsl" as const,
      cwdStrategy: "wsl" as const,
    },
  ]),
}))
const originalUserAgent = navigator.userAgent

vi.mock("@/lib/ipc", () => ({
  ptyListProfiles: ipcMock.ptyListProfiles,
}))

function installLocalStorage(): void {
  const store = new Map<string, string>()
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (key: string) => store.get(key) ?? null,
      setItem: (key: string, value: string) => void store.set(key, String(value)),
      removeItem: (key: string) => void store.delete(key),
      clear: () => store.clear(),
      key: (index: number) => [...store.keys()][index] ?? null,
      get length() {
        return store.size
      },
    },
  })
}

beforeEach(() => {
  installLocalStorage()
  localStorage.clear()
  reloadTerminalSettingsStore()
  Object.defineProperty(navigator, "userAgent", {
    configurable: true,
    value: "Mozilla/5.0 (Windows NT 10.0; Win64; x64)",
  })
  ipcMock.ptyListProfiles.mockClear()
})

afterAll(() => {
  Object.defineProperty(navigator, "userAgent", {
    configurable: true,
    value: originalUserAgent,
  })
})

describe("TerminalSection profiles", () => {
  it("persists a detected WSL profile as structured executable and argv", async () => {
    render(<TerminalSection />)

    const select = screen.getByRole("combobox", { name: "Default profile" })
    await waitFor(() => expect(screen.getByRole("option", { name: "WSL: Ubuntu" })).toBeVisible())
    fireEvent.change(select, { target: { value: "wsl:Ubuntu" } })

    expect(loadTerminalSettings()).toMatchObject({
      defaultProfile: {
        id: "wsl:Ubuntu",
        shell: "C:\\Windows\\System32\\wsl.exe",
        args: ["--distribution", "Ubuntu"],
        kind: "wsl",
        cwdStrategy: "wsl",
      },
    })
  })

  it("keeps spaces inside a custom argv line and persists TUI IME anchoring", () => {
    render(<TerminalSection />)

    fireEvent.change(screen.getByRole("combobox", { name: "Default profile" }), {
      target: { value: "custom" },
    })
    fireEvent.change(screen.getByRole("textbox", { name: "Custom executable" }), {
      target: { value: "pwsh.exe" },
    })
    fireEvent.change(screen.getByRole("textbox", { name: "Custom arguments" }), {
      target: { value: "-Command\nWrite-Output 'hello world'" },
    })
    fireEvent.click(screen.getByRole("radio", { name: "TUI input box" }))

    expect(loadTerminalSettings()).toMatchObject({
      defaultProfile: {
        id: "custom",
        shell: "pwsh.exe",
        args: ["-Command", "Write-Output 'hello world'"],
      },
      imeAnchorMode: "tui",
    })
  })

  it("persists terminal font size changes for live terminal subscribers", () => {
    render(<TerminalSection />)

    fireEvent.change(screen.getByRole("slider", { name: "Terminal text size" }), {
      target: { value: "18" },
    })

    expect(loadTerminalSettings().fontSize).toBe(18)
    expect(screen.getByText("18 px")).toBeInTheDocument()
  })
})

  it("hides WSL mapping outside Windows and keeps a neutral executable placeholder", async () => {
    Object.defineProperty(navigator, "userAgent", {
      configurable: true,
      value: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15",
    })
    render(<TerminalSection />)

    expect(screen.queryByRole("radio", { name: "WSL mapping" })).toBeNull()
    expect(screen.getByRole("textbox", { name: "Custom executable" })).toHaveAttribute(
      "placeholder",
      "Path to executable",
    )
    expect(screen.queryByPlaceholderText(/Program Files/)).toBeNull()
  })

  it("shows WSL mapping on Windows", async () => {
    render(<TerminalSection />)
    expect(screen.getByRole("radio", { name: "WSL mapping" })).toBeInTheDocument()
  })
