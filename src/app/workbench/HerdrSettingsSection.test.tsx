import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

const ipc = vi.hoisted(() => ({
  get: vi.fn(),
  set: vi.fn(),
  wslDistributions: vi.fn()
}))

vi.mock("@/lib/herdrIpc", () => ({
  herdrBinarySourceGet: ipc.get,
  herdrBinarySourceSet: ipc.set,
  herdrWslDistributions: ipc.wslDistributions
}))

import { HerdrSettingsSection } from "./HerdrSettingsSection"
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore"

const initialHerdrState = useHerdrStore.getState()

beforeEach(() => {
  useHerdrStore.setState({ ...herdrInitialState, selectRuntimeTarget: vi.fn(async () => undefined) })
  ipc.get.mockReset().mockResolvedValue({
    configured: "default",
    active: "global",
    resolved: "global",
    available: true,
    path: "/Users/me/.local/bin/herdr",
    version: "0.8.0",
    protocol: 19,
    configuredAvailable: false,
    configuredPath: null,
    configuredReason: "This build does not include a managed Herdr binary",
    configuredVersion: null,
    configuredProtocol: null,
    configurationError: null,
    restartRequired: true
  })
  ipc.set.mockReset()
  ipc.wslDistributions.mockReset().mockResolvedValue([])
})

afterEach(() => {
  useHerdrStore.setState(initialHerdrState, true)
})

