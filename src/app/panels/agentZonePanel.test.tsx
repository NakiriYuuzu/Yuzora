import { StrictMode } from "react"
import { afterEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { open as openImageFileDialog } from "@tauri-apps/plugin-dialog"

import { AgentZonePanel } from "@/app/panels/AgentZonePanel"
import { ComposerSuggestionPopup } from "@/app/panels/ComposerSuggestionPopup"
import { AgentAuthRequiredError, type AgentConnection, type AgentAuthMethod, type PromptBlock } from "@/agent/acpConnection"
import type { BlockEntry, MsgEntry } from "@/agent/acpTypes"
import { workspaceMentionIndex } from "@/agent/workspaceMentionIndex"
import i18n from "@/lib/i18n"
import { readFileBase64 } from "@/lib/ipc"
import { agentInitialState, type SessionState } from "@/state/agentStore"
import { useAgentStore } from "@/state/agentStore"
import { useDiffModalStore } from "@/state/diffModalStore"
import { useTerminalStore } from "@/state/terminalStore"
import { uiInitialState, useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }))
vi.mock("@/lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/ipc")>()),
  readFileBase64: vi.fn(),
}))

// Shorthand for the "panels" namespace this file's strings live in — keeps the
// many assertions below readable while still asserting against the en value.
const pt = (key: string, options?: Record<string, unknown>) =>
  i18n.t(key, { ns: "panels", ...options })

const originalScrollIntoView = HTMLElement.prototype.scrollIntoView

const originalAgentActions = {
  sendPrompt: useAgentStore.getState().sendPrompt,
  cancel: useAgentStore.getState().cancel,
  respondPermission: useAgentStore.getState().respondPermission,
  newSession: useAgentStore.getState().newSession,
  setSessionConfigOption: useAgentStore.getState().setSessionConfigOption,
}

afterEach(() => {
  workspaceMentionIndex.clear()
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
  if (originalScrollIntoView) {
    Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
      configurable: true,
      writable: true,
      value: originalScrollIntoView,
    })
  } else {
    Reflect.deleteProperty(HTMLElement.prototype, "scrollIntoView")
  }
})

// fixture 專用：entry 可省略 id（stable id 由 reducer 生成，測試 literal 不需重複），
// 由 session() 依索引補上。
type TestTranscriptEntry = Omit<MsgEntry, "id"> | Omit<BlockEntry, "id">

function session(
  overrides: Partial<Omit<SessionState, "transcript">> & { transcript?: TestTranscriptEntry[] } = {}
): SessionState {
  const { transcript, ...rest } = overrides
  return {
    title: "Audit git.rs error paths",
    agentLabel: "Codex",
    model: "codex-1",
    tone: "wait",
    transcript: (transcript ?? []).map((entry, index) => ({ id: `t${index}`, ...entry })),
    availableCommands: [],
    stopReason: null,
    stopBadge: null,
    error: null,
    queueDepth: null,
    running: null,
    pendingTurn: false,
    metadataTitle: false,
    cwd: "/workspace",
    ...rest,
  }
}

