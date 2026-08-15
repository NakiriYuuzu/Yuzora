import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type {
  HerdrAgentDetails,
  HerdrAgentInfo,
  HerdrAgentReadResult,
  HerdrCapabilities
} from "@/lib/herdrTypes"
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore"

const ipc = vi.hoisted(() => ({
  get: vi.fn(),
  read: vi.fn()
}))

vi.mock("@/lib/herdrIpc", () => ({
  herdrAgentGet: ipc.get,
  herdrAgentRead: ipc.read
}))

import { AnsiText } from "./AnsiText"
import { HerdrAgentInspector } from "./HerdrAgentInspector"

const capabilities: HerdrCapabilities = {
  binarySource: {
    configured: "global",
    active: "global",
    resolved: "global",
    available: true,
    path: "/bin/herdr",
    configuredAvailable: true,
    configuredPath: "/bin/herdr",
    restartRequired: false
  },
  server: { running: true },
  api: {
    snapshot: true,
    ping: true,
    tabCreate: true,
    workspaceFocus: true,
    workspaceCreate: true,
    workspaceRename: true,
    workspaceClose: true,
    tabRename: true,
    tabClose: true,
    tabFocus: true,
    paneFocus: true,
    paneRename: true,
    paneSplit: true,
    paneZoom: true,
    paneSwap: true,
    paneClose: true,
    layoutExport: true,
    layoutSetSplitRatio: true,
    agentGet: true,
    agentRead: true,
    eventsSubscribe: true,
    worktreeList: true,
    methods: ["agent.get", "agent.read", "events.subscribe"]
  },
  terminal: {
    observe: true,
    control: true,
    takeover: true,
    input: true,
    resize: true,
    scroll: true,
    release: true,
    create: true
  },
  events: { status: "available" }
}

function agent(paneId: string, title: string): HerdrAgentInfo {
  return {
    id: paneId,
    name: title,
    title,
    status: "working",
    workspaceId: "w1",
    tabId: "w1:t1",
    paneId,
    terminalId: `term-${paneId}`,
    sessionName: "default"
  }
}

function details(paneId: string, title: string): HerdrAgentDetails {
  return {
    terminalId: `term-${paneId}`,
    agentStatus: "working",
    workspaceId: "w1",
    tabId: "w1:t1",
    paneId,
    focused: false,
    revision: 1,
    title,
    stateLabels: {}
  }
}

function readResult(
  paneId: string,
  text: string,
  format: "text" | "ansi" = "text"
): HerdrAgentReadResult {
  return {
    paneId,
    workspaceId: "w1",
    tabId: "w1:t1",
    source: "recent",
    format,
    text,
    revision: 1,
    truncated: false
  }
}

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => {
    resolve = done
  })
  return { promise, resolve }
}

beforeEach(() => {
  ipc.get.mockReset()
  ipc.read.mockReset()
  useHerdrStore.setState({
    ...herdrInitialState,
    selectedSessionName: "default",
    sessions: [
      {
        name: "default",
        default: true,
        running: true,
        sessionDir: "/tmp/default",
        socketPath: "/tmp/default.sock"
      }
    ],
    runtimesBySession: {
      default: {
        capabilities,
        snapshot: null, worktreeInventory: null, connectionState: "ready",
        errorMessage: null
      }
    },
    capabilities,
    connectionState: "ready"
  })
})

describe("HerdrAgentInspector", () => {
  it("ignores an older response after the selected agent changes", async () => {
    const oldGet = deferred<HerdrAgentDetails>()
    const oldRead = deferred<HerdrAgentReadResult>()
    ipc.get.mockImplementation(({ target }: { target: string }) =>
      target === "w1:p1" ? oldGet.promise : Promise.resolve(details(target, "New"))
    )
    ipc.read.mockImplementation(({ target }: { target: string }) =>
      target === "w1:p1" ? oldRead.promise : Promise.resolve(readResult(target, "new-output"))
    )

    const view = render(
      <HerdrAgentInspector open onOpenChange={() => undefined} agent={agent("w1:p1", "Old")} />
    )
    view.rerender(
      <HerdrAgentInspector open onOpenChange={() => undefined} agent={agent("w1:p2", "New")} />
    )

    expect(await screen.findByText("new-output")).toBeInTheDocument()
    oldGet.resolve(details("w1:p1", "Old"))
    oldRead.resolve(readResult("w1:p1", "old-output"))
    await Promise.resolve()
    expect(screen.queryByText("old-output")).not.toBeInTheDocument()
  })

  it("surfaces read errors without changing Agent focus", async () => {
    ipc.get.mockResolvedValue(details("w1:p1", "Agent"))
    ipc.read.mockRejectedValue(new Error("read failed"))

    render(
      <HerdrAgentInspector open onOpenChange={() => undefined} agent={agent("w1:p1", "Agent")} />
    )

    expect(await screen.findByRole("alert")).toHaveTextContent("read failed")
    expect(ipc.get).toHaveBeenCalledWith({ sessionName: "default", target: "w1:p1" })
    expect(ipc.read).toHaveBeenCalledWith(
      expect.objectContaining({ sessionName: "default", target: "w1:p1" })
    )
  })

  it("shows empty and truncated output states", async () => {
    ipc.get.mockResolvedValue(details("w1:p1", "Agent"))
    ipc.read.mockResolvedValue({ ...readResult("w1:p1", ""), truncated: true })

    render(
      <HerdrAgentInspector open onOpenChange={() => undefined} agent={agent("w1:p1", "Agent")} />
    )

    expect(await screen.findByText(/No output|沒有輸出/)).toBeInTheDocument()
    expect(screen.getByText(/truncated|截斷/)).toBeInTheDocument()
  })

  it("clamps line requests and renders ANSI without unsafe HTML", async () => {
    ipc.get.mockImplementation(({ target }: { target: string }) =>
      Promise.resolve(details(target, "Agent"))
    )
    ipc.read.mockImplementation(
      ({ target, format }: { target: string; format: "text" | "ansi" }) =>
        Promise.resolve(
          readResult(target, format === "ansi" ? "\u001b[31mred\u001b[0m plain" : "plain", format)
        )
    )

    render(
      <HerdrAgentInspector open onOpenChange={() => undefined} agent={agent("w1:p1", "Agent")} />
    )
    const lines = await screen.findByLabelText(/Lines|行數/)
    fireEvent.change(lines, { target: { value: "999" } })
    await waitFor(() =>
      expect(ipc.read).toHaveBeenLastCalledWith(expect.objectContaining({ lines: 500 }))
    )

    const ansi = render(
      <pre>
        <AnsiText text={"\u001b[31mred\u001b[0m plain"} />
      </pre>
    )
    const red = ansi.container.querySelector("span")
    expect(red).toHaveTextContent("red")
    expect(red).toHaveStyle({ color: "#cc6666" })
    expect(ansi.container.innerHTML).not.toContain("<script")
  })
})
