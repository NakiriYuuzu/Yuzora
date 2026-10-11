import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { herdrFeature, type HerdrPlugin } from "@/lib/herdrFeatures"
import type { HerdrCapabilities, HerdrSnapshot } from "@/lib/herdrTypes"
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore"
import { useHerdrNativeStore } from "@/state/herdrNativeStore"
import { useUiStore } from "@/state/uiStore"
import { useHerdrToolsStore } from "@/state/herdrToolsStore"
import HerdrToolsDialog from "./HerdrToolsDialog"
import i18n from "@/lib/i18n"

vi.mock("@/lib/herdrFeatures", () => ({ herdrFeature: vi.fn() }))

const capabilities = {
  server: { running: true, compatible: true },
  api: { snapshot: true, methods: ["plugin.list", "plugin.pane.open", "plugin.action.invoke", "agent.start", "agent.prompt", "pane.move", "worktree.create", "worktree.list"] }
} as unknown as HerdrCapabilities

const snapshot = {
  herdrSessionId: "work", protocol: 1, version: "test", agents: [], tabs: [],
  focusedWorkspaceId: "w1", focusedPaneId: "p1",
  spaces: [{ id: "w1", label: "Alpha", order: 0, focused: true }, { id: "w2", label: "Beta", order: 1, focused: false }],
  terminals: [
    { terminalId: "t-1", paneId: "p1", workspaceId: "w1", tabId: "tab-1" },
    { terminalId: "t-2", paneId: "p2", workspaceId: "w2", tabId: "tab-2" },
    { terminalId: "t-3", paneId: "p3", workspaceId: "w2", tabId: "tab-2" }
  ]
} as unknown as HerdrSnapshot

beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(herdrFeature).mockResolvedValue({ plugins: [{
    plugin_id: "fixture.demo", name: "Fixture plugin", version: "1.0.0", enabled: true,
    panes: [{ id: "inspect", title: "Split plugin pane", placement: "split" }]
  } satisfies HerdrPlugin] })
  useHerdrNativeStore.setState({ selection: null })
  useHerdrStore.setState({
    ...herdrInitialState,
    sessions: [{ name: "work", default: true, running: true, sessionDir: "/tmp/work", socketPath: "/tmp/work.sock" }],
    runtimesBySession: { work: { connectionState: "ready", capabilities, snapshot, errorMessage: null } } as never
  })
})

afterEach(() => {
  cleanup()
  useHerdrNativeStore.setState({ selection: null })
  useHerdrStore.setState({ ...herdrInitialState })
})

async function openSplitPane() {
  fireEvent.click(await screen.findByRole("button", { name: "Split plugin pane" }))
  return useHerdrNativeStore.getState().selection
}

describe("HERDR tools pane target", () => {
  it.each(["ready", "failed"] as const)("shows a connecting Session suffix until startup is %s without enabling tools", async (state) => {
    useHerdrStore.setState({
      herdrStartup: { state: "starting", error: null },
      sessions: [{ name: "default", default: true, running: false, sessionDir: "/", socketPath: "/sock" }],
      runtimesBySession: {},
    })
    render(<HerdrToolsDialog selection={{ sessionName: "default" }} />)
    expect(screen.getByRole("button", { name: new RegExp(i18n.t("herdrTools:tasks.native.title")) })).toBeDisabled()
    fireEvent.pointerDown(screen.getByRole("button", { name: i18n.t("herdrTools:session") }), { button: 0, ctrlKey: false })
    const option = await screen.findByRole("menuitemradio", { name: /Connecting to Herdr/ })
    expect(option).not.toHaveTextContent(i18n.t("herdrTools:stopped"))
    expect(herdrFeature).not.toHaveBeenCalled()
    expect(useHerdrNativeStore.getState().selection).toBeNull()
    act(() => useHerdrStore.setState({ herdrStartup: { state, error: state === "failed" ? "startup failed" : null } }))
    expect(screen.getByRole("menuitemradio", { name: /Stopped/ })).toBeInTheDocument()
    expect(herdrFeature).not.toHaveBeenCalled()
  })

  it("targets a pane inside the newly selected Space instead of the launcher Space", async () => {
    render(<HerdrToolsDialog selection={{ task: "plugins", sessionName: "work", paneId: "p1" }} />)
    fireEvent.pointerDown(screen.getByRole("button", { name: "Space" }), { button: 0, ctrlKey: false })
    fireEvent.click(await screen.findByRole("menuitemradio", { name: "Beta" }))

    expect(await openSplitPane()).toMatchObject({
      sessionName: "work", paneId: "p2",
      request: { method: "plugin.pane.open", params: { placement: "split", target_pane_id: "p2" } }
    })
  })

  it("defaults to the Space that owns the launcher pane", async () => {
    render(<HerdrToolsDialog selection={{ task: "plugins", sessionName: "work", paneId: "p3" }} />)

    expect(screen.getByRole("button", { name: "Space" })).toHaveTextContent("Beta")
    expect(await openSplitPane()).toMatchObject({ paneId: "p3", request: { params: { target_pane_id: "p3" } } })
  })
})

