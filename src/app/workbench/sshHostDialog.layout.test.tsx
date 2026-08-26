import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/ipc", () => ({
  sshConnect: vi.fn(),
  sshDisconnect: vi.fn(),
}))
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }))

import { SshNavContent } from "@/app/workbench/SshNavContent"
import { useSshStore } from "@/state/sshStore"

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

describe("SSH host dialog layout", () => {
  beforeEach(() => {
    installLocalStorage()
    localStorage.clear()
    useSshStore.setState({ hosts: [], sessions: {}, activeHostId: null, pendingAuthHostId: null })
  })

  afterEach(() => {
    cleanup()
  })

  it("scrolls key-based host form fields while keeping footer actions fixed", () => {
    render(<SshNavContent />)
    fireEvent.click(screen.getByText("New host"))
    fireEvent.click(screen.getByRole("radio", { name: "Key file" }))

    const dialog = screen.getByTestId("ssh-host-dialog")
    expect(dialog).not.toHaveAttribute("data-dialog-size-id")
    expect(dialog.style.width).toBe("")
    expect(dialog.style.height).toBe("")
    expect(dialog.className).toMatch(/sm:max-w-\[420px\]/)
    expect(dialog.className).not.toMatch(/max-w-none/)
    expect(dialog.querySelectorAll('[data-slot="dialog-resize-handle"]')).toHaveLength(0)

    const body = screen.getByTestId("ssh-host-body")
    expect(body).toHaveAttribute("data-slot", "scroll-area")
    expect(body.className).toMatch(/min-h-0/)
    expect(body.className).toMatch(/flex-1/)

    expect(body).toContainElement(screen.getByLabelText("Host"))
    expect(body).toContainElement(screen.getByLabelText("Private key path"))

    const add = screen.getByRole("button", { name: "Add" })
    expect(body.contains(add)).toBe(false)
  })

  it("uses the compact alert-sized contract for the SSH password prompt", () => {
    const host = useSshStore.getState().addHost({
      name: "web",
      host: "example.com",
      port: 22,
      user: "root",
      authKind: "password",
    })
    useSshStore.setState({ pendingAuthHostId: host.id })

    render(<SshNavContent />)

    const dialog = screen.getByTestId("ssh-password-dialog")
    expect(dialog).not.toHaveAttribute("data-dialog-size-id")
    expect(dialog.style.width).toBe("")
    expect(dialog.style.height).toBe("")
    expect(dialog.className).toMatch(/sm:max-w-\[420px\]/)
    expect(dialog.className).not.toMatch(/max-w-none/)
    expect(dialog.querySelectorAll('[data-slot="dialog-resize-handle"]')).toHaveLength(0)
  })
})