describe("HerdrSettingsSection", () => {
  it("separates active and configured-target diagnostics", async () => {
    render(<HerdrSettingsSection />)

    const global = await screen.findByRole("button", { name: /Global|全域/ })
    const managed = screen.getByRole("button", { name: /Yuzora-managed/i })
    expect(global).toHaveAttribute("aria-pressed", "false")
    expect(managed).toHaveAttribute("aria-pressed", "true")
    expect(screen.getByText("/Users/me/.local/bin/herdr")).toBeInTheDocument()
    expect(screen.getByText("0.8.0")).toBeInTheDocument()
    expect(screen.getByText("19")).toBeInTheDocument()
    expect(
      screen.getAllByText(/does not include a managed Herdr binary|尚未內建 managed Herdr binary/)
    ).not.toHaveLength(0)
    await waitFor(() => expect(ipc.get).toHaveBeenCalledTimes(1))
  })

  it("shows the effective managed source when global falls back", async () => {
    ipc.get.mockResolvedValue({
      configured: "global",
      active: "global",
      resolved: "default",
      available: true,
      path: String.raw`C:\Program Files\Yuzora\herdr\windows-x86_64\herdr.exe`,
      reason: "Herdr was not found on PATH; using Yuzora-managed Herdr",
      version: "0.8.0-preview.2026-08-04-d78e3d3b5126",
      protocol: 19,
      configuredAvailable: true,
      configuredPath: String.raw`C:\Program Files\Yuzora\herdr\windows-x86_64\herdr.exe`,
      configuredReason: "Herdr was not found on PATH; using Yuzora-managed Herdr",
      configuredVersion: "0.8.0-preview.2026-08-04-d78e3d3b5126",
      configuredProtocol: 19,
      configurationError: null,
      restartRequired: false
    })

    render(<HerdrSettingsSection />)

    expect(await screen.findByText("default", { selector: "dd" })).toBeInTheDocument()
    expect(screen.getByRole("button", { name: /Global|全域/ })).toHaveAttribute(
      "aria-pressed",
      "true"
    )
    expect(screen.getAllByText(/using Yuzora-managed Herdr/)).not.toHaveLength(0)
  })

  it("lists distros without probing one, selects a Unicode distro by keyboard, and exposes transport diagnostics", async () => {
    const ubuntu = { kind: "wsl" as const, distro: "Ubuntu 開発" }
    const selectRuntimeTarget = vi.fn(async () => undefined)
    ipc.wslDistributions.mockResolvedValue([{ distro: ubuntu.distro }])
    useHerdrStore.setState({
      selectedRuntimeTarget: ubuntu,
      connectionState: "ready",
      errorMessage: "proxy unavailable",
      capabilities: {
        binarySource: { configured: "global", active: "global", available: true, configuredAvailable: true, restartRequired: false },
        server: { running: true },
        api: { snapshot: true, ping: true, tabCreate: false, workspaceFocus: false, workspaceCreate: false, workspaceRename: false, workspaceClose: false, tabRename: false, tabClose: false, tabFocus: false, paneFocus: false, paneRename: false, paneSplit: false, paneZoom: false, paneSwap: false, paneClose: false, layoutExport: false, layoutSetSplitRatio: false, agentGet: false, agentRead: false, eventsSubscribe: false, worktreeList: false, methods: [] },
        terminal: { observe: true, control: true, takeover: true, input: true, resize: true, scroll: true, release: true, create: false },
        events: { status: "unavailable", reason: "proxy unavailable" },
        transport: { mode: "wsl-cli-fallback", state: "degraded", generation: null, pendingRequests: 0, eventListeners: 0, activeChildren: 0, requests: 1, responses: 0, eventsDelivered: 0, staleEventsDropped: 0, maxRequestMs: 0, maxEventDispatchMs: 0, failure: "proxy unavailable" }
      },
      selectRuntimeTarget
    })

    render(<HerdrSettingsSection />)
    const selected = await screen.findByRole("button", { name: /Selected|已選取/ })
    expect(selected).toHaveAttribute("aria-pressed", "true")
    expect(screen.getByTestId("herdr-wsl-transport-diagnostics")).toHaveTextContent("wsl-cli-fallback")
    fireEvent.keyDown(selected, { key: "Enter" })
    fireEvent.click(selected)
    await waitFor(() => expect(selectRuntimeTarget).toHaveBeenCalledWith(ubuntu))
    // The Settings surface only calls host-level distro listing; selecting is
    // delegated to the store and no per-distro probe happens before the click.
    expect(ipc.wslDistributions).toHaveBeenCalledTimes(1)
  })

  it("reports a missing distro without selecting Native or dropping WSL diagnostics", async () => {
    const ubuntu = { kind: "wsl" as const, distro: "Missing Distro" }
    ipc.wslDistributions.mockRejectedValue(new Error("WSL distribution not found"))
    useHerdrStore.setState({ selectedRuntimeTarget: ubuntu, errorMessage: "WSL distribution not found" })
    render(<HerdrSettingsSection />)
    expect(await screen.findByRole("alert")).toHaveTextContent("WSL distribution not found")
    expect(useHerdrStore.getState().selectedRuntimeTarget).toEqual(ubuntu)
  })

  it("normalizes verbatim Windows diagnostic paths without mutating the DTO", async () => {
    const fixture = {
      configured: "default",
      active: "global",
      resolved: "global",
      available: true,
      path: String.raw`\\?\C:\Users\me\.local\bin\herdr.exe`,
      version: "0.8.0",
      protocol: 19,
      configuredAvailable: false,
      configuredPath: null,
      configuredReason: String.raw`Yuzora-managed Herdr binary is unavailable at \\?\C:\Program Files\Yuzora\herdr\windows-x86_64\herdr.exe`,
      configuredVersion: null,
      configuredProtocol: null,
      configurationError: null,
      restartRequired: true
    }
    ipc.get.mockResolvedValue(fixture)

    render(<HerdrSettingsSection />)

    expect(
      await screen.findByText(String.raw`C:\Users\me\.local\bin\herdr.exe`)
    ).toBeInTheDocument()
    expect(
      screen.getByText(
        String.raw`Yuzora-managed Herdr binary is unavailable at C:\Program Files\Yuzora\herdr\windows-x86_64\herdr.exe`
      )
    ).toBeInTheDocument()
    expect(screen.queryByText(/\\\?\\/)).not.toBeInTheDocument()
    expect(fixture.path).toBe(String.raw`\\?\C:\Users\me\.local\bin\herdr.exe`)
    expect(fixture.configuredReason).toBe(
      String.raw`Yuzora-managed Herdr binary is unavailable at \\?\C:\Program Files\Yuzora\herdr\windows-x86_64\herdr.exe`
    )
  })
})