const card = (key: string) => screen.getByRole("button", { name: new RegExp(i18n.t(`herdrTools:tasks.${key}.title`).replace(/[()]/g, "\\$&")) })
const setRuntime = (patch: Record<string, unknown>) => useHerdrStore.setState({ runtimesBySession: { work: { connectionState: "ready", capabilities, snapshot, errorMessage: null, ...patch } } as never })

describe("HERDR actions home", () => {
  it("lists task cards, enabling the ones the Session supports", () => {
    render(<HerdrToolsDialog selection={{ sessionName: "work" }} />)
    expect(screen.getByRole("heading", { name: i18n.t("herdrTools:homeQuestion") })).toBeInTheDocument()
    for (const key of ["worktree", "startAgent", "movePane", "sessions", "native"]) expect(card(key)).toBeEnabled()
    // No agents in the fixture snapshot, so messaging has nobody to talk to.
    expect(card("messageAgent")).toBeDisabled()
    expect(screen.getByText(i18n.t("herdrTools:reason.noAgent"))).toBeInTheDocument()
    expect(screen.getByRole("button", { name: i18n.t("herdrTools:notificationSettings") })).toBeInTheDocument()
  })

  it.each([
    ["Session not connected", { connectionState: "error", capabilities: null, snapshot: null }, "not connected"],
    ["server not running", { capabilities: { ...capabilities, server: { running: false, compatible: true } } }, "server is not running"],
    ["unsupported method with version", { capabilities: { ...capabilities, binaryVersion: "0.8.0", api: { snapshot: true, methods: [] } } }, "(current 0.8.0)"],
    ["no panes", { snapshot: { ...snapshot, terminals: [] } }, "no Pane"],
  ])("explains why tasks are unavailable: %s", (_name, patch, text) => {
    setRuntime(patch)
    render(<HerdrToolsDialog selection={{ sessionName: "work" }} />)
    expect(card("startAgent")).toBeDisabled()
    expect(screen.getAllByText(new RegExp(text.replace(/[()]/g, "\\$&"))).length).toBeGreaterThan(0)
    // Session management stays reachable so a stopped Session can be fixed.
    expect(card("sessions")).toBeEnabled()
  })

  it("disables the Advanced Integrations and Plugins buttons with the reason when unavailable", () => {
    setRuntime({ capabilities: { ...capabilities, api: { snapshot: true, methods: [] } } })
    render(<HerdrToolsDialog selection={{ sessionName: "work" }} />)
    for (const key of ["integrations", "plugins"]) {
      const button = screen.getByRole("button", { name: i18n.t(`herdrTools:tasks.${key}.title`) })
      expect(button).toBeDisabled()
      expect(button.getAttribute("title")).toBeTruthy()
    }
  })

  it("focuses the first available card, opens a task, and returns with the back button", async () => {
    render(<HerdrToolsDialog selection={{ sessionName: "work" }} />)
    expect(card("worktree")).toHaveFocus()
    fireEvent.click(card("movePane"))
    expect(screen.getByRole("heading", { name: i18n.t("herdrTools:tasks.movePane.title") })).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: i18n.t("herdrTools:backToActions") }))
    expect(screen.getByRole("heading", { name: i18n.t("herdrTools:homeQuestion") })).toBeInTheDocument()
  })

  it("steps back on Escape only after navigating from home", () => {
    const direct = render(<HerdrToolsDialog selection={{ task: "movePane", sessionName: "work" }} />)
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" })
    expect(screen.getByRole("heading", { name: i18n.t("herdrTools:tasks.movePane.title") })).toBeInTheDocument()
    direct.unmount()
    render(<HerdrToolsDialog selection={{ sessionName: "work" }} />)
    fireEvent.click(card("movePane"))
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" })
    expect(screen.getByRole("heading", { name: i18n.t("herdrTools:homeQuestion") })).toBeInTheDocument()
  })

  it("opens the full Session view from its card and notification settings in Settings", () => {
    useHerdrToolsStore.getState().open({ sessionName: "work" })
    render(<HerdrToolsDialog selection={{ sessionName: "work", paneId: "p2" }} />)
    fireEvent.click(card("native"))
    expect(useHerdrNativeStore.getState().selection).toMatchObject({ sessionName: "work", paneId: "p2" })
    fireEvent.click(screen.getByRole("button", { name: i18n.t("herdrTools:notificationSettings") }))
    expect(useUiStore.getState().settingsSection).toBe("herdr")
    expect(useHerdrToolsStore.getState().selection).toBeNull()
  })

  it("shows a visible no-Session state instead of an implicit default", () => {
    useHerdrStore.setState({ sessions: [], runtimesBySession: {} })
    render(<HerdrToolsDialog selection={{ sessionName: "default" }} />)
    expect(screen.getAllByText(i18n.t("herdrTools:sessionNone")).length).toBeGreaterThan(0)
    expect(card("startAgent")).toBeDisabled()
  })
})

