import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import type { ConnectedHost, WslDistribution } from "@/lib/hostIpc"
import type { SftpListing } from "@/lib/types"
import type { SshHost } from "@/state/sshStore"

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }))
vi.mock("@/lib/platform", () => ({ isWindowsPlatform: vi.fn(() => false) }))
vi.mock("@/lib/ipc", () => ({ sshConnect: vi.fn(), sshDisconnect: vi.fn(async () => {}), sftpListDir: vi.fn() }))
vi.mock("@/lib/hostIpc", () => ({ requestHost: vi.fn(), wslDistributions: vi.fn(), wslPath: vi.fn() }))
vi.mock("@/lib/remoteFiles", () => ({ registerRuntimeWorkspace: vi.fn(), registerSftpWorkspace: vi.fn() }))
vi.mock("@/app/panels/SftpPanel", () => ({ SftpPanel: () => null }))

import { FolderPickerHost } from "./FolderPickerHost"
import { SshAuthenticationHost } from "./SftpHost"
import { useFolderPickerStore } from "@/state/folderPickerStore"
import { useHostStore } from "@/state/hostStore"
import { useSshStore } from "@/state/sshStore"
import { useRecentWorkspacesStore } from "@/state/recentWorkspaces"
import { rememberRemoteWorkspace } from "@/state/remoteWorkspaceRegistry"
import { remoteFilePath } from "@/lib/runtimeIdentity"
import { isWindowsPlatform } from "@/lib/platform"
import { requestHost, wslDistributions } from "@/lib/hostIpc"
import { sftpListDir, sshConnect } from "@/lib/ipc"
import { registerRuntimeWorkspace, registerSftpWorkspace } from "@/lib/remoteFiles"

const root = "/home/test/中文 project"
const server: SshHost = { id: "ssh-a", name: "Server A", host: "a.example", port: 22, user: "test", authKind: "key", keyPath: "/key" }
const finish = vi.fn()
function connection(hostId: string): ConnectedHost {
  return { owner: { hostId, generation: 1 }, hello: { protocol: 1, version: "test", os: "linux", arch: "x86_64", home: "/home/test", methods: [] } }
}
function connected(host: SshHost, sessionId = "transport-a") {
  useSshStore.setState((state) => ({ sessions: { ...state.sessions, [host.id]: { hostId: host.id, sessionId, status: "connected", fingerprint: "SHA256:test", knownHost: true, error: null } } }))
}
function runtimeConnected(hostId: string) {
  const conn = connection(hostId)
  useHostStore.setState((state) => ({ hosts: { ...state.hosts, [hostId]: { connection: conn, connecting: false, error: null, target: { kind: "ssh", sessionId: "transport-a" }, attempt: 0, retryAt: 0 } } }))
  return conn
}
function recent(hostId: string, access: "runtime" | "sftp" = "runtime", path = root) {
  const uri = remoteFilePath(hostId, path)
  rememberRemoteWorkspace(uri, access)
  useRecentWorkspacesStore.setState({ list: [uri] })
  return uri
}
function mount() { return render(<><FolderPickerHost /><SshAuthenticationHost /></>) }
function selectRecent() { fireEvent.click(screen.getByRole("button", { name: new RegExp(root) })) }
function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => { resolve = done })
  return { promise, resolve }
}

beforeEach(() => {
  vi.clearAllMocks()
  const values = new Map<string, string>()
  vi.stubGlobal("localStorage", { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => { values.set(key, value) } })
  vi.mocked(isWindowsPlatform).mockReturnValue(false)
  vi.mocked(wslDistributions).mockResolvedValue([])
  vi.mocked(sshConnect).mockResolvedValue({ sessionId: "transport-a", fingerprint: "SHA256:test", knownHost: true })
  useSshStore.setState({ hosts: [server], sessions: {}, activeHostId: null, pendingAuthHostId: null })
  useHostStore.setState({ hosts: {}, configs: {} })
  useRecentWorkspacesStore.setState({ list: [] })
  useFolderPickerStore.setState({ open: true, finish, initialLocation: "local", runtimeHostId: undefined, legacyWindowsPath: undefined })
})
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals() })

