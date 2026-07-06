import { afterEach, describe, expect, it, vi } from "vitest"
import { fireEvent, render, screen, waitFor } from "@testing-library/react"

import { AgentZonePanel } from "@/app/panels/AgentZonePanel"
import { AgentAuthRequiredError, type AgentConnection, type AgentAuthMethod } from "@/agent/acpConnection"
import { agentInitialState, type SessionState } from "@/state/agentStore"
import { useAgentStore } from "@/state/agentStore"
import { useDiffModalStore } from "@/state/diffModalStore"
import { useTerminalStore } from "@/state/terminalStore"
import { uiInitialState, useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

const originalAgentActions = {
  sendPrompt: useAgentStore.getState().sendPrompt,
  cancel: useAgentStore.getState().cancel,
  respondPermission: useAgentStore.getState().respondPermission,
}

afterEach(() => {
  useAgentStore.setState({
    ...agentInitialState,
    sessions: new Map(),
    pendingPermissions: new Map(),
    ...originalAgentActions,
  })
  useDiffModalStore.setState({ open: false, source: null, activeIndex: 0, mode: "unified" })
  useTerminalStore.getState().reset()
  useUiStore.setState(uiInitialState)
  useWorkspaceStore.setState({
    workspacePath: null,
    groups: [{ tabs: [], activePath: null }],
    activeGroupIndex: 0,
    pendingReveal: null,
  })
  if (typeof localStorage.clear === "function") localStorage.clear()
  vi.restoreAllMocks()
  vi.clearAllMocks()
})

function session(overrides: Partial<SessionState> = {}): SessionState {
  return {
    title: "Audit git.rs error paths",
    agentLabel: "Codex",
    model: "codex-1",
    tone: "wait",
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

const terminalAuthMethod: AgentAuthMethod = {
  id: "pi_terminal_login",
  name: "Launch pi in the terminal",
  type: "terminal",
  args: ["--terminal-login"],
  env: {},
}

function connectedAgent() {
  const connection: AgentConnection = {
    newSession: vi.fn(async () => "s-1"),
    loadSession: vi.fn(),
    listSessions: vi.fn(async () => []),
    prompt: vi.fn(async () => "end_turn" as const),
    cancel: vi.fn(),
  }
  useAgentStore.getState().setConnection(connection)
  useAgentStore.setState({
    activeSessionId: "s-1",
    sessions: new Map([["s-1", session({ tone: "done" })]]),
  })
  return connection
}

function focusWorkspaceFile(path: string) {
  useWorkspaceStore.setState({
    workspacePath: "/workspace",
    groups: [
      {
        tabs: [{ path, name: path.split("/").pop() ?? path, dirty: false, externallyModified: false }],
        activePath: path,
      },
    ],
    activeGroupIndex: 0,
  })
}

describe("AgentZonePanel", () => {
  it("shows auth-required actions, launches terminal login, and retries session/new", async () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    const connection: AgentConnection = {
      newSession: vi.fn()
        .mockRejectedValueOnce(new AgentAuthRequiredError({
          authMethods: [terminalAuthMethod],
          cwd: "/workspace",
          sessionId: null,
        }))
        .mockResolvedValueOnce("s-after-login"),
      loadSession: vi.fn(),
      listSessions: vi.fn(async () => []),
      prompt: vi.fn(),
      cancel: vi.fn(),
    }
    useAgentStore.getState().setConnection(connection)

    await expect(useAgentStore.getState().newSession("/workspace")).rejects.toThrow("Authentication required")

    const addSession = vi.spyOn(useTerminalStore.getState(), "addSession")
    render(<AgentZonePanel />)

    expect(screen.getByText("pi 需要登入")).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "開啟終端登入" }))

    expect(addSession).toHaveBeenCalledWith(
      "/workspace",
      expect.objectContaining({
        title: "Launch pi in the terminal",
        workspace: "/workspace",
        shellArgs: ["-c", "bunx pi-acp --terminal-login"],
      })
    )
    expect(useUiStore.getState().terminalOpen).toBe(true)

    fireEvent.click(screen.getByRole("button", { name: "重試" }))

    await waitFor(() => {
      expect(connection.newSession).toHaveBeenCalledTimes(2)
      expect(screen.queryByText("pi 需要登入")).not.toBeInTheDocument()
    })
    expect(useAgentStore.getState().activeSessionId).toBe("s-after-login")
  })

  it("renders the active session transcript, block actions, streaming cursor, and minimal markdown", () => {
    const respondPermission = vi.fn()
    useAgentStore.setState({
      activeSessionId: "s-1",
      respondPermission,
      sessions: new Map([
        [
          "s-1",
          session({
            transcript: [
              { who: "you", text: "Audit git.rs error paths", streaming: false },
              {
                who: "agent",
                text: "Plan **done** with `src/git.rs`\n```ts\nconst ok = true\n```",
                streaming: true,
              },
              {
                kind: "tool",
                text: "run  bun test",
                meta: "ok · 1.8s",
                actions: [{ label: "View", kind: "view_tool" }],
              },
              {
                kind: "diff",
                text: "src/git.rs",
                meta: "+9 -3",
                actions: [
                  { label: "View", kind: "view_diff" },
                  { label: "Apply diff", kind: "apply_diff" },
                ],
              },
              {
                kind: "perm",
                text: "Permission requested · write src/git.rs",
                meta: "pending",
                actions: [
                  {
                    label: "Approve",
                    kind: "allow_once",
                    payload: { optionId: "allow_once" },
                  },
                  {
                    label: "Deny",
                    kind: "reject_once",
                    payload: { optionId: "reject_once" },
                  },
                ],
              },
              {
                kind: "error",
                text: "ACP session dropped",
                actions: [{ label: "Retry", kind: "retry" }],
              },
              { kind: "plan", text: "✓ inspect\n• edit", meta: "1/2" },
            ],
          }),
        ],
      ]),
    })

    render(<AgentZonePanel />)

    expect(screen.getAllByText("Audit git.rs error paths").length).toBeGreaterThan(0)
    expect(screen.getAllByText("Codex").length).toBeGreaterThan(0)
    expect(screen.getByText("ACP")).toBeInTheDocument()
    expect(screen.getByText("codex-1")).toBeInTheDocument()
    expect(screen.getByText("等待")).toBeInTheDocument()

    expect(screen.getByText("你")).toBeInTheDocument()
    expect(screen.getByText(/run\s+bun test/)).toBeInTheDocument()
    expect(screen.getAllByText("src/git.rs").length).toBeGreaterThan(0)
    expect(screen.getByText("Permission requested · write src/git.rs")).toBeInTheDocument()
    expect(screen.getByText("ACP session dropped")).toBeInTheDocument()
    expect(screen.getByText((content) => content.includes("✓ inspect"))).toBeInTheDocument()

    expect(screen.getByText("done").tagName).toBe("STRONG")
    expect(screen.getAllByText("src/git.rs").some((node) => node.tagName === "CODE")).toBe(true)
    expect(screen.getByText("const ok = true").closest("pre")).not.toBeNull()
    expect(screen.getByTestId("agent-streaming-cursor")).toBeInTheDocument()

    expect(screen.getAllByRole("button", { name: "View" })).toHaveLength(2)
    expect(screen.getByRole("button", { name: "Apply diff" })).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Approve" }))
    expect(respondPermission).toHaveBeenCalledWith("s-1", "allow_once")
  })

  it("opens a diff block View action with text blobs and treats null oldText as empty", () => {
    const payload = {
      toolCallId: "tc1",
      path: "src/git.rs",
      oldText: null,
      newText: "fn main() {}\n",
    }
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([
        [
          "s-1",
          session({
            transcript: [
              {
                kind: "diff",
                text: "agent diff",
                meta: JSON.stringify(payload),
                actions: [{ label: "View", kind: "view_diff", payload }],
              },
            ],
          }),
        ],
      ]),
    })

    render(<AgentZonePanel />)
    fireEvent.click(screen.getByRole("button", { name: "View" }))

    const s = useDiffModalStore.getState()
    expect(s.open).toBe(true)
    expect(s.source).toEqual({
      type: "text",
      title: "src/git.rs",
      original: { kind: "full", content: "" },
      modified: { kind: "full", content: "fn main() {}\n" },
    })
  })

  it("opens slash commands, inserts a command, and sends with Cmd+Enter", async () => {
    const sendPrompt = vi.fn(async () => "end_turn" as const)
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    useAgentStore.setState({
      activeSessionId: "s-1",
      sendPrompt,
      sessions: new Map([
        [
          "s-1",
          session({
            tone: "done",
            availableCommands: [
              { name: "fix", description: "Fix selected issue" },
              { name: "format", description: "Format the current file" },
            ],
          }),
        ],
      ]),
    })

    render(<AgentZonePanel />)

    const composer = screen.getByRole("textbox", { name: "輸入給 Agent 的訊息" })
    fireEvent.change(composer, { target: { value: "/f" } })

    expect(screen.getByRole("listbox", { name: "Slash commands" })).toBeInTheDocument()
    fireEvent.click(screen.getByRole("option", { name: "/fix Fix selected issue" }))
    expect(composer).toHaveValue("/fix ")

    fireEvent.change(composer, { target: { value: "Run tests" } })
    fireEvent.keyDown(composer, { key: "Enter", metaKey: true })

    await waitFor(() => expect(sendPrompt).toHaveBeenCalledWith("/workspace", "Run tests"))
    expect(composer).toHaveValue("")
  })

  it("sends the active editor file as a resource_link block", async () => {
    const connection = connectedAgent()
    focusWorkspaceFile("/workspace/src/main.ts")

    render(<AgentZonePanel />)

    expect(screen.getByText("main.ts")).toBeInTheDocument()
    const composer = screen.getByRole("textbox", { name: "輸入給 Agent 的訊息" })
    fireEvent.change(composer, { target: { value: "Explain this file" } })
    fireEvent.keyDown(composer, { key: "Enter", metaKey: true })

    await waitFor(() => {
      expect(connection.prompt).toHaveBeenCalledWith("s-1", [
        { type: "text", text: "Explain this file" },
        { type: "resource_link", uri: "file:///workspace/src/main.ts", name: "main.ts" },
      ])
    })
  })

  it.each([
    {
      path: "/workspace/src/hello world.ts",
      name: "hello world.ts",
      uri: "file:///workspace/src/hello%20world.ts",
    },
    {
      path: "/workspace/src/中文檔名.ts",
      name: "中文檔名.ts",
      uri: "file:///workspace/src/%E4%B8%AD%E6%96%87%E6%AA%94%E5%90%8D.ts",
    },
  ])("encodes the active editor file URI for $name", async ({ path, name, uri }) => {
    const connection = connectedAgent()
    focusWorkspaceFile(path)

    render(<AgentZonePanel />)

    expect(screen.getByText(name)).toBeInTheDocument()
    const composer = screen.getByRole("textbox", { name: "輸入給 Agent 的訊息" })
    fireEvent.change(composer, { target: { value: "Explain this file" } })
    fireEvent.keyDown(composer, { key: "Enter", metaKey: true })

    await waitFor(() => {
      expect(connection.prompt).toHaveBeenCalledWith("s-1", [
        { type: "text", text: "Explain this file" },
        { type: "resource_link", uri, name },
      ])
    })
  })

  it("omits the active editor file after its chip is removed", async () => {
    const connection = connectedAgent()
    focusWorkspaceFile("/workspace/src/main.ts")

    render(<AgentZonePanel />)

    fireEvent.click(screen.getByRole("button", { name: "移除 main.ts 檔案脈絡" }))
    expect(screen.queryByText("main.ts")).not.toBeInTheDocument()

    const composer = screen.getByRole("textbox", { name: "輸入給 Agent 的訊息" })
    fireEvent.change(composer, { target: { value: "Explain without file" } })
    fireEvent.keyDown(composer, { key: "Enter", metaKey: true })

    await waitFor(() => {
      expect(connection.prompt).toHaveBeenCalledWith("s-1", [
        { type: "text", text: "Explain without file" },
      ])
    })
  })
})
