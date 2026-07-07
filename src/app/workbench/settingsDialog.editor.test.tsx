import { beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import { SettingsDialog } from "@/app/workbench/SettingsDialog"
import { loadEditorSettings, useEditorSettingsStore } from "@/state/editorSettingsStore"

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}))

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

function renderDialog() {
  return render(
    <SettingsDialog
      open
      onOpenChange={() => {}}
      theme="light"
      onThemeChange={() => {}}
      initialSection="editor"
    />,
  )
}

beforeEach(() => {
  installLocalStorage()
  cleanup()
  vi.clearAllMocks()
  useEditorSettingsStore.setState({ fontSize: 13, minimap: false })
})

describe("Settings · Editor pane", () => {
  it("font-size Segmented reflects the store and writes the choice through", () => {
    renderDialog()
    // Default 13 is the selected radio.
    expect(screen.getByRole("radio", { name: "13" })).toHaveAttribute("aria-checked", "true")

    fireEvent.click(screen.getByRole("radio", { name: "14" }))

    expect(useEditorSettingsStore.getState().fontSize).toBe(14)
    expect(loadEditorSettings().fontSize).toBe(14)
    expect(screen.getByRole("radio", { name: "14" })).toHaveAttribute("aria-checked", "true")
    expect(screen.getByRole("radio", { name: "13" })).toHaveAttribute("aria-checked", "false")
  })

  it("minimap switch reflects the store and writes the choice through", () => {
    renderDialog()
    const toggle = screen.getByRole("switch", { name: "Show minimap" })
    expect(toggle).toHaveAttribute("aria-checked", "false")

    fireEvent.click(toggle)

    expect(useEditorSettingsStore.getState().minimap).toBe(true)
    expect(loadEditorSettings().minimap).toBe(true)
    expect(screen.getByRole("switch", { name: "Show minimap" })).toHaveAttribute("aria-checked", "true")
  })

  it("persists across a close/reopen: a fresh load from localStorage drives the reopened pane", () => {
    renderDialog()
    fireEvent.click(screen.getByRole("radio", { name: "15" }))
    fireEvent.click(screen.getByRole("switch", { name: "Show minimap" }))
    cleanup()

    // Simulate an app restart: wipe the in-memory store, then re-hydrate it from
    // localStorage exactly as the store initializer does on a fresh boot.
    useEditorSettingsStore.setState({ fontSize: 13, minimap: false })
    useEditorSettingsStore.setState(loadEditorSettings())

    renderDialog()
    expect(screen.getByRole("radio", { name: "15" })).toHaveAttribute("aria-checked", "true")
    expect(screen.getByRole("switch", { name: "Show minimap" })).toHaveAttribute("aria-checked", "true")
  })
})
