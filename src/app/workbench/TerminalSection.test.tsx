import { afterAll, beforeEach, describe, expect, it, vi } from "vitest"
import { fireEvent, render, screen, waitFor } from "@testing-library/react"

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
})
