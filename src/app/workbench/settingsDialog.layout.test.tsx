import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { SettingsDialog } from "@/app/workbench/SettingsDialog"

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

function setViewport(width: number, height: number) {
  Object.defineProperty(window, "innerWidth", {
    configurable: true,
    writable: true,
    value: width,
  })
  Object.defineProperty(window, "innerHeight", {
    configurable: true,
    writable: true,
    value: height,
  })
  window.dispatchEvent(new Event("resize"))
}

describe("Settings dialog layout at constrained height", () => {
  beforeEach(() => {
    installLocalStorage()
    setViewport(900, 700)
  })

  afterEach(() => {
    cleanup()
  })

  it("scrolls the sidebar section list while keeping the version footer fixed", () => {
    // Force the dialog to its 640x440 minimum so ten 37px rows cannot fit.
    localStorage.setItem(
      "yuzora.dialog-sizes.v1",
      JSON.stringify({
        version: 1,
        sizes: { settings: { widthRatio: 0.01, heightRatio: 0.01 } },
      }),
    )

    render(
      <SettingsDialog
        open
        onOpenChange={() => {}}
        theme="light"
        onThemeChange={() => {}}
        initialSection="editor"
      />,
    )

    const content = document.querySelector(
      '[data-slot="dialog-content"][data-dialog-size-id="settings"]',
    ) as HTMLElement
    expect(content.style.width).toBe("640px")
    expect(content.style.height).toBe("440px")

    const sidebarScroll = screen.getByTestId("settings-sidebar-scroll")
    expect(sidebarScroll).toHaveAttribute("data-slot", "scroll-area")
    expect(sidebarScroll.className).toMatch(/min-h-0/)
    expect(sidebarScroll.className).toMatch(/flex-1/)

    const footer = screen.getByTestId("settings-sidebar-footer")
    expect(footer.className).toMatch(/shrink-0/)
    expect(footer.compareDocumentPosition(sidebarScroll) & Node.DOCUMENT_POSITION_PRECEDING).toBeTruthy()

    // All section rows remain mounted inside the scroll owner, not clipped out of the tree.
    expect(screen.getByRole("tab", { name: "Appearance" })).toBeInTheDocument()
    expect(screen.getByRole("tab", { name: "About & Updates" })).toBeInTheDocument()
  })

  it("supports vertical navigation, search focus and Escape clearing", async () => {
    const onOpenChange = vi.fn()
    render(<SettingsDialog open onOpenChange={onOpenChange} theme="light" onThemeChange={() => {}} />)
    const search = screen.getByRole("textbox", { name: "Search settings" })
    await waitFor(() => expect(search).toHaveFocus())
    expect(screen.getByRole("tablist")).toHaveAttribute("aria-orientation", "vertical")
    const appearance = screen.getByRole("tab", { name: "Appearance" })
    appearance.focus()
    fireEvent.keyDown(appearance, { key: "ArrowDown" })
    await waitFor(() => expect(screen.getByRole("tab", { name: "Editor" })).toHaveFocus())
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "f", ctrlKey: true })
    expect(search).toHaveFocus()
    fireEvent.change(search, { target: { value: "editor font" } })
    expect(screen.getByRole("status")).toHaveTextContent("editor font")
    fireEvent.keyDown(search, { key: "Escape" })
    expect(search).toHaveValue("")
    expect(onOpenChange).not.toHaveBeenCalled()
  })

})
