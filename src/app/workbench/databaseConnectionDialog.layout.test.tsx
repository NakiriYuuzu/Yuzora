import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/ipc", () => ({
  dbProfileList: vi.fn(async () => ({ profiles: [], recovery: [] })),
  dbProfileImportLegacy: vi.fn(async () => ({ profiles: [], recovery: [] })),
  dbProfileCreate: vi.fn(),
  dbProfileUpdate: vi.fn(),
  dbProfileForget: vi.fn(),
  dbProfileRemoveCredential: vi.fn(),
  dbProfileRecover: vi.fn(),
  dbProfileOpen: vi.fn(),
  dbTestConnection: vi.fn(),
  dbList: vi.fn(async () => []),
  dbColumns: vi.fn(async () => []),
  dbQueryRun: vi.fn(),
}))

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(async () => null),
}))

import { DatabaseNavContent } from "@/app/workbench/DatabaseNavContent"
import { useDbStore } from "@/state/dbStore"

function installLocalStorage(): void {
  const store = new Map<string, string>()
  Object.defineProperty(globalThis, "localStorage", {
    value: {
      getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
      setItem: (k: string, v: string) => void store.set(k, String(v)),
      removeItem: (k: string) => void store.delete(k),
      clear: () => store.clear(),
      key: (i: number) => [...store.keys()][i] ?? null,
      get length() {
        return store.size
      },
    },
    configurable: true,
    writable: true,
  })
}

describe("Database connection dialog layout", () => {
  beforeEach(() => {
    installLocalStorage()
    localStorage.clear()
    useDbStore.getState().reset()
  })

  afterEach(() => {
    cleanup()
  })

  it("wraps the long form body in ScrollArea while keeping footer actions outside", async () => {
    render(<DatabaseNavContent />)
    fireEvent.click(screen.getByText("New connection…"))
    fireEvent.click(screen.getByText("PostgreSQL"))

    const dialog = await screen.findByTestId("database-connection-dialog")
    expect(dialog).not.toHaveAttribute("data-dialog-size-id")
    expect(dialog.style.width).toBe("")
    expect(dialog.style.height).toBe("")
    expect(dialog.className).toMatch(/sm:max-w-\[420px\]/)
    expect(dialog.className).not.toMatch(/max-w-none/)
    expect(dialog.querySelectorAll('[data-slot="dialog-resize-handle"]')).toHaveLength(0)

    const body = screen.getByTestId("database-connection-body")
    expect(body).toHaveAttribute("data-slot", "scroll-area")
    expect(body.className).toMatch(/min-h-0/)
    expect(body.className).toMatch(/flex-1/)

    expect(body).toContainElement(screen.getByLabelText("Host"))
    expect(body).toContainElement(screen.getByText("Transport"))
    expect(body).toContainElement(screen.getByText("Verify certificate and hostname"))

    const save = screen.getByRole("button", { name: "Save and Connect" })
    const test = screen.getByRole("button", { name: "Test connection" })
    expect(body.contains(save)).toBe(false)
    expect(body.contains(test)).toBe(false)
  })
})
