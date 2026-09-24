import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { herdrFeature, type HerdrPlugin } from "@/lib/herdrFeatures"
import type { HerdrCapabilities, HerdrSnapshot } from "@/lib/herdrTypes"
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore"
import { useHerdrNativeStore } from "@/state/herdrNativeStore"
import HerdrToolsDialog from "./HerdrToolsDialog"

vi.mock("@/lib/herdrFeatures", () => ({ herdrFeature: vi.fn() }))

const capabilities = {
  server: { running: true, compatible: true },
  api: { snapshot: true, methods: ["plugin.list", "plugin.pane.open", "plugin.action.invoke"] }
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
  it("targets a pane inside the newly selected Space instead of the launcher Space", async () => {
    render(<HerdrToolsDialog selection={{ tool: "plugins", sessionName: "work", paneId: "p1" }} />)
    fireEvent.pointerDown(screen.getByRole("button", { name: "Space" }), { button: 0, ctrlKey: false })
    fireEvent.click(await screen.findByRole("menuitemradio", { name: "Beta" }))

    expect(await openSplitPane()).toMatchObject({
      sessionName: "work", paneId: "p2",
      request: { method: "plugin.pane.open", params: { placement: "split", target_pane_id: "p2" } }
    })
  })

  it("defaults to the Space that owns the launcher pane", async () => {
    render(<HerdrToolsDialog selection={{ tool: "plugins", sessionName: "work", paneId: "p3" }} />)

    expect(screen.getByRole("button", { name: "Space" })).toHaveTextContent("Beta")
    expect(await openSplitPane()).toMatchObject({ paneId: "p3", request: { params: { target_pane_id: "p3" } } })
  })
})