describe("HERDR agent tasks", () => {
  const withAgents = { ...snapshot, agents: [{ id: "a1", name: "claude", status: "idle", workspaceId: "w1", paneId: "p1", displayAgent: "claude", title: "Reviewer" }] } as unknown as HerdrSnapshot

  it("auto-generates a unique, valid agent name and starts with it", async () => {
    setRuntime({ snapshot: { ...withAgents, agents: [{ ...withAgents.agents[0], name: "codex" }] } })
    vi.mocked(herdrFeature).mockResolvedValue({})
    render(<HerdrToolsDialog selection={{ task: "startAgent", sessionName: "work", paneId: "p2" }} />)
    expect(screen.getByText(i18n.t("herdrTools:selectedPane"))).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: i18n.t("herdrTools:start") }))
    await waitFor(() => expect(herdrFeature).toHaveBeenCalledWith("work", { method: "agent.start", params: expect.objectContaining({ pane_id: "p2", kind: "codex", name: "codex-2" }) }))
  })

  it("shows inline validation for a custom agent name and blocks starting", () => {
    render(<HerdrToolsDialog selection={{ task: "startAgent", sessionName: "work", paneId: "p1" }} />)
    fireEvent.click(screen.getByRole("button", { name: i18n.t("herdrTools:advanced") }))
    fireEvent.change(screen.getByLabelText(i18n.t("herdrTools:agentName")), { target: { value: "Bad Name" } })
    expect(screen.getAllByText(i18n.t("herdrTools:agentNameInvalid")).length).toBeGreaterThan(0)
    expect(screen.getByRole("button", { name: i18n.t("herdrTools:start") })).toBeDisabled()
  })

  it("offers an empty state that switches to starting an agent when none is running", () => {
    render(<HerdrToolsDialog selection={{ task: "messageAgent", sessionName: "work" }} />)
    expect(screen.getByText(i18n.t("herdrTools:noAgentsTitle"))).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: i18n.t("herdrTools:tasks.startAgent.title") }))
    expect(screen.getByRole("heading", { name: i18n.t("herdrTools:tasks.startAgent.title") })).toBeInTheDocument()
  })

  it("lists running agents as message targets and sends the prompt to the chosen pane", async () => {
    setRuntime({ snapshot: withAgents })
    vi.mocked(herdrFeature).mockResolvedValue({})
    render(<HerdrToolsDialog selection={{ task: "messageAgent", sessionName: "work", paneId: "p1" }} />)
    expect(screen.getByText("Reviewer")).toBeInTheDocument()
    const send = screen.getByRole("button", { name: i18n.t("herdrTools:send") })
    expect(send).toBeDisabled()
    fireEvent.change(screen.getByLabelText(i18n.t("herdrTools:prompt")), { target: { value: "ship it" } })
    fireEvent.click(send)
    await waitFor(() => expect(herdrFeature).toHaveBeenCalledWith("work", { method: "agent.prompt", params: { target: "p1", text: "ship it" } }))
  })

  it("keeps diagnostic JSON inside a collapsible details block", async () => {
    setRuntime({ snapshot: withAgents, capabilities: { ...capabilities, api: { snapshot: true, methods: ["agent.explain"] } } })
    vi.mocked(herdrFeature).mockResolvedValue({ type: "agent_explain", detail: "because" })
    render(<HerdrToolsDialog selection={{ task: "messageAgent", sessionName: "work", paneId: "p1" }} />)
    fireEvent.click(screen.getByRole("button", { name: i18n.t("herdrTools:advanced") }))
    fireEvent.click(screen.getByRole("button", { name: i18n.t("herdrTools:explain") }))
    fireEvent.click(await screen.findByRole("button", { name: i18n.t("herdrTools:showDetails") }))
    expect(await screen.findByText(/"detail": "because"/)).toBeInTheDocument()
  })
})

