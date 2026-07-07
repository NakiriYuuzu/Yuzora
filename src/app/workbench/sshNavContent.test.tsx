import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

vi.mock("@/lib/ipc", () => ({
  sshConnect: vi.fn(),
  sshDisconnect: vi.fn()
}))
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }))

import { sshDisconnect } from "@/lib/ipc"
import { SshNavContent } from "@/app/workbench/SshNavContent"
import { useSshStore, type NewSshHost } from "@/state/sshStore"

const mockDisconnect = vi.mocked(sshDisconnect)

// The Bun-hosted test runtime injects an empty `localStorage` global with no
// Storage methods; install a minimal in-memory Storage so sshStore's
// load/save round-trip runs for real (mirrors sshStore.test.ts).
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
    }
  }
  Object.defineProperty(globalThis, "localStorage", {
    value: mock,
    configurable: true,
    writable: true
  })
}

const passwordHost: NewSshHost = {
  name: "web",
  host: "example.com",
  port: 22,
  user: "root",
  authKind: "password"
}

const keyHost: NewSshHost = {
  name: "box",
  host: "10.0.0.5",
  port: 2222,
  user: "deploy",
  authKind: "key",
  keyPath: "/home/u/.ssh/id_ed25519"
}

beforeEach(() => {
  installLocalStorage()
  localStorage.clear()
  vi.clearAllMocks()
  useSshStore.setState({ hosts: [], sessions: {}, activeHostId: null, pendingAuthHostId: null })
})

afterEach(() => {
  cleanup()
})

describe("SshNavContent edit host", () => {
  it("opens the edit dialog prefilled and saves via updateHost", () => {
    const host = useSshStore.getState().addHost(keyHost)
    render(<SshNavContent />)

    fireEvent.click(screen.getByLabelText("Edit box"))

    expect(screen.getByText("Edit SSH host")).toBeInTheDocument()
    const nameInput = screen.getByLabelText("Name") as HTMLInputElement
    expect(nameInput.value).toBe("box")
    // Port arrives as a string (String(host.port)) so the numeric input renders.
    expect((screen.getByLabelText("Port") as HTMLInputElement).value).toBe("2222")

    fireEvent.change(nameInput, { target: { value: "box-renamed" } })
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    expect(useSshStore.getState().hosts).toHaveLength(1)
    expect(useSshStore.getState().hosts.find((h) => h.id === host.id)!.name).toBe("box-renamed")
  })

  it("editing a connected host keeps the live session (no disconnect)", () => {
    const host = useSshStore.getState().addHost(passwordHost)
    useSshStore.setState({
      sessions: {
        [host.id]: {
          hostId: host.id,
          sessionId: "sess-1",
          status: "connected",
          fingerprint: "SHA256:fp",
          knownHost: false,
          error: null
        }
      },
      activeHostId: host.id
    })
    render(<SshNavContent />)

    fireEvent.click(screen.getByLabelText("Edit web"))
    fireEvent.change(screen.getByLabelText("Name"), { target: { value: "web2" } })
    fireEvent.click(screen.getByRole("button", { name: "Save" }))

    const session = useSshStore.getState().sessions[host.id]
    expect(session.status).toBe("connected")
    expect(session.sessionId).toBe("sess-1")
    expect(mockDisconnect).not.toHaveBeenCalled()
    expect(useSshStore.getState().hosts.find((h) => h.id === host.id)!.name).toBe("web2")
  })
})

describe("SshNavContent inline delete confirm", () => {
  it("requires a confirm click before removing the host", () => {
    useSshStore.getState().addHost(passwordHost)
    render(<SshNavContent />)

    expect(screen.queryByLabelText("Confirm removing web")).not.toBeInTheDocument()

    fireEvent.click(screen.getByLabelText("Remove web"))
    expect(screen.getByLabelText("Confirm removing web")).toBeInTheDocument()
    expect(screen.getByLabelText("Keep host")).toBeInTheDocument()
    // The removal is deferred until the confirm click.
    expect(useSshStore.getState().hosts).toHaveLength(1)

    fireEvent.click(screen.getByLabelText("Confirm removing web"))
    expect(useSshStore.getState().hosts).toHaveLength(0)
  })

  it("cancelling keeps the host and restores the delete button", () => {
    useSshStore.getState().addHost(passwordHost)
    render(<SshNavContent />)

    fireEvent.click(screen.getByLabelText("Remove web"))
    fireEvent.click(screen.getByLabelText("Keep host"))

    expect(useSshStore.getState().hosts).toHaveLength(1)
    expect(screen.getByLabelText("Remove web")).toBeInTheDocument()
    expect(screen.queryByLabelText("Confirm removing web")).not.toBeInTheDocument()
  })

  it("opening a confirm on another row resets the previous row's confirm", () => {
    useSshStore.getState().addHost(passwordHost)
    useSshStore.getState().addHost(keyHost)
    render(<SshNavContent />)

    fireEvent.click(screen.getByLabelText("Remove web"))
    expect(screen.getByLabelText("Confirm removing web")).toBeInTheDocument()

    fireEvent.click(screen.getByLabelText("Remove box"))
    expect(screen.getByLabelText("Confirm removing box")).toBeInTheDocument()
    // Only one row may be in the confirm state at a time.
    expect(screen.queryByLabelText("Confirm removing web")).not.toBeInTheDocument()
    expect(screen.getByLabelText("Remove web")).toBeInTheDocument()
  })

  it("opening the edit dialog resets a pending delete confirm", () => {
    useSshStore.getState().addHost(passwordHost)
    useSshStore.getState().addHost(keyHost)
    render(<SshNavContent />)

    fireEvent.click(screen.getByLabelText("Remove web"))
    expect(screen.getByLabelText("Confirm removing web")).toBeInTheDocument()

    fireEvent.click(screen.getByLabelText("Edit box"))
    expect(screen.queryByLabelText("Confirm removing web")).not.toBeInTheDocument()
    expect(screen.getByLabelText("Remove web")).toBeInTheDocument()
    expect(screen.getByText("Edit SSH host")).toBeInTheDocument()
  })
})