it("routes an offline SSH runtime recent folder through its key login and preserves its root", async () => {
  const uri = recent(server.id)
  mount(); selectRecent()
  await screen.findByRole("button", { name: "Set up this host" })
  expect(finish).not.toHaveBeenCalled()
  expect(sshConnect).toHaveBeenCalledWith("a.example", 22, "test", { kind: "key", keyPath: "/key", passphrase: undefined })
  expect(screen.getByRole("tab", { name: "Runtime workspace" })).toHaveAttribute("data-state", "active")
  act(() => { runtimeConnected(server.id) })
  expect(screen.getByLabelText("Remote folder")).toHaveValue(root)
  vi.mocked(requestHost).mockImplementation(async (_owner, request) => {
    if (request.method === "workspaceOpen") return { capabilityId: "cap-a", canonicalPath: root }
    return []
  })
  vi.mocked(registerRuntimeWorkspace).mockResolvedValue(uri)
  fireEvent.click(screen.getByRole("button", { name: "Browse" }))
  await waitFor(() => expect(screen.getByRole("button", { name: "Open folder" })).toBeEnabled())
  fireEvent.click(screen.getByRole("button", { name: "Open folder" }))
  await waitFor(() => expect(finish).toHaveBeenCalledWith(uri))
  expect(requestHost).toHaveBeenCalledWith(connection(server.id).owner, { method: "workspaceOpen", params: { path: root } })
})

it("uses the shared password prompt and keeps SFTP access without setting up a runtime", async () => {
  const host = { ...server, authKind: "password" as const }
  useSshStore.setState({ hosts: [host] })
  const uri = recent(host.id, "sftp")
  const setup = vi.spyOn(useHostStore.getState(), "setup")
  mount(); selectRecent()
  const prompt = await screen.findByTestId("ssh-password-dialog")
  expect(prompt).not.toHaveAttribute("aria-hidden", "true")
  expect(sshConnect).not.toHaveBeenCalled()
  fireEvent.change(within(prompt).getByLabelText("SSH password"), { target: { value: "fixture-only" } })
  fireEvent.click(within(prompt).getByRole("button", { name: "Connect" }))
  await waitFor(() => expect(screen.getByLabelText("Remote folder")).toHaveValue(root))
  expect(sshConnect).toHaveBeenCalledWith("a.example", 22, "test", { kind: "password", password: "fixture-only" })
  vi.mocked(sftpListDir).mockResolvedValue({ cwd: root, entries: [] })
  vi.mocked(registerSftpWorkspace).mockResolvedValue(uri)
  fireEvent.click(screen.getByRole("button", { name: "Browse" }))
  await waitFor(() => expect(screen.getByRole("button", { name: "Open folder" })).toBeEnabled())
  fireEvent.click(screen.getByRole("button", { name: "Open folder" }))
  await waitFor(() => expect(finish).toHaveBeenCalledWith(uri))
  expect(registerSftpWorkspace).toHaveBeenCalledWith(host.id, root)
  expect(setup).not.toHaveBeenCalled()
})

it("cancelling authentication keeps the recent folder picker open without connecting", async () => {
  useSshStore.setState({ hosts: [{ ...server, authKind: "password" }] })
  recent(server.id, "sftp")
  mount(); selectRecent()
  fireEvent.click(within(await screen.findByTestId("ssh-password-dialog")).getByRole("button", { name: "Cancel" }))
  expect(screen.getByRole("dialog", { name: "Add folder" })).toBeVisible()
  expect(finish).not.toHaveBeenCalled()
  expect(sshConnect).not.toHaveBeenCalled()
})