describe("HERDR worktree and session tasks", () => {
  it("creates a worktree for the Space picked inside the form, keeping base and path under Advanced", async () => {
    vi.mocked(herdrFeature).mockResolvedValue({})
    render(<HerdrToolsDialog selection={{ task: "worktree", sessionName: "work", workspaceId: "w2" }} />)
    expect(screen.getByRole("combobox", { name: i18n.t("herdrTools:worktreeSpace") })).toHaveTextContent(/Beta/)
    expect(screen.queryByLabelText(i18n.t("herdrTools:base"))).not.toBeInTheDocument()
    fireEvent.change(screen.getByLabelText(i18n.t("herdrTools:branch")), { target: { value: "feature/x" } })
    fireEvent.click(screen.getByRole("button", { name: i18n.t("herdrTools:createWorktree") }))
    await waitFor(() => expect(herdrFeature).toHaveBeenCalledWith("work", { method: "worktree.create", params: { workspace_id: "w2", branch: "feature/x", base: undefined, path: undefined, focus: false } }))
  })

  it("keeps typed branch, base and path when the Space changes inside the form", async () => {
    render(<HerdrToolsDialog selection={{ task: "worktree", sessionName: "work", workspaceId: "w1" }} />)
    fireEvent.change(screen.getByLabelText(i18n.t("herdrTools:branch")), { target: { value: "feature/keep" } })
    fireEvent.click(screen.getByText(i18n.t("herdrTools:advanced")))
    fireEvent.change(screen.getByLabelText(i18n.t("herdrTools:base")), { target: { value: "main" } })
    fireEvent.change(screen.getByLabelText(i18n.t("herdrTools:pathOptional")), { target: { value: "/tmp/wt" } })
    const space = screen.getByRole("combobox", { name: i18n.t("herdrTools:worktreeSpace") })
    fireEvent.keyDown(space, { key: "ArrowDown" })
    fireEvent.click(await screen.findByRole("option", { name: /Beta/ }))
    await waitFor(() => expect(screen.getByRole("combobox", { name: i18n.t("herdrTools:worktreeSpace") })).toHaveTextContent(/Beta/))
    expect(screen.getByLabelText(i18n.t("herdrTools:branch"))).toHaveValue("feature/keep")
    expect(screen.getByLabelText(i18n.t("herdrTools:base"))).toHaveValue("main")
    expect(screen.getByLabelText(i18n.t("herdrTools:pathOptional"))).toHaveValue("/tmp/wt")
  })

  it("validates a new Session name inline", () => {
    render(<HerdrToolsDialog selection={{ task: "sessions", sessionName: "work" }} />)
    fireEvent.change(screen.getByLabelText(i18n.t("herdrTools:name")), { target: { value: "bad name!" } })
    expect(screen.getByRole("alert")).toHaveTextContent(i18n.t("herdrTools:sessionNameInvalid"))
    expect(screen.getByRole("button", { name: i18n.t("herdrTools:createAndStart") })).toBeDisabled()
    expect(screen.getByRole("button", { name: i18n.t("herdrTools:switchToSession") })).toBeInTheDocument()
    expect(screen.getByRole("button", { name: i18n.t("herdrTools:stopKeepLayout") })).toBeInTheDocument()
  })
})
