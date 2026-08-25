import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

vi.mock("@/lib/herdrIpc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/herdrIpc")>()),
  herdrAgentCatalog: vi.fn()
}))

import { herdrAgentCatalog } from "@/lib/herdrIpc"
import { HerdrNewAgentDialog } from "./HerdrNewAgentDialog"
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore"
import { uiInitialState, useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

const createAgent = vi.fn()

beforeEach(() => {
  useUiStore.setState(uiInitialState)
  useWorkspaceStore.setState({
    workspacePath: "/workspace",
    groups: [{ tabs: [], activePath: null }],
    activeGroupIndex: 0
  })
  useHerdrStore.setState({
    ...herdrInitialState,
    selectedSessionName: "default",
    selectedSpaceId: "w1",
    snapshot: {
      herdrSessionId: "default",
      protocol: 19,
      version: "0.8.0",
      spaces: [{ id: "w1", label: "Yuzora", order: 1, focused: true }],
      agents: [],
      tabs: [],
      terminals: [],
      raw: {}
    },
    spaces: () => [{ id: "w1", label: "Yuzora", order: 1, focused: true }],
    createAgentBlockedReason: () => null,
    createAgentInSelectedSpace: createAgent
  })
  vi.mocked(herdrAgentCatalog).mockResolvedValue([
    {
      agent: "codex",
      source: "bundled",
      sourceKind: "bundled",
      activeVersion: "2026.07.18.1",
      detectedBinaryPath: String.raw`C:\Users\Yuuzu\bin\codex.cmd`,
      bypassFlags: ["--dangerously-bypass-approvals-and-sandbox"]
    },
    {
      agent: "pi",
      source: "bundled",
      sourceKind: "bundled",
      detectedBinaryPath: null,
      bypassFlags: []
    }
  ])
  createAgent.mockResolvedValue({
    herdrSessionId: "default",
    workspaceId: "w1",
    terminalId: "term-2",
    paneId: "pane-2",
    tabId: "tab-2",
    title: "codex",
    name: "codex",
    kind: "codex"
  })
})

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
  useHerdrStore.setState({ ...herdrInitialState, attachments: new Map() })
})

it("loads the manifest catalog, keeps bypass opt-in off by default, and opens the created Agent", async () => {
  const onOpenChange = vi.fn()
  render(<HerdrNewAgentDialog open onOpenChange={onOpenChange} />)

  expect(await screen.findByText("codex")).toBeInTheDocument()
  expect(screen.getByText("C:\\Users\\Yuuzu\\bin\\codex.cmd")).toBeInTheDocument()
  const bypass = screen.getByRole("switch", { name: "Bypass permissions" })
  expect(bypass).not.toBeChecked()

  fireEvent.click(bypass)
  fireEvent.click(screen.getByRole("button", { name: "Start Agent" }))

  await waitFor(() => {
    expect(createAgent).toHaveBeenCalledWith("codex", true)
  })
  expect(useUiStore.getState().mode).toBe("ade")
  expect(
    useWorkspaceStore
      .getState()
      .groups.flatMap((group) => group.tabs)
      .some((tab) => tab.kind === "herdr-terminal" && tab.herdrTabId === "tab-2")
  ).toBe(true)
  expect(onOpenChange).toHaveBeenCalledWith(false)
})

it("keeps server-advertised kinds selectable when Yuzora PATH detection misses", async () => {
  render(<HerdrNewAgentDialog open onOpenChange={() => {}} />)
  await screen.findByText("pi")

  fireEvent.click(screen.getByText("pi"))
  expect(screen.getByText("Not detected on Yuzora PATH · Herdr validates on start")).toBeInTheDocument()
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: "Start Agent" }))
  })

  await waitFor(() => {
    expect(createAgent).toHaveBeenCalledWith("pi", false)
  })
})
