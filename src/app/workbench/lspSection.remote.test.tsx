import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { mockIPC, clearMocks } from "@tauri-apps/api/mocks"
import i18n from "@/lib/i18n"
import { LspSection } from "./LspSection"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { useLspStore } from "@/state/lspStore"
import { remoteFilePath, parseRemoteFilePath } from "@/lib/runtimeIdentity"
import { requestWorkspace } from "@/lib/remoteTrust"
import { setRemoteLspTrace } from "@/lib/remoteLsp"
import { installRemoteLsp, cancelRemoteLspInstall } from "@/lib/remoteLspInstall"
import type { LspInstallProgress, LspServerInfo } from "@/lib/types"

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn() }))
vi.mock("@/lib/remoteFiles", () => ({ runtimeWorkspaceService: (uri: string) => ({ uri, root: "/repo", capabilityId: "cap", owner: { hostId: parseRemoteFilePath(uri)!.hostId, generation: 1 }, assertCurrent: () => {} }) }))
vi.mock("@/lib/remoteTrust", () => ({ requestWorkspace: vi.fn() }))
vi.mock("@/lib/remoteLsp", () => ({ remoteLspTraceEnabled: () => false, setRemoteLspTrace: vi.fn(async () => {}), restartConfiguredRemoteLsp: vi.fn(async () => {}) }))
vi.mock("@/lib/remoteLspInstall", () => ({ installRemoteLsp: vi.fn(), cancelRemoteLspInstall: vi.fn() }))

const workspaceA = remoteFilePath("host-a", "/repo")
const workspaceB = remoteFilePath("host-b", "/repo")
let nativeCalls: string[]
const config = { defaults: { python: "pyright" }, workspaces: {} }
beforeEach(async () => {
  vi.clearAllMocks()
  nativeCalls = []
  mockIPC((command) => { nativeCalls.push(command) })
  await i18n.changeLanguage("en")
  useWorkspaceStore.setState({ workspacePath: workspaceA })
  useLspStore.getState().reset()
  vi.mocked(requestWorkspace).mockImplementation(async (_service, operation) => {
    if (operation.method !== "lspConfig") throw new Error("unexpected operation")
    const call = operation.params.call
    if (call.action === "stale") return [] as never
    if (call.action === "detect") return { workspace: call.global ? "" : "/repo", language: call.language, serverId: "pyright", status: { status: "missing", installHint: "install on host" } } as never
    return config as never
  })
})
afterEach(() => { cleanup(); clearMocks() })

it("routes defaults, profiles, detection and trace to the selected host", async () => {
  render(<LspSection />)
  await waitFor(() => expect(requestWorkspace).toHaveBeenCalledTimes(6))
  fireEvent.click(screen.getByRole("radio", { name: "This host default" }))
  const python = within(screen.getByTestId("lsp-card-python"))
  await waitFor(() => expect(requestWorkspace).toHaveBeenCalledWith(expect.objectContaining({ uri: workspaceA }), expect.objectContaining({ params: { workspace: "cap", call: { action: "detect", language: "python", global: true } } })))
  fireEvent.click(python.getByRole("radio", { name: "pylsp" }))
  await waitFor(() => expect(requestWorkspace).toHaveBeenCalledWith(expect.objectContaining({ uri: workspaceA }), { method: "lspConfig", params: { workspace: "cap", call: { action: "set", language: "python", serverId: "pylsp", global: true } } }))
  fireEvent.click(screen.getByRole("switch", { name: /JSON-RPC/ }))
  await waitFor(() => expect(setRemoteLspTrace).toHaveBeenCalledWith(workspaceA, true))
  expect(nativeCalls.filter((command) => command.startsWith("lsp_"))).toEqual([])
})

it("keeps install progress and cancellation on the initiating host when switching folders", async () => {
  let progress!: (event: LspInstallProgress) => void
  let finish!: (info: LspServerInfo) => void
  vi.mocked(installRemoteLsp).mockImplementation((_context, _workspace, _language, onProgress) => {
    progress = onProgress!
    return new Promise((resolve) => { finish = resolve })
  })
  render(<LspSection />)
  await waitFor(() => expect(requestWorkspace).toHaveBeenCalledTimes(6))
  fireEvent.click(within(screen.getByTestId("lsp-card-python")).getByRole("button", { name: "Install" }))
  await waitFor(() => expect(installRemoteLsp).toHaveBeenCalledWith(workspaceA, workspaceA, "python", expect.any(Function)))
  act(() => progress({ language: "python", phase: "download", percent: 42, message: "host-a-progress" }))
  fireEvent.click(screen.getByRole("button", { name: i18n.t("settings.lsp.cancelInstall", { ns: "workbench" }) }))
  await waitFor(() => expect(cancelRemoteLspInstall).toHaveBeenCalledWith(workspaceA, "python"))
  act(() => useWorkspaceStore.setState({ workspacePath: workspaceB }))
  await waitFor(() => expect(requestWorkspace).toHaveBeenCalledWith(expect.objectContaining({ uri: workspaceB }), expect.anything()))
  act(() => progress({ language: "python", phase: "download", percent: 60, message: "old-host-progress" }))
  expect(screen.queryByText(/old-host-progress|host-a-progress/)).not.toBeInTheDocument()
  await act(async () => finish({ workspace: workspaceA, language: "python", serverId: "pyright", path: "/host-a-only/bin/pyright", command: "pyright", status: { status: "stopped" }, lastStartupLog: null, lastError: null, restartCount: 0 }))
  expect(screen.queryByText("/host-a-only/bin/pyright")).not.toBeInTheDocument()
  expect(useLspStore.getState().servers.python?.workspace).not.toBe(workspaceA)
})