it.each(["runtime", "sftp"] as const)("opens a connected %s recent folder directly", (access) => {
  connected(server)
  runtimeConnected(server.id)
  const uri = recent(server.id, access)
  mount(); selectRecent()
  expect(finish).toHaveBeenCalledWith(uri)
  expect(sshConnect).not.toHaveBeenCalled()
})

it("does not reconnect a missing host using another host with the same name and path", () => {
  recent(server.id)
  useSshStore.setState({ hosts: [{ ...server, id: "ssh-replacement", user: "other-account" }] })
  mount(); selectRecent()
  expect(screen.getByRole("alert")).toHaveTextContent("host is unavailable")
  expect(finish).not.toHaveBeenCalled()
  expect(sshConnect).not.toHaveBeenCalled()
})

it("does not reuse an earlier host's delayed SFTP listing after switching hosts", async () => {
  const other = { ...server, id: "ssh-b", name: "Server B", host: "b.example" }
  useSshStore.setState({ hosts: [server, other], activeHostId: server.id })
  connected(server); connected(other, "transport-b")
  useFolderPickerStore.setState({ initialLocation: "remote" })
  const listing = deferred<SftpListing>()
  vi.mocked(sftpListDir).mockReturnValue(listing.promise)
  mount()
  fireEvent.click(screen.getByRole("button", { name: "Browse" }))
  act(() => { useSshStore.getState().setActiveHost(other.id) })
  await act(async () => listing.resolve({ cwd: "/only-on-a", entries: [] }))
  expect(screen.getByLabelText("Remote folder")).toHaveValue(".")
  expect(screen.getByRole("button", { name: "Open folder" })).toBeDisabled()
  expect(screen.getByRole("button", { name: "Browse" })).toBeEnabled()
})

function wslConfig() {
  useHostStore.setState({ configs: { "wsl-a": { hostId: "wsl-a", label: "Ubuntu", kind: "wsl", distro: "Ubuntu", helper: "/helper", binary: "/herdr" } } })
  recent("wsl-a")
}
const distributions: WslDistribution[] = [
  { hostId: "wsl-b", name: "Debian", version: 2 },
  { hostId: "wsl-a", name: "Ubuntu", version: 2 },
]

it("selects the discovered WSL identity and original root instead of the default distribution", async () => {
  vi.mocked(isWindowsPlatform).mockReturnValue(true)
  vi.mocked(wslDistributions).mockResolvedValue(distributions)
  wslConfig()
  const conn = connection("wsl-a")
  const setup = vi.spyOn(useHostStore.getState(), "setup").mockImplementation(async () => {
    runtimeConnected("wsl-a")
    return conn
  })
  mount(); selectRecent()
  fireEvent.click(await screen.findByRole("button", { name: "Set up this host" }))
  await waitFor(() => expect(screen.getByLabelText("Remote folder")).toHaveValue(root))
  expect(setup).toHaveBeenCalledWith("wsl-a", "Ubuntu", { kind: "wsl", distro: "Ubuntu" }, false)
  expect(finish).not.toHaveBeenCalled()
})

it.each([1, 2])("rejects a WSL recent identity absent from WSL2 discovery (version %s)", async (version) => {
  vi.mocked(isWindowsPlatform).mockReturnValue(true)
  vi.mocked(wslDistributions).mockResolvedValue([{ ...distributions[1], version, hostId: version === 1 ? "wsl-a" : "replacement-id" }])
  wslConfig(); mount(); selectRecent()
  expect(await screen.findByRole("alert")).toHaveTextContent("host is unavailable")
  expect(screen.queryByRole("button", { name: "Set up this host" })).not.toBeInTheDocument()
  expect(finish).not.toHaveBeenCalled()
})

it("keeps an unavailable WSL recent folder open on macOS or Linux", () => {
  wslConfig(); mount(); selectRecent()
  expect(screen.getByRole("alert")).toHaveTextContent("host is unavailable")
  expect(finish).not.toHaveBeenCalled()
  expect(sshConnect).not.toHaveBeenCalled()
})
