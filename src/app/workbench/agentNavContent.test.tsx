import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import { AgentNavContent } from "@/app/workbench/AgentNavContent"
import type { SessionState } from "@/state/agentStore"
import { useAgentStore } from "@/state/agentStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

const originalAgentActions = {
  newSession: useAgentStore.getState().newSession,
  selectSession: useAgentStore.getState().selectSession,
}

afterEach(() => {
  cleanup()
  useAgentStore.setState({
    sessions: new Map(),
    pendingPermissions: new Map(),
    activeSessionId: null,
    connectionState: "idle",
    connection: null,
    ...originalAgentActions,
  })
  useWorkspaceStore.setState({ workspacePath: null })
  vi.clearAllMocks()
})

function session(overrides: Partial<SessionState> = {}): SessionState {
  return {
    title: "修復 git.rs error path",
    agentLabel: "Codex",
    model: "codex-1",
    tone: "run",
    transcript: [],
    availableCommands: [],
    stopReason: null,
    stopBadge: null,
    error: null,
    queueDepth: null,
    running: null,
    pendingTurn: false,
    metadataTitle: false,
    cwd: "/workspace",
    ...overrides,
  }
}

describe("AgentNavContent", () => {
  it("lists sessions with tone dot, title, and agent/model metadata", () => {
    useAgentStore.setState({
      activeSessionId: "s-run",
      sessions: new Map([
        ["s-run", session({ title: "修復 git.rs error path", tone: "run" })],
        [
          "s-fail",
          session({
            title: "整理 README",
            agentLabel: "Claude",
            model: "sonnet-4",
            tone: "fail",
          }),
        ],
      ]),
    })

    render(<AgentNavContent />)

    expect(screen.getByText("修復 git.rs error path")).toBeInTheDocument()
    expect(screen.getByText("Codex / codex-1")).toBeInTheDocument()
    expect(screen.getByText("整理 README")).toBeInTheDocument()
    expect(screen.getByText("Claude / sonnet-4")).toBeInTheDocument()
    expect(screen.getByTestId("agent-session-tone-s-run")).toHaveStyle({
      background: "var(--yz-accent)",
    })
    expect(screen.getByTestId("agent-session-tone-s-fail")).toHaveStyle({
      background: "#e23b54",
    })
    expect(screen.getByRole("button", { name: /修復 git\.rs error path/ })).toHaveAttribute(
      "aria-current",
      "page"
    )
  })

  it("clicking a session selects it", () => {
    useAgentStore.setState({
      activeSessionId: "s-run",
      sessions: new Map([
        ["s-run", session({ title: "修復 git.rs error path" })],
        ["s-done", session({ title: "整理 README", tone: "done" })],
      ]),
    })

    render(<AgentNavContent />)

    fireEvent.click(screen.getByRole("button", { name: /整理 README/ }))

    expect(useAgentStore.getState().activeSessionId).toBe("s-done")
  })

  it("uses Chinese assistive copy for the empty state", () => {
    render(<AgentNavContent />)

    expect(screen.getByText("尚無 session", { selector: ".sr-only" })).toBeInTheDocument()
  })

  it("新增 session 會使用目前 workspace cwd 建立 session", async () => {
    const newSession = vi.fn(async () => "s-new")
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    useAgentStore.setState({ newSession })

    render(<AgentNavContent />)

    const button = screen.getByRole("button", { name: "新增 session" })
    fireEvent.click(button)

    await waitFor(() => expect(newSession).toHaveBeenCalledWith("/workspace"))
  })

  it("disables 新增 session and never spawns when no folder is open", () => {
    const newSession = vi.fn(async () => "s-new")
    useWorkspaceStore.setState({ workspacePath: null })
    useAgentStore.setState({ newSession })

    render(<AgentNavContent />)

    const button = screen.getByRole("button", { name: "新增 session" })
    expect(button).toBeDisabled()
    fireEvent.click(button)

    expect(newSession).not.toHaveBeenCalled()
  })

  it("never falls back to a relative cwd when workspacePath is not absolute", () => {
    const newSession = vi.fn(async () => "s-new")
    useWorkspaceStore.setState({ workspacePath: "." })
    useAgentStore.setState({ newSession })

    render(<AgentNavContent />)

    const button = screen.getByRole("button", { name: "新增 session" })
    expect(button).toBeDisabled()
    fireEvent.click(button)

    expect(newSession).not.toHaveBeenCalled()
  })
})