function activateSessionWithCommands(commands: SessionState["availableCommands"]) {
  useWorkspaceStore.setState({ workspacePath: "/workspace" })
  useAgentStore.setState({
    activeSessionId: "s-1",
    sessions: new Map([["s-1", session({ tone: "done", availableCommands: commands })]]),
  })
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
    newSession: vi.fn(async () => ({ sessionId: "s-1", startupInfo: null })),
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

function workspaceIndexSnapshot(
  entries: Array<{ relativePath: string; canonicalPath: string }>,
  truncated = false
) {
  return {
    workspace: "/workspace",
    entries,
    truncated,
    revision: useWorkspaceStore.getState().treeRevision,
  }
}

describe("AgentZonePanel", () => {
  it("keeps loading, empty, and error rendering as data-source-independent popup slots", () => {
    const { rerender } = render(
      <ComposerSuggestionPopup
        id="generic-suggestions"
        ariaLabel="Suggestions"
        items={[]}
        selectedIndex={0}
        onSelect={() => undefined}
        status="loading"
        loadingSlot="Loading choices"
        emptySlot="No choices"
        errorSlot="Choices failed"
      />
    )
    expect(screen.getByRole("listbox", { name: "Suggestions" })).toHaveAttribute(
      "aria-busy",
      "true"
    )
    expect(screen.getByText("Loading choices")).toBeInTheDocument()

    rerender(
      <ComposerSuggestionPopup
        id="generic-suggestions"
        ariaLabel="Suggestions"
        items={[]}
        selectedIndex={0}
        onSelect={() => undefined}
        status="error"
        loadingSlot="Loading choices"
        emptySlot="No choices"
        errorSlot="Choices failed"
      />
    )
    expect(screen.getByText("Choices failed")).toBeInTheDocument()

    rerender(
      <ComposerSuggestionPopup
        id="generic-suggestions"
        ariaLabel="Suggestions"
        items={[]}
        selectedIndex={0}
        onSelect={() => undefined}
        emptySlot="No choices"
      />
    )
    expect(screen.getByText("No choices")).toBeInTheDocument()
  })

  it("creates exactly one visible draft under StrictMode after workspace hydration", async () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    const connection: AgentConnection = {
      newSession: vi.fn(async () => ({
        sessionId: "draft-1",
        startupInfo: null,
        agentIdentity: {
          selectedPreset: "pi" as const,
          commandMode: "latest" as const,
          trustedAgentId: "pi" as const,
        },
      })),
      loadSession: vi.fn(),
      listSessions: vi.fn(async () => []),
      prompt: vi.fn(async () => "end_turn" as const),
      cancel: vi.fn(),
      dropSession: vi.fn(),
    }
    useAgentStore.getState().setConnection(connection)
    useAgentStore.getState().markWorkspaceHydrated("/workspace")

    render(
      <StrictMode>
        <AgentZonePanel />
      </StrictMode>
    )

    await waitFor(() => expect(useAgentStore.getState().activeSessionId).toBe("draft-1"))
    expect(connection.newSession).toHaveBeenCalledTimes(1)
    expect(useAgentStore.getState().sessions.get("draft-1")).toMatchObject({
      cwd: "/workspace",
      ephemeral: true,
      transcript: [],
    })
    expect(screen.queryByText(pt("agentZonePanel.emptyTitle"))).not.toBeInTheDocument()
  })

  it("preserves an owned draft when Agent mode unmounts and reuses it on remount", async () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    const connection: AgentConnection = {
      newSession: vi.fn(async () => ({ sessionId: "draft-owned", startupInfo: null })),
      loadSession: vi.fn(),
      listSessions: vi.fn(async () => []),
      prompt: vi.fn(async () => "end_turn" as const),
      cancel: vi.fn(),
      dropSession: vi.fn(),
    }
    useAgentStore.getState().setConnection(connection)
    useAgentStore.getState().markWorkspaceHydrated("/workspace")

    const first = render(<AgentZonePanel />)
    await waitFor(() => expect(useAgentStore.getState().activeSessionId).toBe("draft-owned"))
    first.unmount()

    expect(useAgentStore.getState().sessions.has("draft-owned")).toBe(true)
    expect(connection.dropSession).not.toHaveBeenCalled()

    render(<AgentZonePanel />)
    await waitFor(() => expect(useAgentStore.getState().activeSessionId).toBe("draft-owned"))
    expect(connection.newSession).toHaveBeenCalledTimes(1)
  })

  it("selects an existing restored workspace session instead of creating a draft", async () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    const connection: AgentConnection = {
      newSession: vi.fn(),
      loadSession: vi.fn(),
      listSessions: vi.fn(async () => []),
      prompt: vi.fn(async () => "end_turn" as const),
      cancel: vi.fn(),
    }
    useAgentStore.getState().setConnection(connection)
    useAgentStore.getState().hydrateRestoredSessions([{
      sessionId: "restored",
      cwd: "/workspace",
      agentId: "pi",
      createdAt: 1,
      lastActiveAt: 1,
    }])
    useAgentStore.getState().markWorkspaceHydrated("/workspace")

    render(<AgentZonePanel />)

    await waitFor(() => expect(useAgentStore.getState().activeSessionId).toBe("restored"))
    expect(connection.newSession).not.toHaveBeenCalled()
  })

  it("does not let an old workspace draft completion replace the current workspace draft", async () => {
    let resolveA!: (value: { sessionId: string; startupInfo: null }) => void
    let resolveB!: (value: { sessionId: string; startupInfo: null }) => void
    const connection: AgentConnection = {
      newSession: vi.fn((cwd: string) => new Promise<{ sessionId: string; startupInfo: null }>((resolve) => {
        if (cwd === "/ws-a") resolveA = resolve
        else resolveB = resolve
      })),
      loadSession: vi.fn(),
      listSessions: vi.fn(async () => []),
      prompt: vi.fn(async () => "end_turn" as const),
      cancel: vi.fn(),
      dropSession: vi.fn(),
      disposePrepared: vi.fn(async () => true),
    }
    useAgentStore.getState().setConnection(connection)
    useAgentStore.getState().markWorkspaceHydrated("/ws-a")
    useAgentStore.getState().markWorkspaceHydrated("/ws-b")
    useWorkspaceStore.setState({ workspacePath: "/ws-a" })
    const view = render(<AgentZonePanel />)
    await waitFor(() => expect(connection.newSession).toHaveBeenCalledWith("/ws-a", "pi"))

    act(() => useWorkspaceStore.setState({ workspacePath: "/ws-b" }))
    view.rerender(<AgentZonePanel />)
    await waitFor(() => expect(connection.newSession).toHaveBeenCalledWith("/ws-b", "pi"))

    resolveA({ sessionId: "draft-a", startupInfo: null })
    await waitFor(() => expect(connection.dropSession).toHaveBeenCalledWith("draft-a"))
    expect(useAgentStore.getState().sessions.has("draft-a")).toBe(false)

    resolveB({ sessionId: "draft-b", startupInfo: null })
    await waitFor(() => expect(useAgentStore.getState().activeSessionId).toBe("draft-b"))
  })

  it("activates A→B→A at workspace scope, including the reopened A", async () => {
    const activateWorkspace = vi.spyOn(workspaceMentionIndex, "activateWorkspace")
    activateSessionWithCommands([])

    render(<AgentZonePanel />)
    await waitFor(() => expect(activateWorkspace).toHaveBeenCalledTimes(1))
    expect(activateWorkspace).toHaveBeenLastCalledWith("/workspace")

    await act(async () => useWorkspaceStore.setState({ workspacePath: "/workspace-b" }))
    await waitFor(() => expect(activateWorkspace).toHaveBeenCalledTimes(2))
    expect(activateWorkspace).toHaveBeenLastCalledWith("/workspace-b")

    await act(async () => useWorkspaceStore.setState({ workspacePath: "/workspace" }))
    await waitFor(() => expect(activateWorkspace).toHaveBeenCalledTimes(3))
    expect(activateWorkspace).toHaveBeenLastCalledWith("/workspace")
  })

  it("shows auth-required actions, launches terminal login, and retries session/new", async () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    const connection: AgentConnection = {
      newSession: vi.fn()
        .mockRejectedValueOnce(new AgentAuthRequiredError({
          authMethods: [terminalAuthMethod],
          cwd: "/workspace",
          sessionId: null,
        }))
        .mockResolvedValueOnce({ sessionId: "s-after-login", startupInfo: null }),
      loadSession: vi.fn(),
      listSessions: vi.fn(async () => []),
      prompt: vi.fn(),
      cancel: vi.fn(),
    }
    useAgentStore.getState().setConnection(connection)

    await expect(useAgentStore.getState().newSession("/workspace")).rejects.toThrow("Authentication required")

    const addSession = vi.spyOn(useTerminalStore.getState(), "addSession")
    render(<AgentZonePanel />)

    expect(screen.getByText(pt("agentZonePanel.authRequiredTitle"))).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: pt("agentZonePanel.openTerminalLogin") }))

    expect(addSession).toHaveBeenCalledWith(
      "/workspace",
      expect.objectContaining({
        title: "Launch pi in the terminal",
        workspace: "/workspace",
        shellArgs: ["-c", "bunx pi-acp@latest --terminal-login"],
      })
    )
    expect(useUiStore.getState().terminalOpen).toBe(true)

    fireEvent.click(screen.getByRole("button", { name: pt("agentZonePanel.retry") }))

    await waitFor(() => {
      expect(connection.newSession).toHaveBeenCalledTimes(2)
      expect(screen.queryByText(pt("agentZonePanel.authRequiredTitle"))).not.toBeInTheDocument()
    })
    expect(useAgentStore.getState().activeSessionId).toBe("s-after-login")
  })

  it("renders a connection-error banner with connectionError text and retries via newSession", async () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    const connection: AgentConnection = {
      newSession: vi.fn(async () => ({ sessionId: "s-retry", startupInfo: null })),
      loadSession: vi.fn(),
      listSessions: vi.fn(async () => []),
      prompt: vi.fn(),
      cancel: vi.fn(),
    }
    useAgentStore.getState().setConnection(connection)
    useAgentStore.setState({
      connectionState: "error",
      connectionError: "ACP initialize timed out after 60000ms",
    })

    render(<AgentZonePanel />)

    expect(screen.getByText(pt("agentZonePanel.connectionErrorTitle"))).toBeInTheDocument()
    expect(screen.getByText("ACP initialize timed out after 60000ms")).toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: pt("agentZonePanel.retry") }))

    await waitFor(() => {
      expect(connection.newSession).toHaveBeenCalledWith("/workspace")
    })
  })

  it("does not show the connection-error banner when auth is required instead", () => {
    useAgentStore.setState({
      connectionState: "error",
      connectionError: "Authentication required",
      authRequired: {
        cwd: "/workspace",
        sessionId: null,
        authMethods: [terminalAuthMethod],
        message: "Authentication required",
      },
    })

    render(<AgentZonePanel />)

    expect(screen.getByText(pt("agentZonePanel.authRequiredTitle"))).toBeInTheDocument()
    expect(screen.queryByText(pt("agentZonePanel.connectionErrorTitle"))).not.toBeInTheDocument()
  })

  it("renders the active session transcript and minimal markdown without a stale wait-state cursor", () => {
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
    expect(screen.queryByText("codex-1")).not.toBeInTheDocument()
    expect(screen.getByText(pt("agentZonePanel.tone.wait"))).toBeInTheDocument()

    // P2：sender 標籤列移除、改 aria-label（user 玻璃氣泡）。
    expect(screen.getByLabelText(pt("agentZonePanel.you"))).toBeInTheDocument()
    // tool 已聚合進 activity 鏈（P1）：預設收合、標題不直接可見，展開後才出現。
    expect(screen.queryByText(/run\s+bun test/)).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: pt("agentZonePanel.chainSteps", { n: 1 }) }))
    expect(screen.getByText(/run\s+bun test/)).toBeInTheDocument()
    expect(screen.getAllByText("src/git.rs").length).toBeGreaterThan(0)
    expect(screen.getByText("Permission requested · write src/git.rs")).toBeInTheDocument()
    expect(screen.getByText("ACP session dropped")).toBeInTheDocument()
    expect(screen.getByText((content) => content.includes("✓ inspect"))).toBeInTheDocument()

    expect(screen.getByText("done").tagName).toBe("STRONG")
    expect(screen.getAllByText("src/git.rs").some((node) => node.tagName === "CODE")).toBe(true)
    expect(screen.getByText("const ok = true").closest("pre")).not.toBeNull()
    expect(screen.queryByTestId("agent-streaming-cursor")).not.toBeInTheDocument()

    // view/apply diff 功能已移除（2026-07-21 回饋）：不應再出現任何 diff action 按鈕。
    expect(screen.queryByRole("button", { name: "View" })).toBeNull()
    expect(screen.queryByRole("button", { name: "Apply diff" })).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "Approve" }))
    expect(respondPermission).toHaveBeenCalledWith("s-1", "allow_once")
  })

  it("renders exactly one cursor only for a running, streaming agent text at the deduped tail", () => {
    const cases: Array<{
      name: string
      tone: SessionState["tone"]
      transcript: TestTranscriptEntry[]
      infoBanner?: string
      expected: number
    }> = [
      {
        name: "running streaming agent tail",
        tone: "run",
        transcript: [{ who: "agent", text: "live", streaming: true }],
        expected: 1,
      },
      {
        name: "two streaming agents still produce one tail cursor",
        tone: "run",
        transcript: [
          { who: "agent", text: "older", streaming: true },
          { who: "agent", text: "live", streaming: true },
        ],
        expected: 1,
      },
      {
        name: "streaming agent before user tail",
        tone: "run",
        transcript: [
          { who: "agent", text: "older", streaming: true },
          { who: "you", text: "follow-up", streaming: true },
        ],
        expected: 0,
      },
      {
        name: "streaming agent before tool tail",
        tone: "run",
        transcript: [
          { who: "agent", text: "older", streaming: true },
          { kind: "tool", text: "running tool" },
        ],
        expected: 0,
      },
      {
        name: "permission tail",
        tone: "run",
        transcript: [
          { who: "agent", text: "older", streaming: true },
          { kind: "perm", text: "Permission requested" },
        ],
        expected: 0,
      },
      {
        name: "thought tail",
        tone: "run",
        transcript: [
          { who: "agent", text: "older", streaming: true },
          { kind: "thought", text: "Thinking" },
        ],
        expected: 0,
      },
      {
        name: "wait tone",
        tone: "wait",
        transcript: [{ who: "agent", text: "waiting", streaming: true }],
        expected: 0,
      },
      {
        name: "done tone",
        tone: "done",
        transcript: [{ who: "agent", text: "done", streaming: true }],
        expected: 0,
      },
      {
        name: "cancel notice tail",
        tone: "idle",
        transcript: [
          { who: "agent", text: "older", streaming: true },
          { kind: "notice", text: "Cancelled" },
        ],
        expected: 0,
      },
      {
        name: "error tail",
        tone: "fail",
        transcript: [
          { who: "agent", text: "older", streaming: true },
          { kind: "error", text: "Failed" },
        ],
        expected: 0,
      },
      {
        name: "non-streaming agent tail",
        tone: "run",
        transcript: [{ who: "agent", text: "complete", streaming: false }],
        expected: 0,
      },
      {
        name: "deduped sole info banner",
        tone: "run",
        infoBanner: "Startup",
        transcript: [{ who: "agent", text: "Startup", streaming: true }],
        expected: 0,
      },
      {
        name: "live tail after info-banner dedupe",
        tone: "run",
        infoBanner: "Startup",
        transcript: [
          { who: "agent", text: "Startup", streaming: false },
          { who: "agent", text: "live", streaming: true },
        ],
        expected: 1,
      },
    ]

    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    const view = render(<AgentZonePanel />)

    for (const testCase of cases) {
      act(() => {
        useAgentStore.setState({
          activeSessionId: "s-1",
          sessions: new Map([["s-1", session({
            tone: testCase.tone,
            transcript: testCase.transcript,
            infoBanner: testCase.infoBanner,
          })]]),
        })
      })
      view.rerender(<AgentZonePanel />)
      expect(
        screen.queryAllByTestId("agent-streaming-cursor"),
        testCase.name
      ).toHaveLength(testCase.expected)
    }
  })

  it("P1: failed tool step in the activity chain expands to details with destructive styling", () => {
    const failedMeta = JSON.stringify({ toolCallId: "tc1", status: "failed", rawOutput: { error: "nope" } })
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([
        [
          "s-1",
          session({
            transcript: [
              { kind: "tool", text: `write a.txt\n${"x".repeat(120)}`, meta: failedMeta },
            ],
          }),
        ],
      ]),
    })

    render(<AgentZonePanel />)

    // 收合：鏈只剩一行 header，rawOutput 未展開
    expect(screen.queryByText(/"error": "nope"/)).toBeNull()

    // 展開鏈：header 顯示步驟數與失敗計數
    const header = screen
      .getByText(pt("agentZonePanel.chainSteps", { n: 1 }))
      .closest("button") as HTMLElement
    expect(header.textContent).toContain(pt("agentZonePanel.chainFailed", { n: 1 }))
    fireEvent.click(header)

    // 失敗態：step 用 destructive 樣式（vendored chain-of-thought 的 failed 擴充）
    const stepToggle = screen.getByRole("button", { name: pt("agentZonePanel.toolToggle") })
    expect(stepToggle.closest(".text-destructive")).not.toBeNull()

    // 展開 step 明細
    fireEvent.click(stepToggle)
    expect(screen.getByText(/"error": "nope"/)).toBeInTheDocument()
  })

  it("P3: an answered permission card locks its options and shows the chosen result", () => {
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([["s-1", session({
        tone: "run",
        transcript: [
          {
            kind: "perm",
            text: "Permission requested · write src/git.rs",
            actions: [
              { label: "Approve", kind: "allow_once", payload: { optionId: "allow_once" } },
              { label: "Deny", kind: "reject_once", payload: { optionId: "reject_once" } },
            ],
          },
        ],
        permissionOutcomes: { t0: "allow_once" },
      })]]),
    })

    render(<AgentZonePanel />)

    const approve = screen.getByRole("button", { name: /Approve/ })
    const deny = screen.getByRole("button", { name: "Deny" })
    expect(approve).toBeDisabled()
    expect(deny).toBeDisabled()
    expect(approve).toHaveAttribute("aria-pressed", "true")
    expect(deny).toHaveAttribute("aria-pressed", "false")
    expect(
      screen.getByText(pt("agentZonePanel.permAnswered", { option: "Approve" }))
    ).toBeInTheDocument()
  })

  it("Phase 4: renders a degrade notice block and starts a new session via its action", () => {
    const newSession = vi.fn(async () => "s-2")
    useAgentStore.setState({
      activeSessionId: "s-1",
      newSession,
      sessions: new Map([
        [
          "s-1",
          session({
            transcript: [
              {
                kind: "notice",
                text: "This agent can't restore the previous conversation.",
                actions: [
                  {
                    label: "Start a new conversation with the same agent",
                    kind: "start_new_session",
                    payload: { cwd: "/workspace", agentId: "codex" },
                  },
                ],
              },
            ],
          }),
        ],
      ]),
    })

    render(<AgentZonePanel />)

    expect(screen.getByText("This agent can't restore the previous conversation.")).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Start a new conversation with the same agent" }))

    expect(newSession).toHaveBeenCalledWith("/workspace", "codex")
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

    const composer = screen.getByRole("combobox", {
      name: pt("agentZonePanel.composerAriaLabel"),
    }) as HTMLTextAreaElement
    fireEvent.change(composer, { target: { value: "/f" } })

    expect(screen.getByRole("listbox", { name: "Slash commands" })).toBeInTheDocument()
    fireEvent.click(screen.getByRole("option", { name: "/fix Fix selected issue" }))
    expect(composer).toHaveValue("/fix ")

    fireEvent.change(composer, { target: { value: "Run tests" } })
    fireEvent.keyDown(composer, { key: "Enter", metaKey: true })

    await waitFor(() => expect(sendPrompt).toHaveBeenCalledWith("/workspace", [
      { type: "text", text: "Run tests" },
    ]))
    expect(composer).toHaveValue("")
  })

  describe("圖片附件（P4）", () => {
    function imageCapableSetup(
      sendPrompt = vi.fn(async (_cwd: string, _prompt: string | PromptBlock[]) => "end_turn" as const)
    ) {
      const connection = {
        newSession: vi.fn(async () => ({ sessionId: "s-1", startupInfo: null })),
        loadSession: vi.fn(),
        listSessions: vi.fn(async () => []),
        prompt: vi.fn(async () => "end_turn" as const),
        cancel: vi.fn(),
        supportsImagePrompt: vi.fn(() => true),
      } as unknown as AgentConnection
      useWorkspaceStore.setState({ workspacePath: "/workspace" })
      useAgentStore.getState().setConnection(connection)
      useAgentStore.setState({
        activeSessionId: "s-1",
        sendPrompt,
        sessions: new Map([["s-1", session({ tone: "done" })]]),
      })
      return { connection, sendPrompt }
    }

    function pasteImage(composer: HTMLElement, file: File) {
      fireEvent.paste(composer, {
        clipboardData: {
          items: [{ type: file.type, getAsFile: () => file }],
        },
      })
    }

    function pngFile(name = "shot.png", bytes = 8, type = "image/png") {
      return new File([new Uint8Array(bytes)], name, { type })
    }

    it("貼上圖片建立縮圖 chip；送出時轉 image block 且送後清空", async () => {
      const { sendPrompt } = imageCapableSetup()
      render(<AgentZonePanel />)
      const composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })

      pasteImage(composer, pngFile())
      const chip = await screen.findByTestId("composer-image-chip")
      expect(within(chip).getByRole("img")).toHaveAttribute("src", expect.stringContaining("data:image/png;base64,"))

      fireEvent.change(composer, { target: { value: "what is this" } })
      fireEvent.keyDown(composer, { key: "Enter", metaKey: true })

      await waitFor(() => expect(sendPrompt).toHaveBeenCalledTimes(1))
      const blocks = sendPrompt.mock.calls[0][1] as PromptBlock[]
      expect(blocks[0]).toEqual({ type: "text", text: "what is this" })
      expect(blocks[1]).toMatchObject({ type: "image", mimeType: "image/png" })
      expect(typeof (blocks[1] as { data: unknown }).data).toBe("string")
      expect((blocks[1] as { data: string }).data).not.toContain("data:")
      expect(screen.queryByTestId("composer-image-chip")).not.toBeInTheDocument()
    })

    it("純圖片（無文字）也可送出", async () => {
      const { sendPrompt } = imageCapableSetup()
      render(<AgentZonePanel />)
      const composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })

      pasteImage(composer, pngFile())
      await screen.findByTestId("composer-image-chip")
      fireEvent.keyDown(composer, { key: "Enter", metaKey: true })

      await waitFor(() => expect(sendPrompt).toHaveBeenCalledTimes(1))
      const blocks = sendPrompt.mock.calls[0][1] as PromptBlock[]
      expect(blocks).toHaveLength(1)
      expect(blocks[0]).toMatchObject({ type: "image" })
    })

    it("超過 5MB 拒絕並顯示 notice", async () => {
      imageCapableSetup()
      render(<AgentZonePanel />)
      const composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })

      const big = pngFile("big.png")
      Object.defineProperty(big, "size", { value: 6 * 1024 * 1024 })
      pasteImage(composer, big)

      expect(await screen.findByTestId("composer-notice")).toHaveTextContent(
        pt("agentZonePanel.imageTooLarge", { max: "5MB" })
      )
      expect(screen.queryByTestId("composer-image-chip")).not.toBeInTheDocument()
    })

    it("非白名單 mime 拒絕並顯示 notice", async () => {
      imageCapableSetup()
      render(<AgentZonePanel />)
      const composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })

      pasteImage(composer, pngFile("art.tiff", 8, "image/tiff"))

      expect(await screen.findByTestId("composer-notice")).toHaveTextContent(
        pt("agentZonePanel.imageBadType")
      )
      expect(screen.queryByTestId("composer-image-chip")).not.toBeInTheDocument()
    })

    it("超過 8 張上限時拒絕多的並顯示 notice", async () => {
      imageCapableSetup()
      render(<AgentZonePanel />)
      const composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })

      for (let i = 0; i < 9; i++) pasteImage(composer, pngFile(`s${i}.png`))

      await waitFor(() =>
        expect(screen.getAllByTestId("composer-image-chip")).toHaveLength(8)
      )
      expect(screen.getByTestId("composer-notice")).toHaveTextContent(
        pt("agentZonePanel.imageTooMany", { max: 8 })
      )
    })

    it("capability=false：不渲染上傳按鈕，貼圖顯示不支援提示", async () => {
      const sendPrompt = vi.fn(async () => "end_turn" as const)
      const connection = {
        newSession: vi.fn(async () => ({ sessionId: "s-1", startupInfo: null })),
        loadSession: vi.fn(),
        listSessions: vi.fn(async () => []),
        prompt: vi.fn(async () => "end_turn" as const),
        cancel: vi.fn(),
        supportsImagePrompt: vi.fn(() => false),
      } as unknown as AgentConnection
      useWorkspaceStore.setState({ workspacePath: "/workspace" })
      useAgentStore.getState().setConnection(connection)
      useAgentStore.setState({
        activeSessionId: "s-1",
        sendPrompt,
        sessions: new Map([["s-1", session({ tone: "done" })]]),
      })
      render(<AgentZonePanel />)

      expect(
        screen.queryByRole("button", { name: pt("agentZonePanel.attachImage") })
      ).not.toBeInTheDocument()

      const composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })
      pasteImage(composer, pngFile())

      expect(await screen.findByTestId("composer-notice")).toHaveTextContent(
        pt("agentZonePanel.imageNotSupported", { agent: "Codex" })
      )
      expect(screen.queryByTestId("composer-image-chip")).not.toBeInTheDocument()
    })

    it("capability=true：渲染上傳按鈕", () => {
      imageCapableSetup()
      render(<AgentZonePanel />)

      expect(
        screen.getByRole("button", { name: pt("agentZonePanel.attachImage") })
      ).toBeInTheDocument()
    })

    it("Windows extended image picker 只顯示 basename 並以 raw path 讀檔", async () => {
      imageCapableSetup()
      const rawPath = String.raw`\\?\C:\工作區\截圖.png`
      vi.mocked(openImageFileDialog).mockResolvedValue(rawPath)
      vi.mocked(readFileBase64).mockResolvedValue({ data: "aGVsbG8=", size: 5 })
      render(<AgentZonePanel />)

      fireEvent.click(screen.getByRole("button", { name: pt("agentZonePanel.attachImage") }))

      const chip = await screen.findByTestId("composer-image-chip")
      expect(chip).toHaveTextContent("截圖.png")
      expect(chip).not.toHaveTextContent(rawPath)
      expect(readFileBase64).toHaveBeenCalledWith(rawPath, expect.any(Number))
    })

    it("user bubble 渲染已送圖片縮圖", () => {
      useWorkspaceStore.setState({ workspacePath: "/workspace" })
      useAgentStore.setState({
        activeSessionId: "s-1",
        sessions: new Map([
          [
            "s-1",
            session({
              tone: "done",
              transcript: [
                {
                  who: "you",
                  text: "look",
                  streaming: false,
                  images: [{ mimeType: "image/png", dataUrl: "data:image/png;base64,aGVsbG8=" }],
                },
              ],
            }),
          ],
        ]),
      })
      render(<AgentZonePanel />)

      const strip = screen.getByTestId("message-image-strip")
      expect(within(strip).getByRole("presentation")).toHaveAttribute(
        "src",
        "data:image/png;base64,aGVsbG8="
      )
    })

    it("0-byte 圖片拒收並顯示 notice", async () => {
      imageCapableSetup()
      render(<AgentZonePanel />)
      const composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })

      pasteImage(composer, pngFile("empty.png", 0))

      expect(await screen.findByTestId("composer-notice")).toHaveTextContent(
        pt("agentZonePanel.imageEmpty")
      )
      expect(screen.queryByTestId("composer-image-chip")).not.toBeInTheDocument()
    })

    it("純圖片訊息的 bubble 只顯示縮圖，不重複渲染 [image] 佔位文字", () => {
      useWorkspaceStore.setState({ workspacePath: "/workspace" })
      useAgentStore.setState({
        activeSessionId: "s-1",
        sessions: new Map([
          [
            "s-1",
            session({
              tone: "done",
              transcript: [
                {
                  who: "you",
                  text: "[image] [image]",
                  streaming: false,
                  images: [
                    { mimeType: "image/png", dataUrl: "data:image/png;base64,aGVsbG8=" },
                    { mimeType: "image/png", dataUrl: "data:image/png;base64,d29ybGQ=" },
                  ],
                },
              ],
            }),
          ],
        ]),
      })
      render(<AgentZonePanel />)

      const bubble = screen.getByTestId("agent-message-bubble")
      expect(within(bubble).getByTestId("message-image-strip")).toBeInTheDocument()
      expect(bubble.textContent).not.toContain("[image]")
      // 混合訊息保留原文（佔位標記圖片在文中的位置）。
      act(() => {
        useAgentStore.setState({
          sessions: new Map([
            [
              "s-1",
              session({
                tone: "done",
                transcript: [
                  {
                    who: "you",
                    text: "look at this [image]",
                    streaming: false,
                    images: [{ mimeType: "image/png", dataUrl: "data:image/png;base64,aGVsbG8=" }],
                  },
                ],
              }),
            ],
          ]),
        })
      })
      expect(screen.getByTestId("agent-message-bubble").textContent).toContain(
        "look at this [image]"
      )
    })
  })

  it("uses one keyboard path for direct typing with complete listbox ARIA and nearest scrolling", async () => {
    const scrollIntoView = vi.fn()
    Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
      configurable: true,
      writable: true,
      value: scrollIntoView,
    })
    activateSessionWithCommands([
      { name: "fix", description: "Fix selected issue" },
      { name: "format", description: "Format the current file" },
      { name: "review", description: "Review the current file" },
    ])
    render(<AgentZonePanel />)

    const composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })
    composer.focus()
    fireEvent.change(composer, { target: { value: "/f" } })

    const listbox = screen.getByRole("listbox", { name: pt("agentZonePanel.slashCommands") })
    const fix = screen.getByRole("option", { name: "/fix Fix selected issue" })
    const format = screen.getByRole("option", { name: "/format Format the current file" })
    expect(composer).toHaveAttribute("aria-expanded", "true")
    expect(composer).toHaveAttribute("aria-autocomplete", "list")
    expect(composer).toHaveAttribute("aria-controls", listbox.id)
    expect(composer).toHaveAttribute("aria-activedescendant", fix.id)
    expect(fix).toHaveAttribute("aria-selected", "true")
    expect(format).toHaveAttribute("aria-selected", "false")
    await waitFor(() => expect(scrollIntoView).toHaveBeenCalledWith({ block: "nearest" }))

    scrollIntoView.mockClear()
    fireEvent.keyDown(composer, { key: "ArrowDown" })
    expect(fix).toHaveAttribute("aria-selected", "false")
    expect(format).toHaveAttribute("aria-selected", "true")
    expect(composer).toHaveAttribute("aria-activedescendant", format.id)
    await waitFor(() => expect(scrollIntoView).toHaveBeenCalledWith({ block: "nearest" }))

    fireEvent.keyDown(composer, { key: "Tab" })
    await waitFor(() => {
      expect(composer).toHaveValue("/format ")
      expect(composer).toHaveFocus()
      expect(composer).toHaveProperty("selectionStart", 8)
    })
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument()
    expect(composer).not.toHaveAttribute("aria-controls")
    expect(composer).not.toHaveAttribute("aria-activedescendant")

    fireEvent.change(composer, { target: { value: "/r" } })
    fireEvent.keyDown(composer, { key: "Enter" })
    await waitFor(() => expect(composer).toHaveValue("/review "))
  })

  it("inserts slash at the caret, preserves range-external text, and toggles without duplicate slashes", async () => {
    activateSessionWithCommands([
      { name: "fix", description: "Fix selected issue" },
      { name: "format", description: "Format the current file" },
    ])
    render(<AgentZonePanel />)

    const composer = screen.getByRole("combobox", {
      name: pt("agentZonePanel.composerAriaLabel"),
    }) as HTMLTextAreaElement
    const slashButton = screen.getByRole("button", { name: pt("agentZonePanel.slashCommands") })
    fireEvent.change(composer, { target: { value: "prefix suffix" } })
    composer.setSelectionRange(7, 7)
    slashButton.focus()
    fireEvent.click(slashButton)

    await waitFor(() => {
      expect(composer).toHaveValue("prefix /suffix")
      expect(composer).toHaveFocus()
      expect(composer).toHaveProperty("selectionStart", 8)
      expect(composer).toHaveAttribute("data-slot", "input-group-control")
      expect(composer.closest('[data-slot="input-group"]')).not.toBeNull()
    })
    expect(screen.getByRole("listbox")).toBeInTheDocument()

    fireEvent.click(slashButton)
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument()
    expect(composer).toHaveValue("prefix /suffix")

    fireEvent.click(slashButton)
    expect(screen.getByRole("listbox")).toBeInTheDocument()
    expect(composer).toHaveValue("prefix /suffix")

    fireEvent.keyDown(composer, { key: "ArrowDown" })
    fireEvent.keyDown(composer, { key: "Enter" })
    await waitFor(() => {
      expect(composer).toHaveValue("prefix /format suffix")
      expect(composer).toHaveProperty("selectionStart", 15)
    })
  })

  it("dismisses with Escape and reopens the same trigger from the slash button without editing text", () => {
    activateSessionWithCommands([{ name: "fix", description: "Fix selected issue" }])
    render(<AgentZonePanel />)

    const composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })
    const slashButton = screen.getByRole("button", { name: pt("agentZonePanel.slashCommands") })
    fireEvent.change(composer, { target: { value: "/f" } })
    expect(screen.getByRole("listbox")).toBeInTheDocument()

    fireEvent.keyDown(composer, { key: "Escape" })
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument()
    expect(composer).toHaveValue("/f")
    expect(composer).toHaveAttribute("aria-expanded", "false")
    expect(composer).not.toHaveAttribute("aria-controls")

    fireEvent.click(slashButton)
    expect(screen.getByRole("listbox")).toBeInTheDocument()
    expect(composer).toHaveValue("/f")
  })

  it("refreshes directly from late availableCommands updates and clamps selection after shrink", () => {
    activateSessionWithCommands([
      { name: "one", description: "First" },
      { name: "two", description: "Second" },
      { name: "three", description: "Third" },
    ])
    render(<AgentZonePanel />)

    const composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })
    fireEvent.change(composer, { target: { value: "/" } })
    fireEvent.keyDown(composer, { key: "ArrowDown" })
    fireEvent.keyDown(composer, { key: "ArrowDown" })
    expect(screen.getByRole("option", { name: "/three Third" })).toHaveAttribute(
      "aria-selected",
      "true"
    )

    act(() => useAgentStore.getState().setAvailableCommands("s-1", []))
    expect(screen.queryByRole("option")).not.toBeInTheDocument()
    expect(screen.getByText(pt("agentZonePanel.noCommands"))).toBeInTheDocument()
    expect(composer).not.toHaveAttribute("aria-activedescendant")

    act(() => useAgentStore.getState().setAvailableCommands("s-1", [
      { name: "late", description: "Arrived later" },
    ]))
    const late = screen.getByRole("option", { name: "/late Arrived later" })
    expect(late).toHaveAttribute("aria-selected", "true")
    expect(composer).toHaveAttribute("aria-activedescendant", late.id)
  })

  it("preserves composer focus on pointer down and explicitly restores it after selection", async () => {
    activateSessionWithCommands([{ name: "fix", description: "Fix selected issue" }])
    const outside = document.createElement("button")
    document.body.appendChild(outside)
    render(<AgentZonePanel />)

    const composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })
    composer.focus()
    fireEvent.change(composer, { target: { value: "/f" } })
    const option = screen.getByRole("option", { name: "/fix Fix selected issue" })
    const mouseDown = new MouseEvent("mousedown", { bubbles: true, cancelable: true })
    fireEvent(option, mouseDown)
    expect(mouseDown.defaultPrevented).toBe(true)
    expect(composer).toHaveFocus()

    outside.focus()
    expect(composer).not.toHaveFocus()
    fireEvent.click(option)
    await waitFor(() => {
      expect(composer).toHaveFocus()
      expect(composer).toHaveValue("/fix ")
      expect(composer).toHaveProperty("selectionStart", 5)
      expect(composer).toHaveAttribute("data-slot", "input-group-control")
      expect(composer.closest('[data-slot="input-group"]')).not.toBeNull()
    })
    outside.remove()
  })

  it("suppresses suggestion opening and application for the full IME composition", () => {
    activateSessionWithCommands([{ name: "fix", description: "Fix selected issue" }])
    render(<AgentZonePanel />)

    const composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })
    const slashButton = screen.getByRole("button", { name: pt("agentZonePanel.slashCommands") })
    fireEvent.compositionStart(composer)
    fireEvent.change(composer, { target: { value: "/f" } })
    fireEvent.keyDown(composer, { key: "ArrowDown" })
    fireEvent.keyDown(composer, { key: "Enter" })
    fireEvent.click(slashButton)
    expect(composer).toHaveValue("/f")
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument()

    fireEvent.compositionEnd(composer)
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument()

    fireEvent.change(composer, { target: { value: "/fi" } })
    expect(screen.getByRole("listbox")).toBeInTheDocument()
    fireEvent.compositionStart(composer)
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument()
    fireEvent.keyDown(composer, { key: "Enter" })
    expect(composer).toHaveValue("/fi")
  })

  it("remounts composer intent by session so text, popup, and selection never cross sessions", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([
        ["s-1", session({ tone: "done", availableCommands: [
          { name: "fix", description: "Fix" },
          { name: "format", description: "Format" },
        ] })],
        ["s-2", session({ title: "Second", tone: "done", availableCommands: [
          { name: "review", description: "Review" },
        ] })],
      ]),
    })
    render(<AgentZonePanel />)

    let composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })
    fireEvent.change(composer, { target: { value: "/f" } })
    fireEvent.keyDown(composer, { key: "ArrowDown" })
    expect(screen.getByRole("option", { name: "/format Format" })).toHaveAttribute(
      "aria-selected",
      "true"
    )

    act(() => useAgentStore.setState({ activeSessionId: "s-2" }))
    composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })
    expect(composer).toHaveValue("")
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument()
    expect(composer).not.toHaveAttribute("aria-activedescendant")
    fireEvent.change(composer, { target: { value: "second draft" } })

    act(() => useAgentStore.setState({ activeSessionId: "s-1" }))
    composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })
    expect(composer).toHaveValue("")
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument()
  })

  it("lazily indexes legal @ triggers, dedupes the active file, and sends ordered resource links", async () => {
    const connection = connectedAgent()
    focusWorkspaceFile("/workspace/src/main.ts")
    const load = vi.spyOn(workspaceMentionIndex, "load").mockResolvedValue(workspaceIndexSnapshot([
      { relativePath: "src/main.ts", canonicalPath: "/workspace/src/main.ts" },
      { relativePath: "docs/spec file.md", canonicalPath: "/workspace/docs/spec file.md" },
    ], true))

    render(<AgentZonePanel />)
    expect(load).not.toHaveBeenCalled()
    const composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })
    fireEvent.change(composer, { target: { value: "email@example.com" } })
    expect(load).not.toHaveBeenCalled()

    fireEvent.change(composer, { target: { value: "@src" } })
    const activeFileOption = await screen.findByRole("option", { name: "Attach src/main.ts" })
    expect(load).toHaveBeenCalledWith("/workspace", useWorkspaceStore.getState().treeRevision)
    expect(screen.getByText(/first 50,000 files/)).toBeInTheDocument()
    fireEvent.click(activeFileOption)

    fireEvent.change(composer, { target: { value: "@spec" } })
    fireEvent.click(await screen.findByRole("option", { name: "Attach docs/spec file.md" }))
    expect(screen.getAllByRole("listitem")).toHaveLength(2)

    fireEvent.change(composer, { target: { value: "Review both" } })
    fireEvent.keyDown(composer, { key: "Enter", metaKey: true })
    await waitFor(() => expect(connection.prompt).toHaveBeenCalledWith("s-1", [
      { type: "text", text: "Review both" },
      { type: "resource_link", uri: "file:///workspace/src/main.ts", name: "main.ts" },
      {
        type: "resource_link",
        uri: "file:///workspace/docs/spec%20file.md",
        name: "docs/spec file.md",
      },
    ]))
  })

  it("renders nonblocking workspace-index loading and visible error states", async () => {
    let rejectLoad!: (error: unknown) => void
    vi.spyOn(workspaceMentionIndex, "load").mockReturnValue(new Promise((_resolve, reject) => {
      rejectLoad = reject
    }))
    activateSessionWithCommands([])
    render(<AgentZonePanel />)

    const composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })
    fireEvent.change(composer, { target: { value: "@src" } })
    expect(await screen.findByText(pt("agentZonePanel.workspaceIndexLoading"))).toBeInTheDocument()
    expect(composer).toHaveValue("@src")

    await act(async () => rejectLoad(new Error("walk failed")))
    expect(await screen.findByRole("alert")).toHaveTextContent("walk failed")
    expect(composer).toHaveValue("@src")
  })

  it("renders a dedicated ready-but-empty @ state", async () => {
    vi.spyOn(workspaceMentionIndex, "load").mockResolvedValue(workspaceIndexSnapshot([]))
    activateSessionWithCommands([])
    render(<AgentZonePanel />)

    const composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })
    fireEvent.change(composer, { target: { value: "@nothing" } })
    expect(await screen.findByText(pt("agentZonePanel.noMatchingFiles"))).toBeInTheDocument()
    expect(screen.getByRole("listbox", { name: pt("agentZonePanel.fileSuggestions") })).not
      .toHaveAttribute("aria-busy")
  })

  it("removes explicit attachments individually without disturbing the others", async () => {
    vi.spyOn(workspaceMentionIndex, "load").mockResolvedValue(workspaceIndexSnapshot([
      { relativePath: "src/a.ts", canonicalPath: "/workspace/src/a.ts" },
      { relativePath: "src/b.ts", canonicalPath: "/workspace/src/b.ts" },
    ]))
    activateSessionWithCommands([])
    render(<AgentZonePanel />)

    const composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })
    fireEvent.change(composer, { target: { value: "@a" } })
    fireEvent.click(await screen.findByRole("option", { name: "Attach src/a.ts" }))
    fireEvent.change(composer, { target: { value: "@b" } })
    fireEvent.click(await screen.findByRole("option", { name: "Attach src/b.ts" }))

    fireEvent.click(screen.getByRole("button", {
      name: pt("agentZonePanel.removeFileContext", { fileName: "src/a.ts" }),
    }))
    expect(screen.queryByText("src/a.ts")).not.toBeInTheDocument()
    expect(screen.getByText("src/b.ts")).toBeInTheDocument()
  })

  it("partitions trusted Codex skills, replaces one chip, and sends the exact skill-only raw prefix", async () => {
    const sendPrompt = vi.fn(async () => "end_turn" as const)
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    useAgentStore.setState({
      activeSessionId: "s-1",
      sendPrompt,
      sessions: new Map([["s-1", session({
        agentId: "codex",
        tone: "done",
        availableCommands: [
          { name: "fix", description: "Fix" },
          { name: "$review", description: "Review" },
          { name: "$deploy", description: "Deploy" },
        ],
      })]]),
    })
    render(<AgentZonePanel />)

    const composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })
    fireEvent.change(composer, { target: { value: "/" } })
    expect(screen.getByRole("option", { name: "/fix Fix" })).toBeInTheDocument()
    expect(screen.queryByRole("option", { name: /\$review/ })).not.toBeInTheDocument()

    fireEvent.change(composer, { target: { value: "$rev" } })
    fireEvent.click(screen.getByRole("option", { name: /\$review skill: Review/ }))
    expect(screen.getByRole("listitem", { name: "Selected skill: $review" })).toBeInTheDocument()

    fireEvent.change(composer, { target: { value: "/fix $dep" } })
    fireEvent.click(screen.getByRole("option", { name: /\$deploy skill: Deploy/ }))
    expect(composer).toHaveValue("")
    expect(screen.queryByRole("listitem", { name: "Selected skill: $review" })).not.toBeInTheDocument()
    expect(screen.getByRole("listitem", { name: "Selected skill: $deploy" })).toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: pt("agentZonePanel.sendAriaLabel") }))
    await waitFor(() => expect(sendPrompt).toHaveBeenCalledWith("/workspace", [
      { type: "text", text: "/$deploy" },
    ]))
    expect(screen.queryByRole("list", { name: pt("agentZonePanel.composerIntent") }))
      .not.toBeInTheDocument()
  })

  it("clears a selected skill and explicit attachments after successful send", async () => {
    const sendPrompt = vi.fn(async () => "end_turn" as const)
    vi.spyOn(workspaceMentionIndex, "load").mockResolvedValue(workspaceIndexSnapshot([
      { relativePath: "docs/spec.md", canonicalPath: "/workspace/docs/spec.md" },
    ]))
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    useAgentStore.setState({
      activeSessionId: "s-1",
      sendPrompt,
      sessions: new Map([["s-1", session({
        agentId: "codex",
        tone: "done",
        availableCommands: [{ name: "$review", description: "Review" }],
      })]]),
    })
    render(<AgentZonePanel />)
    const composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })

    fireEvent.change(composer, { target: { value: "$rev" } })
    fireEvent.click(screen.getByRole("option", { name: /\$review skill: Review/ }))
    fireEvent.change(composer, { target: { value: "@spec" } })
    fireEvent.click(await screen.findByRole("option", { name: "Attach docs/spec.md" }))
    fireEvent.change(composer, { target: { value: "Inspect" } })
    fireEvent.click(screen.getByRole("button", { name: pt("agentZonePanel.sendAriaLabel") }))

    await waitFor(() => expect(sendPrompt).toHaveBeenCalledWith("/workspace", [
      { type: "text", text: "/$review Inspect" },
      { type: "resource_link", uri: "file:///workspace/docs/spec.md", name: "docs/spec.md" },
    ]))
    expect(composer).toHaveValue("")
    expect(screen.queryByRole("list", { name: pt("agentZonePanel.composerIntent") }))
      .not.toBeInTheDocument()
  })

  it("clears selected skill and explicit attachment intent on cancel and session switches", async () => {
    const cancel = vi.fn(async () => true)
    vi.spyOn(workspaceMentionIndex, "load").mockResolvedValue(workspaceIndexSnapshot([
      { relativePath: "docs/spec.md", canonicalPath: "/workspace/docs/spec.md" },
    ]))
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    useAgentStore.setState({
      activeSessionId: "s-1",
      cancel,
      sessions: new Map([
        ["s-1", session({
          agentId: "codex",
          tone: "done",
          availableCommands: [{ name: "$review", description: "Review" }],
        })],
        ["s-2", session({ title: "Second", agentId: "codex", tone: "done" })],
      ]),
    })
    render(<AgentZonePanel />)

    async function selectIntent() {
      const composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })
      fireEvent.change(composer, { target: { value: "$rev" } })
      fireEvent.click(screen.getByRole("option", { name: /\$review skill: Review/ }))
      fireEvent.change(composer, { target: { value: "@spec" } })
      fireEvent.click(await screen.findByRole("option", { name: "Attach docs/spec.md" }))
      expect(screen.getByRole("list", { name: pt("agentZonePanel.composerIntent") }))
        .toBeInTheDocument()
    }

    await selectIntent()
    act(() => {
      const state = useAgentStore.getState()
      const sessions = new Map(state.sessions)
      sessions.set("s-1", { ...sessions.get("s-1")!, tone: "run", running: true })
      useAgentStore.setState({ sessions })
    })
    fireEvent.click(screen.getByRole("button", { name: pt("agentZonePanel.cancelAriaLabel") }))
    expect(cancel).toHaveBeenCalledWith("s-1")
    expect(screen.queryByRole("list", { name: pt("agentZonePanel.composerIntent") }))
      .not.toBeInTheDocument()

    act(() => {
      const state = useAgentStore.getState()
      const sessions = new Map(state.sessions)
      sessions.set("s-1", { ...sessions.get("s-1")!, tone: "done", running: false })
      useAgentStore.setState({ sessions })
    })
    await selectIntent()
    act(() => useAgentStore.setState({ activeSessionId: "s-2" }))
    expect(screen.queryByRole("list", { name: pt("agentZonePanel.composerIntent") }))
      .not.toBeInTheDocument()
    act(() => useAgentStore.setState({ activeSessionId: "s-1" }))
    expect(screen.queryByRole("list", { name: pt("agentZonePanel.composerIntent") }))
      .not.toBeInTheDocument()
  })

  it("fails closed for custom agents and clears a stale selected skill immediately", async () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([["s-1", session({
        agentId: "custom",
        tone: "done",
        availableCommands: [{ name: "$review", description: "Review" }],
      })]]),
    })
    const { rerender } = render(<AgentZonePanel />)
    const composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })
    fireEvent.change(composer, { target: { value: "$" } })
    expect(screen.getByText(pt("agentZonePanel.skillsUnsupported"))).toBeInTheDocument()
    expect(screen.queryByRole("option")).not.toBeInTheDocument()

    act(() => useAgentStore.setState({
      sessions: new Map([["s-1", session({
        agentId: "codex",
        tone: "done",
        availableCommands: [{ name: "$review", description: "Review" }],
      })]]),
    }))
    rerender(<AgentZonePanel />)
    fireEvent.change(composer, { target: { value: "$rev" } })
    fireEvent.click(screen.getByRole("option", { name: /\$review skill: Review/ }))
    expect(screen.getByRole("listitem", { name: "Selected skill: $review" })).toBeInTheDocument()

    act(() => useAgentStore.getState().setAvailableCommands("s-1", []))
    await waitFor(() => {
      expect(screen.queryByRole("listitem", { name: "Selected skill: $review" })).not.toBeInTheDocument()
    })
    expect(screen.getByRole("button", { name: pt("agentZonePanel.sendAriaLabel") })).toBeDisabled()
  })

  it("derives compact shadcn Model/Effort menus only from exact select categories and renders groups", async () => {
    const setSessionConfigOption = vi.fn(async () => [])
    const longModelLabel = "Reasoning model with an intentionally long descriptive label"
    useAgentStore.setState({
      activeSessionId: "s-1",
      setSessionConfigOption,
      sessions: new Map([["s-1", session({
        model: "legacy-header-model",
        tone: "idle",
        configOptions: [
          {
            id: "adapter-model-id",
            name: "Runtime model",
            category: "model",
            type: "select",
            currentValue: "reasoning",
            options: [{
              group: "reasoning",
              name: "Reasoning models",
              options: [{ value: "reasoning", name: longModelLabel }],
            }],
          },
          {
            id: "adapter-effort-id",
            name: "Runtime effort",
            category: "thought_level",
            type: "select",
            currentValue: "high",
            options: [{ value: "low", name: "Low" }, { value: "high", name: "High" }],
          },
          {
            id: "boolean-model-id",
            name: "Boolean model",
            category: "model",
            type: "boolean",
            currentValue: true,
          },
          {
            id: "similar-category-id",
            name: "Similar category",
            category: "model_variant",
            type: "select",
            currentValue: "ignored",
            options: [{ value: "ignored", name: "Ignored" }],
          },
        ],
        configRevision: 1,
        configRequest: null,
        configError: null,
      })]]),
    })

    render(<AgentZonePanel />)

    const model = screen.getByRole("button", { name: pt("agentZonePanel.modelConfigAria") })
    const effort = screen.getByRole("button", { name: pt("agentZonePanel.effortConfigAria") })
    expect(model).toHaveAttribute("data-slot", "popover-trigger")
    expect(effort).toHaveAttribute("data-slot", "popover-trigger")
    expect(model).toHaveAttribute("data-variant", "ghost")
    expect(effort).toHaveAttribute("data-variant", "ghost")
    expect(screen.getByTestId("agent-config-controls")).toHaveAttribute(
      "data-layout",
      "composer-toolbar"
    )
    expect(model).toHaveTextContent(longModelLabel)
    expect(screen.getByTestId("agent-session-header")).not.toHaveTextContent("legacy-header-model")

    fireEvent.click(model)
    expect(await screen.findByText("Reasoning models")).toBeInTheDocument()
    expect(await screen.findByRole("option", { name: longModelLabel })).toHaveAttribute(
      "data-checked",
      "true"
    )

    fireEvent.keyDown(
      screen.getByPlaceholderText(pt("agentZonePanel.configSearch", { name: "Runtime model" })),
      { key: "Escape" }
    )
    await waitFor(() => expect(screen.queryByText("Reasoning models")).not.toBeInTheDocument())
    fireEvent.click(effort)
    fireEvent.click(await screen.findByRole("option", { name: "Low" }))
    expect(setSessionConfigOption).toHaveBeenCalledWith("s-1", "adapter-effort-id", "low")
  })

  it("filters config menu options through the combobox search input", async () => {
    const setSessionConfigOption = vi.fn(async () => [])
    useAgentStore.setState({
      activeSessionId: "s-1",
      setSessionConfigOption,
      sessions: new Map([["s-1", session({
        tone: "idle",
        configOptions: [{
          id: "adapter-model-id",
          name: "Runtime model",
          category: "model",
          type: "select",
          currentValue: "sol",
          options: [
            { value: "sol", name: "gpt-5.6-sol" },
            { value: "luna", name: "gpt-5.6-luna" },
            { value: "opus", name: "claude-opus" },
          ],
        }],
        configRevision: 1,
        configRequest: null,
        configError: null,
      })]]),
    })

    render(<AgentZonePanel />)
    fireEvent.click(screen.getByRole("button", { name: pt("agentZonePanel.modelConfigAria") }))
    const search = await screen.findByPlaceholderText(
      pt("agentZonePanel.configSearch", { name: "Runtime model" })
    )
    fireEvent.change(search, { target: { value: "luna" } })
    await waitFor(() => {
      expect(screen.queryByRole("option", { name: "claude-opus" })).toBeNull()
    })
    fireEvent.click(screen.getByRole("option", { name: "gpt-5.6-luna" }))
    expect(setSessionConfigOption).toHaveBeenCalledWith("s-1", "adapter-model-id", "luna")
    // 選取後選單收合。
    await waitFor(() => {
      expect(screen.queryByRole("option", { name: "gpt-5.6-luna" })).toBeNull()
    })
  })

  it("aggregates tool diff stats into a changes summary above the composer", async () => {
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([["s-1", session({
        tone: "idle",
        transcript: [
          {
            kind: "tool",
            text: "edit\n[diff: src/a.ts]",
            meta: JSON.stringify({
              toolCallId: "t1",
              kind: "edit",
              status: "completed",
              diffs: [{ path: "src/a.ts", added: 5, removed: 2 }],
            }),
          },
          {
            kind: "tool",
            text: "edit\n[diff: src/b.ts]",
            meta: JSON.stringify({
              toolCallId: "t2",
              kind: "edit",
              status: "completed",
              diffs: [
                { path: "src/b.ts", added: 1, removed: 0 },
                { path: "src/a.ts", added: 2, removed: 1 },
              ],
            }),
          },
        ],
      })]]),
    })

    render(<AgentZonePanel />)
    const summary = screen.getByTestId("agent-changes-summary")
    expect(summary).toHaveTextContent(pt("agentZonePanel.changesFiles", { count: 2 }))
    expect(summary).toHaveTextContent("+8")
    expect(summary).toHaveTextContent("−3")

    fireEvent.click(summary)
    // per-file 明細：同檔多次 diff 行數累加。
    expect(await screen.findByText("src/a.ts")).toBeInTheDocument()
    expect(screen.getByText("+7")).toBeInTheDocument()
    expect(screen.getByText("−3", { selector: "li *" })).toBeInTheDocument()
  })

  it("renders no changes summary when the transcript has no diff stats", () => {
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([["s-1", session({
        tone: "idle",
        transcript: [{ kind: "tool", text: "read\nsrc/a.ts", meta: JSON.stringify({ toolCallId: "t1", kind: "read" }) }],
      })]]),
    })
    render(<AgentZonePanel />)
    expect(screen.queryByTestId("agent-changes-summary")).toBeNull()
  })

  // soak 回饋 #1/#5：turn 進行中 config 開放（僅 setter 在途時鎖）。
  it("disables config menu triggers only during a setter request and renders a retryable error", () => {
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([["s-1", session({
        tone: "idle",
        configOptions: [{
          id: "adapter-model-id",
          name: "Runtime model",
          category: "model",
          type: "select",
          currentValue: "fast",
          options: [{ value: "fast", name: "Fast" }],
        }],
        configRequest: { token: 7, configId: "adapter-model-id", value: "fast" },
        configError: null,
      })]]),
    })
    const { rerender } = render(<AgentZonePanel />)

    expect(screen.getByRole("button", { name: pt("agentZonePanel.modelConfigAria") })).toBeDisabled()

    useAgentStore.setState({
      sessions: new Map([["s-1", session({
        tone: "idle",
        configOptions: [{
          id: "adapter-model-id",
          name: "Runtime model",
          category: "model",
          type: "select",
          currentValue: "fast",
          options: [{ value: "fast", name: "Fast" }],
        }],
        configRequest: null,
        configError: "Adapter rejected the model",
      })]]),
    })
    rerender(<AgentZonePanel />)
    expect(screen.getByRole("button", { name: pt("agentZonePanel.modelConfigAria") })).toBeEnabled()
    expect(screen.getByRole("alert")).toHaveTextContent("Adapter rejected the model")

    useAgentStore.setState({
      sessions: new Map([["s-1", session({
        tone: "run",
        running: true,
        configOptions: [{
          id: "adapter-model-id",
          name: "Runtime model",
          category: "model",
          type: "select",
          currentValue: "fast",
          options: [{ value: "fast", name: "Fast" }],
        }],
        configRequest: null,
      })]]),
    })
    rerender(<AgentZonePanel />)
    // turn 進行中仍可切換（pi 隨時可切、下一次 LLM 呼叫生效）。
    expect(screen.getByRole("button", { name: pt("agentZonePanel.modelConfigAria") })).toBeEnabled()
  })

  // soak 回饋 #4：turn 進行中送出鈕仍在（steering）、與停止鈕並列，送出走一般 prompt 流程。
  it("keeps the send button during a running turn and steers the next message", async () => {
    const sendPrompt = vi.fn(async (_cwd: string, _prompt: string | PromptBlock[]) => "end_turn" as const)
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([["s-1", session({ tone: "run", running: true, pendingTurn: true, cwd: "/workspace" })]]),
      sendPrompt,
    })
    render(<AgentZonePanel />)

    expect(screen.getByRole("button", { name: pt("agentZonePanel.cancelAriaLabel") })).toBeInTheDocument()
    const send = screen.getByRole("button", { name: pt("agentZonePanel.sendAriaLabel") })
    const composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })
    fireEvent.change(composer, { target: { value: "steer this way" } })
    fireEvent.click(send)

    await waitFor(() => expect(sendPrompt).toHaveBeenCalledOnce())
    expect(sendPrompt.mock.calls[0][1]).toEqual([{ type: "text", text: "steer this way" }])
  })

  it("uses a compact shadcn shell and expands transcript content width", () => {
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([["s-1", session({
        tone: "done",
        transcript: [
          { who: "you", text: "Please inspect this", streaming: false },
          { who: "agent", text: "I can use the wider thread surface.", streaming: false },
        ],
      })]]),
    })

    render(<AgentZonePanel />)

    const header = screen.getByTestId("agent-session-header")
    expect(header).toHaveAttribute("data-density", "compact")
    expect(within(header).getByText("ACP").closest('[data-slot="badge"]')).not.toBeNull()

    const composer = screen.getByTestId("agent-composer")
    expect(composer).toHaveAttribute("data-slot", "input-group")
    expect(composer).toHaveAttribute("data-layout", "stacked-toolbar")

    const bubbles = screen.getAllByTestId("agent-message-bubble")
    const userBubble = bubbles.find((bubble) => bubble.getAttribute("data-sender") === "you")
    const agentBubble = bubbles.find((bubble) => bubble.getAttribute("data-sender") === "agent")
    // P2（Atelier）：user 玻璃氣泡 76%；agent 改頭像＋全寬內容流（不再限寬）。
    expect(userBubble).toHaveStyle({ maxWidth: "76%" })
    expect(agentBubble).not.toHaveStyle({ maxWidth: "94%" })
    expect(agentBubble).toHaveStyle({ flex: "1" })
  })

  it("focuses the active composer when continueSession requests focus for that session", async () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([["s-1", session({ tone: "idle" })]]),
    })
    const outside = document.createElement("button")
    document.body.appendChild(outside)
    render(<AgentZonePanel />)
    const composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })
    outside.focus()
    expect(composer).not.toHaveFocus()

    await act(async () => {
      await useAgentStore.getState().continueSession("s-1")
    })

    expect(composer).toHaveFocus()
    outside.remove()
  })

  it("sends the active editor file as a resource_link block", async () => {
    const connection = connectedAgent()
    focusWorkspaceFile("/workspace/src/main.ts")

    render(<AgentZonePanel />)

    expect(screen.getByText("main.ts")).toBeInTheDocument()
    const composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })
    fireEvent.change(composer, { target: { value: "Explain this file" } })
    fireEvent.keyDown(composer, { key: "Enter", metaKey: true })

    await waitFor(() => {
      expect(connection.prompt).toHaveBeenCalledWith("s-1", [
        { type: "text", text: "Explain this file" },
        { type: "resource_link", uri: "file:///workspace/src/main.ts", name: "main.ts" },
      ])
    })
  })

  it("sanitizes an extended workspace attachment tooltip but keeps the raw resource path", async () => {
    const connection = connectedAgent()
    const workspace = String.raw`\\?\C:\工作區`
    const path = String.raw`\\?\C:\工作區\src\main.ts`
    useWorkspaceStore.setState({
      workspacePath: workspace,
      groups: [{
        tabs: [{ path, name: "main.ts", dirty: false, externallyModified: false }],
        activePath: path,
      }],
      activeGroupIndex: 0,
    })
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([["s-1", session({ cwd: workspace, tone: "done" })]]),
    })

    render(<AgentZonePanel />)

    const chip = screen.getByText("main.ts").closest("[role='listitem']")
    expect(chip).toHaveAttribute("title", String.raw`C:\工作區\src\main.ts`)
    expect(chip).not.toHaveAttribute("title", path)

    const composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })
    fireEvent.change(composer, { target: { value: "Explain this file" } })
    fireEvent.keyDown(composer, { key: "Enter", metaKey: true })

    await waitFor(() => expect(connection.prompt).toHaveBeenCalledWith("s-1", [
      { type: "text", text: "Explain this file" },
      { type: "resource_link", uri: "file:///C:/%E5%B7%A5%E4%BD%9C%E5%8D%80/src/main.ts", name: "main.ts" },
    ]))
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
    const composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })
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

    fireEvent.click(
      screen.getByRole("button", {
        name: pt("agentZonePanel.removeFileContext", { fileName: "main.ts" }),
      })
    )
    expect(screen.queryByText("main.ts")).not.toBeInTheDocument()

    const composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })
    fireEvent.change(composer, { target: { value: "Explain without file" } })
    fireEvent.keyDown(composer, { key: "Enter", metaKey: true })

    await waitFor(() => {
      expect(connection.prompt).toHaveBeenCalledWith("s-1", [
        { type: "text", text: "Explain without file" },
      ])
    })
  })

  it("shows workspace guidance and no session when no absolute cwd is available", () => {
    render(<AgentZonePanel />)

    expect(screen.getByText(pt("agentZonePanel.noWorkspaceTitle"))).toBeInTheDocument()
    expect(screen.getByText(pt("agentZonePanel.emptyTitle"))).toBeInTheDocument()
  })

  it("refuses to spawn with a relative cwd and keeps the workspace guidance visible", async () => {
    const prompt = vi.fn(async () => "end_turn" as const)
    const newSession = vi.fn(async () => ({ sessionId: "s-1", startupInfo: null }))
    const connection: AgentConnection = {
      newSession,
      loadSession: vi.fn(),
      listSessions: vi.fn(async () => []),
      prompt,
      cancel: vi.fn(),
    }
    useAgentStore.getState().setConnection(connection)
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([["s-1", session({ tone: "done", cwd: null })]]),
    })
    useWorkspaceStore.setState({ workspacePath: "." })

    render(<AgentZonePanel />)

    expect(screen.getByText(pt("agentZonePanel.noWorkspaceTitle"))).toBeInTheDocument()

    const composer = screen.getByRole("combobox", { name: pt("agentZonePanel.composerAriaLabel") })
    fireEvent.change(composer, { target: { value: "Hi" } })
    fireEvent.keyDown(composer, { key: "Enter", metaKey: true })

    await Promise.resolve()
    expect(newSession).not.toHaveBeenCalled()
    expect(prompt).not.toHaveBeenCalled()
    expect(composer).toHaveValue("Hi")
  })

  it.each([
    { agentId: "pi", colorVar: "var(--agent-pi)", bgVar: "var(--agent-pi-soft)" },
    { agentId: "claude", colorVar: "var(--agent-claude)", bgVar: "var(--agent-claude-soft)" },
    { agentId: "codex", colorVar: "var(--agent-codex)", bgVar: "var(--agent-codex-soft)" },
  ] as const)(
    "resolves agentId=$agentId to the $colorVar brand token regardless of agentLabel",
    ({ agentId, colorVar, bgVar }) => {
      useAgentStore.setState({
        activeSessionId: "s-1",
        sessions: new Map([["s-1", session({ agentId, agentLabel: "Some Random" })]]),
      })

      render(<AgentZonePanel />)

      expect(screen.getByTestId("agent-avatar")).toHaveStyle({ background: colorVar })
      expect(screen.getByTestId("agent-session-header").getAttribute("style")).toContain(bgVar)
    }
  )

  // P5 雙 runtime badge：判斷來源是 initialize.agentInfo.name（live 事實），
  // 尚未續聊的 restored session 沒有 name → 不顯示。
  it.each([
    { agentName: "yuzora-pi-acp", labelKey: "agentZonePanel.runtimeBuiltin" },
    { agentName: "pi-acp", labelKey: "agentZonePanel.runtimeCommunity" },
  ] as const)("shows the runtime badge for pi sessions reporting $agentName", ({ agentName, labelKey }) => {
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([["s-1", session({ agentId: "pi", agentName })]]),
    })

    render(<AgentZonePanel />)

    expect(screen.getByTestId("agent-runtime-badge")).toHaveTextContent(pt(labelKey))
  })

  it("hides the runtime badge without an adapter name or for non-pi sessions", () => {
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([["s-1", session({ agentId: "pi" })]]),
    })
    render(<AgentZonePanel />)
    expect(screen.queryByTestId("agent-runtime-badge")).toBeNull()

    cleanup()
    useAgentStore.setState({
      activeSessionId: "s-2",
      sessions: new Map([["s-2", session({ agentId: "claude", agentName: "pi-acp" })]]),
    })
    render(<AgentZonePanel />)
    expect(screen.queryByTestId("agent-runtime-badge")).toBeNull()
  })

  it("falls back to the neutral --agent-custom token when agentId is undefined, regardless of agentLabel", () => {
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([["s-1", session({ agentId: undefined, agentLabel: "Some Random" })]]),
    })

    render(<AgentZonePanel />)

    expect(screen.getByTestId("agent-avatar")).toHaveStyle({ background: "var(--agent-custom)" })
    expect(screen.getByTestId("agent-session-header").getAttribute("style")).toContain(
      "var(--agent-custom-soft)"
    )
  })

  it("falls back to the neutral --agent-custom token when agentId is the custom preset", () => {
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([["s-1", session({ agentId: "custom", agentLabel: "My Custom Agent" })]]),
    })

    render(<AgentZonePanel />)

    expect(screen.getByTestId("agent-avatar")).toHaveStyle({ background: "var(--agent-custom)" })
  })

  it("does not render the usage chip when session.usage is undefined", () => {
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([["s-1", session()]]),
    })

    render(<AgentZonePanel />)

    expect(screen.queryByTestId("agent-usage-chip")).not.toBeInTheDocument()
  })

  it.each([
    { used: 18234, size: 200000, expected: "18.2k / 200k" },
    { used: 999, size: 1000, expected: "999 / 1k" },
    { used: 1500000, size: 2000000, expected: "1.5M / 2M" },
    { used: 999_950, size: 2_000_000, expected: "1M / 2M" },
    { used: 999_949, size: 2_000_000, expected: "999.9k / 2M" },
  ])("formats the usage chip as $expected", ({ used, size, expected }) => {
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([["s-1", session({ usage: { used, size } })]]),
    })

    render(<AgentZonePanel />)

    expect(screen.getByTestId("agent-usage-chip")).toHaveTextContent(expected)
  })

  it("does not render the usage chip when usage.size is 0", () => {
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([["s-1", session({ usage: { used: 5, size: 0 } })]]),
    })

    render(<AgentZonePanel />)

    expect(screen.queryByTestId("agent-usage-chip")).not.toBeInTheDocument()
  })

  it("still renders a complete cost chip when usage.size is 0", () => {
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([["s-1", session({
        usage: { used: 0, size: 0, cost: { amount: 0.000001, currency: "EUR" } },
      })]]),
    })

    render(<AgentZonePanel />)

    expect(screen.queryByTestId("agent-usage-chip")).not.toBeInTheDocument()
    const cost = screen.getByTestId("agent-cost-chip")
    expect(cost).toHaveTextContent("0.000001 EUR")
    expect(cost.getAttribute("style")).not.toMatch(/max-width|overflow|text-overflow/)
  })

  it.each([
    { used: 50, size: 100, level: "normal" },
    { used: 85, size: 100, level: "warn" },
    { used: 97, size: 100, level: "danger" },
  ])("switches the usage ring level to $level at $used%", ({ used, size, level }) => {
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([["s-1", session({ usage: { used, size } })]]),
    })

    render(<AgentZonePanel />)

    expect(screen.getByTestId("agent-usage-ring")).toHaveAttribute("data-usage-level", level)
  })

  it("shows exact structured usage and zero/EUR cost in the named context popover", () => {
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([
        [
          "s-1",
          session({ usage: { used: 18234, size: 200000, cost: { amount: 0, currency: "EUR" } } }),
        ],
      ]),
    })

    render(<AgentZonePanel />)

    const context = screen.getByTestId("agent-usage-chip")
    expect(context.tagName).toBe("BUTTON")
    expect(context).not.toHaveAttribute("title")
    expect(context).toHaveAttribute("aria-controls", "agent-usage-popover")
    fireEvent.keyDown(context, { key: "Enter" })
    const popover = screen.getByRole("dialog", { name: pt("agentZonePanel.usagePopoverAria") })
    expect(screen.getByTestId("agent-usage-used")).toHaveTextContent("18,234")
    expect(screen.getByTestId("agent-usage-size")).toHaveTextContent("200,000")
    expect(screen.getByTestId("agent-usage-remaining")).toHaveTextContent("181,766")
    expect(screen.getByTestId("agent-usage-percent")).toHaveTextContent("9.117%")
    expect(screen.getByTestId("agent-usage-cost")).toHaveTextContent("0 EUR")
    expect(popover).toHaveTextContent(pt("agentZonePanel.usageUsed"))
    expect(popover).toHaveTextContent(pt("agentZonePanel.usageRemaining"))
    expect(popover.getAttribute("style")).toContain("100cqw")
    const cost = screen.getByTestId("agent-cost-chip")
    expect(cost).toHaveTextContent("0 EUR")
    expect(cost).not.toHaveTextContent("$")
    expect(cost.getAttribute("style")).not.toMatch(/max-width|overflow|text-overflow/)
  })

  it.each([
    { amount: 0, currency: "EUR", expected: "0 EUR" },
    { amount: 0.000001, currency: "EUR", expected: "0.000001 EUR" },
    { amount: 1e-24, currency: "EUR", expected: "1e-24 EUR" },
    { amount: 0.42, currency: "JPY", expected: "0.42 JPY" },
  ])("keeps zero/micro cost exact in both chip and popover ($expected)", ({ amount, currency, expected }) => {
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([
        ["s-1", session({ usage: { used: 10, size: 100, cost: { amount, currency } } })],
      ]),
    })

    render(<AgentZonePanel />)

    expect(screen.getByTestId("agent-cost-chip")).toHaveTextContent(expected)
    expect(screen.getByTestId("agent-cost-chip")).not.toHaveTextContent("$")
    expect(screen.getByTestId("agent-cost-chip").getAttribute("style"))
      .not.toMatch(/max-width|overflow|text-overflow/)
    fireEvent.keyDown(screen.getByTestId("agent-usage-chip"), { key: "Enter" })
    expect(screen.getByTestId("agent-usage-cost")).toHaveTextContent(expected)
  })

  it("opens context details with Space and returns focus after Escape or outside click", async () => {
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([["s-1", session({ usage: { used: 10, size: 100 } })]]),
    })
    const outside = document.createElement("button")
    document.body.appendChild(outside)
    render(<AgentZonePanel />)
    const context = screen.getByTestId("agent-usage-chip")

    context.focus()
    fireEvent.keyDown(context, { key: " " })
    expect(screen.getByTestId("agent-usage-popover")).toBeInTheDocument()
    expect(screen.queryByTestId("agent-usage-cost")).not.toBeInTheDocument()
    fireEvent.keyDown(document, { key: "Escape" })
    expect(screen.queryByTestId("agent-usage-popover")).not.toBeInTheDocument()
    expect(context).toHaveFocus()

    fireEvent.keyDown(context, { key: " " })
    outside.focus()
    fireEvent.mouseDown(outside)
    expect(screen.queryByTestId("agent-usage-popover")).not.toBeInTheDocument()
    await waitFor(() => expect(context).toHaveFocus())
    outside.remove()
  })

  it("does not render the info chip when infoBanner is null", () => {
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([["s-1", session({ infoBanner: null })]]),
    })

    render(<AgentZonePanel />)

    expect(screen.queryByTestId("agent-info-chip")).not.toBeInTheDocument()
    expect(screen.queryByRole("note")).not.toBeInTheDocument()
  })

  it("shows the info chip's stripped first line, opens/closes the popover, and no longer renders the old full-width banner", () => {
    const infoBanner = "## **pi v0.0.31**\n\nStartup details go here.\nSecond line of detail."
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([["s-1", session({ infoBanner })]]),
    })

    render(<AgentZonePanel />)

    // 舊版整寬 InfoBanner 不再渲染
    expect(screen.queryByRole("note")).not.toBeInTheDocument()

    const chip = screen.getByTestId("agent-info-chip")
    expect(chip).toHaveTextContent("pi v0.0.31")
    expect(chip.textContent).not.toContain("#")
    expect(chip.textContent).not.toContain("**")

    expect(screen.queryByTestId("agent-info-popover")).not.toBeInTheDocument()
    fireEvent.click(chip)
    expect(screen.getByTestId("agent-info-popover")).toHaveTextContent("Startup details go here.")
    expect(screen.getByTestId("agent-info-popover")).toHaveTextContent("Second line of detail.")

    fireEvent.keyDown(document, { key: "Escape" })
    expect(screen.queryByTestId("agent-info-popover")).not.toBeInTheDocument()

    fireEvent.click(chip)
    expect(screen.getByTestId("agent-info-popover")).toBeInTheDocument()
    fireEvent.mouseDown(document.body)
    expect(screen.queryByTestId("agent-info-popover")).not.toBeInTheDocument()

    fireEvent.click(chip)
    expect(screen.getByTestId("agent-info-popover")).toBeInTheDocument()
    fireEvent.click(chip)
    expect(screen.queryByTestId("agent-info-popover")).not.toBeInTheDocument()
  })

  it("strips a leading non-content markdown line (e.g. a bare separator) to find the first real info line", () => {
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([["s-1", session({ infoBanner: "---\nStartup v1" })]]),
    })

    render(<AgentZonePanel />)

    expect(screen.getByTestId("agent-info-chip")).toHaveTextContent("Startup v1")
  })

  it("does not render the info chip when infoBanner strips down to nothing (blank/marker-only lines)", () => {
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([["s-1", session({ infoBanner: "\n  \n" })]]),
    })

    render(<AgentZonePanel />)

    expect(screen.queryByTestId("agent-info-chip")).not.toBeInTheDocument()
  })

  it("closes the info popover when the underlying session's infoBanner text changes", () => {
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([["s-1", session({ infoBanner: "First session info" })]]),
    })
    const { rerender } = render(<AgentZonePanel />)

    const chip = screen.getByTestId("agent-info-chip")
    fireEvent.click(chip)
    expect(screen.getByTestId("agent-info-popover")).toBeInTheDocument()

    useAgentStore.setState({
      sessions: new Map([["s-1", session({ infoBanner: "Second session info" })]]),
    })
    rerender(<AgentZonePanel />)

    expect(screen.queryByTestId("agent-info-popover")).not.toBeInTheDocument()
  })

  it("dedupes a transcript's first agent entry when it matches infoBanner, but keeps it otherwise", () => {
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([
        [
          "s-1",
          session({
            infoBanner: "  Startup notice  ",
            transcript: [
              { who: "agent", text: "Startup notice", streaming: false },
              { who: "you", text: "hello", streaming: false },
            ],
          }),
        ],
      ]),
    })

    render(<AgentZonePanel />)

    // "Startup notice" 只該出現在 info chip pill 裡（=firstInfoLine 本身），transcript
    // 首則已被 dedupeInfoBanner 濾除，故不會有第二個節點。
    expect(screen.getAllByText("Startup notice")).toHaveLength(1)
    expect(screen.getAllByText("Startup notice")[0]).toBe(screen.getByTestId("agent-info-chip"))
    expect(screen.getByText("hello")).toBeInTheDocument()
  })

  it("keeps the first transcript entry when its text differs from infoBanner", () => {
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([
        [
          "s-1",
          session({
            infoBanner: "Startup notice",
            transcript: [
              { who: "agent", text: "Different first message", streaming: false },
            ],
          }),
        ],
      ]),
    })

    render(<AgentZonePanel />)

    expect(screen.getByText("Different first message")).toBeInTheDocument()
  })

  it("keeps a non-first transcript entry even if its text matches infoBanner", () => {
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([
        [
          "s-1",
          session({
            infoBanner: "Startup notice",
            transcript: [
              { who: "you", text: "hi", streaming: false },
              { who: "agent", text: "Startup notice", streaming: false },
            ],
          }),
        ],
      ]),
    })

    render(<AgentZonePanel />)

    // 非首則相同文字不被 dedupe：info chip 與 transcript 各出現一次，共兩個節點。
    expect(screen.getAllByText("Startup notice")).toHaveLength(2)
  })

  it("hides a null-cwd active session once the workspace has an absolute path (P10-B)", () => {
    // active session 沒有 cwd（null），但目前 workspace 已是絕對路徑：sendPrompt 會
    // 透過 selectActiveSessionForCwd 視為「不屬於這個 workspace」而開新 session，
    // 面板不該仍顯示這個 null-cwd session（否則顯示與送出行為不一致）。
    useAgentStore.setState({
      activeSessionId: "s-1",
      sessions: new Map([["s-1", session({ tone: "done", cwd: null })]]),
    })
    useWorkspaceStore.setState({ workspacePath: "/ws-b" })

    render(<AgentZonePanel />)

    expect(screen.getByText(pt("agentZonePanel.emptyTitle"))).toBeInTheDocument()
    expect(screen.queryByText(session().title)).not.toBeInTheDocument()
  })
})
