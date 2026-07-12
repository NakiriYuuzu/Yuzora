import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import { AgentNavContent } from "@/app/workbench/AgentNavContent"
import { commandFor } from "@/app/workbench/contextMenuDefs"
import { AGENT_SETTINGS_STORAGE_KEY } from "@/app/workbench/settingsStorage"
import type { SessionState } from "@/state/agentStore"
import { useAgentStore } from "@/state/agentStore"
import { useContextMenuStore } from "@/state/contextMenuStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

const originalAgentActions = {
  newSession: useAgentStore.getState().newSession,
  selectSession: useAgentStore.getState().selectSession,
  continueSession: useAgentStore.getState().continueSession,
}

// The Bun-hosted test runtime injects an empty `localStorage` global with no
// Storage methods (see agentStore.login.test.ts). Install a minimal in-memory
// Storage so agent-settings persistence is exercised for real.
function installLocalStorage(): void {
  const store = new Map<string, string>()
  const mock = {
    getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
    setItem: (k: string, v: string) => void store.set(k, String(v)),
    removeItem: (k: string) => void store.delete(k),
    clear: () => store.clear(),
    key: (i: number) => [...store.keys()][i] ?? null,
    get length() {
      return store.size
    },
  }
  Object.defineProperty(globalThis, "localStorage", {
    value: mock,
    configurable: true,
    writable: true,
  })
}

beforeEach(() => {
  installLocalStorage()
  localStorage.clear()
})

afterEach(() => {
  cleanup()
  useAgentStore.setState({
    sessions: new Map(),
    pendingPermissions: new Map(),
    activeSessionId: null,
    connectionState: "idle",
    connection: null,
    renamingSessionId: null,
    confirmRemoveRequest: null,
    ...originalAgentActions,
  })
  useContextMenuStore.setState({ request: null, x: 0, y: 0, availabilityRevision: 0 })
  useWorkspaceStore.setState({ workspacePath: null })
  vi.clearAllMocks()
})

function session(overrides: Partial<SessionState> = {}): SessionState {
  return {
    title: "修復 git.rs error path",
    agentId: "codex",
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
  it("lists only sessions whose cwd matches the current workspace", () => {
    useWorkspaceStore.setState({ workspacePath: "/ws-a" })
    useAgentStore.setState({
      sessions: new Map([
        ["a1", session({ title: "A", cwd: "/ws-a" })],
        ["b1", session({ title: "B", cwd: "/ws-b" })],
      ]),
    })

    render(<AgentNavContent />)

    expect(screen.queryByText("A")).toBeInTheDocument()
    expect(screen.queryByText("B")).not.toBeInTheDocument()
  })

  it("lists sessions with tone dot, title, and agent/model metadata", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    useAgentStore.setState({
      activeSessionId: "s-run",
      sessions: new Map([
        ["s-run", session({ title: "修復 git.rs error path", tone: "run" })],
        [
          "s-fail",
          session({
            title: "整理 README",
            agentId: "claude",
            agentLabel: "Claude",
            model: "sonnet-4",
            tone: "fail",
          }),
        ],
      ]),
    })

    render(<AgentNavContent />)

    expect(screen.getByText("修復 git.rs error path")).toBeInTheDocument()
    expect(screen.getByText("Codex · Codex / codex-1")).toBeInTheDocument()
    expect(screen.getByText("整理 README")).toBeInTheDocument()
    expect(screen.getByText("Claude · Claude / sonnet-4")).toBeInTheDocument()
    expect(screen.getByTestId("agent-session-tone-s-run")).toHaveStyle({
      background: "var(--yz-run)",
    })
    expect(screen.getByTestId("agent-session-tone-s-fail")).toHaveStyle({
      background: "#e23b54",
    })
    expect(screen.getByRole("button", { name: /修復 git\.rs error path/ })).toHaveAttribute(
      "aria-current",
      "page"
    )
  })

  it("never renders the literal 'undefined' when a session has no agentId", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    const sessionWithoutAgentId: SessionState = {
      title: "無 agentId 的舊 session",
      agentLabel: "Agent",
      model: null,
      tone: "idle",
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
    }
    useAgentStore.setState({
      activeSessionId: "s-no-agent-id",
      sessions: new Map([["s-no-agent-id", sessionWithoutAgentId]]),
    })

    render(<AgentNavContent />)

    expect(screen.getByText("無 agentId 的舊 session")).toBeInTheDocument()
    expect(screen.queryByText(/undefined/)).not.toBeInTheDocument()
    // agentId 未知時顯示名以 agentLabel 為準（agentDisplayName 語意），不再 fallback
    // 成 preset 顯示名 "Pi"。
    expect(screen.getByText("Agent · Agent")).toBeInTheDocument()
  })

  it("prefixes the row metadata with the agent preset label", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    useAgentStore.setState({
      activeSessionId: "s-codex",
      sessions: new Map([
        ["s-codex", session({ agentId: "codex", agentLabel: "gpt-5", model: null })],
      ]),
    })

    render(<AgentNavContent />)

    expect(screen.getByText("Codex · gpt-5")).toBeInTheDocument()
  })

  it("clicking a session selects it", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
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

  it("Phase 4: clicking a session row dispatches continueSession, not just selectSession", () => {
    const continueSession = vi.fn(async () => undefined)
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    useAgentStore.setState({
      continueSession,
      sessions: new Map([["s-restored", session({ title: "Restored chat", restored: true })]]),
    })

    render(<AgentNavContent />)

    fireEvent.click(screen.getByRole("button", { name: /Restored chat/ }))

    expect(continueSession).toHaveBeenCalledWith("s-restored")
  })

  it("right-clicking a SessionRow opens an agentSession request for that exact row only", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    useAgentStore.setState({
      activeSessionId: "active",
      sessions: new Map([
        ["clicked", session({ title: "Clicked chat" })],
        ["active", session({ title: "Active chat" })],
      ]),
    })
    const { container } = render(<AgentNavContent />)

    fireEvent.contextMenu(container.firstElementChild!, { clientX: 3, clientY: 4 })
    expect(useContextMenuStore.getState().request).toBeNull()

    fireEvent.contextMenu(screen.getByRole("button", { name: /Clicked chat/ }), {
      clientX: 17,
      clientY: 23,
    })
    expect(useContextMenuStore.getState()).toMatchObject({
      request: { kind: "agentSession", sessionId: "clicked" },
      x: 17,
      y: 23,
    })
  })

  it("inline rename auto-focuses/selects all, trims on Enter, and clears the rename channel", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    useAgentStore.setState({
      renamingSessionId: "s-1",
      sessions: new Map([["s-1", session({
        title: "Agent title",
        agentTitle: "Agent title",
        sessionAlias: null,
      })]]),
    })
    render(<AgentNavContent />)

    const input = screen.getByTestId("agent-session-rename-s-1") as HTMLInputElement
    expect(input).toHaveFocus()
    expect(input.selectionStart).toBe(0)
    expect(input.selectionEnd).toBe("Agent title".length)
    fireEvent.change(input, { target: { value: "  Local alias  " } })
    fireEvent.keyDown(input, { key: "Enter" })

    expect(useAgentStore.getState().sessions.get("s-1")).toMatchObject({
      sessionAlias: "Local alias",
      title: "Local alias",
    })
    expect(useAgentStore.getState().renamingSessionId).toBeNull()
  })

  it("inline rename Escape cancels without saving; blur confirms and empty input clears alias", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    useAgentStore.setState({
      renamingSessionId: "s-1",
      sessions: new Map([["s-1", session({
        title: "Old alias",
        agentTitle: "Agent title",
        sessionAlias: "Old alias",
      })]]),
    })
    const { rerender } = render(<AgentNavContent />)

    let input = screen.getByTestId("agent-session-rename-s-1") as HTMLInputElement
    fireEvent.change(input, { target: { value: "Should not save" } })
    fireEvent.keyDown(input, { key: "Escape" })
    expect(useAgentStore.getState().sessions.get("s-1")?.sessionAlias).toBe("Old alias")

    useAgentStore.getState().beginRenameSession("s-1")
    rerender(<AgentNavContent />)
    input = screen.getByTestId("agent-session-rename-s-1") as HTMLInputElement
    fireEvent.change(input, { target: { value: "   " } })
    fireEvent.blur(input)
    expect(useAgentStore.getState().sessions.get("s-1")).toMatchObject({
      sessionAlias: null,
      title: "Agent title",
    })
  })

  it("Remove Session modal names the resolved clicked title; cancel preserves and danger confirm removes", async () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    useAgentStore.setState({
      activeSessionId: "other",
      sessions: new Map([
        ["clicked", session({
          title: "Local alias",
          sessionAlias: "Local alias",
          agentTitle: "Agent title",
        })],
        ["other", session({ title: "Other chat" })],
      ]),
    })
    render(<AgentNavContent />)
    const request = { kind: "agentSession" as const, sessionId: "clicked" }
    const remove = commandFor(request, "cmRemoveSession")
    if (!remove) throw new Error("missing cmRemoveSession")

    const cancelled = remove.executor(request)
    expect(await screen.findByText(/Local alias/)).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    await expect(cancelled).resolves.toBe("cancelled")
    expect(useAgentStore.getState().sessions.has("clicked")).toBe(true)

    const confirmed = remove.executor(request)
    const danger = await screen.findByRole("button", { name: "Remove" })
    expect(danger).toHaveAttribute("data-variant", "destructive")
    fireEvent.click(danger)
    await expect(confirmed).resolves.toBe("completed")
    expect(useAgentStore.getState().sessions.has("clicked")).toBe(false)
    expect(useAgentStore.getState().activeSessionId).toBe("other")
  })

  it("uses i18n assistive copy for the empty state", () => {
    render(<AgentNavContent />)

    expect(screen.getByText("No sessions yet", { selector: ".sr-only" })).toBeInTheDocument()
  })

  it("新增 session 開啟 picker，點 pi 卡以目前 workspace cwd 建立 session", async () => {
    const newSession = vi.fn(async () => "s-new")
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    useAgentStore.setState({ newSession })

    render(<AgentNavContent />)

    const button = screen.getByRole("button", { name: "New session" })
    fireEvent.click(button)
    fireEvent.click(screen.getByTestId("agent-picker-card-pi"))

    await waitFor(() => expect(newSession).toHaveBeenCalledWith("/workspace", "pi"))
  })

  it("creates a session with the agent chosen in the picker", () => {
    const spy = vi.spyOn(useAgentStore.getState(), "newSession").mockResolvedValue("s1")
    useWorkspaceStore.setState({ workspacePath: "/ws-a" })
    render(<AgentNavContent />)
    fireEvent.click(screen.getByRole("button", { name: "New session" }))
    fireEvent.click(screen.getByTestId("agent-picker-card-codex"))
    expect(spy).toHaveBeenCalledWith("/ws-a", "codex")
  })

  it("disables 新增 session and never spawns when no folder is open", () => {
    const newSession = vi.fn(async () => "s-new")
    useWorkspaceStore.setState({ workspacePath: null })
    useAgentStore.setState({ newSession })

    render(<AgentNavContent />)

    const button = screen.getByRole("button", { name: "New session" })
    expect(button).toBeDisabled()
    fireEvent.click(button)

    expect(newSession).not.toHaveBeenCalled()
  })

  it("F2: 未動 picker 時，新增 session 尊重全域 custom 設定（省略 agentId 走 custom path）", async () => {
    localStorage.setItem(
      AGENT_SETTINGS_STORAGE_KEY,
      JSON.stringify({ preset: "custom", command: "uvx my-acp", traceEnabled: false })
    )
    const newSession = vi.fn(async () => "s-new")
    useWorkspaceStore.setState({ workspacePath: "/ws-a" })
    useAgentStore.setState({ newSession })

    render(<AgentNavContent />)

    fireEvent.click(screen.getByRole("button", { name: "New session" }))
    // custom 是全域 preset 時，picker 一開就高亮並展開 custom 卡；按 Enter 確認即可
    // 沿用「省略 agentId」的 custom-command path（單參數呼叫），不需重新輸入 command。
    fireEvent.keyDown(screen.getByTestId("agent-picker-custom-input"), { key: "Enter" })

    await waitFor(() => expect(newSession).toHaveBeenCalledWith("/ws-a"))
  })

  it("F4: session.cwd 帶 trailing slash 時仍視為同一個 workspace 而列出", () => {
    useWorkspaceStore.setState({ workspacePath: "/ws-a" })
    useAgentStore.setState({
      sessions: new Map([
        ["a1", session({ title: "A", cwd: "/ws-a/" })],
      ]),
    })

    render(<AgentNavContent />)

    expect(screen.queryByText("A")).toBeInTheDocument()
  })

  it.each([
    { agentId: "pi", glyph: "π" },
    { agentId: "claude", glyph: "C" },
    { agentId: "codex", glyph: "X" },
  ] as const)("renders the $agentId row badge with the $glyph glyph", ({ agentId, glyph }) => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    useAgentStore.setState({
      sessions: new Map([["s-1", session({ agentId, cwd: "/workspace" })]]),
    })

    render(<AgentNavContent />)

    expect(screen.getByTestId("agent-session-badge-s-1")).toHaveTextContent(glyph)
  })

  it("renders a first-letter glyph for a session with no known agentId", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    useAgentStore.setState({
      sessions: new Map([
        ["s-1", session({ agentId: undefined, agentLabel: "gpt-5", cwd: "/workspace" })],
      ]),
    })

    render(<AgentNavContent />)

    // agentId 未知時 glyph 與顯示名皆以 session.agentLabel 為準（agentDisplayName
    // 語意），不再 fallback 用 agent preset 顯示名。
    expect(screen.getByTestId("agent-session-badge-s-1")).toHaveTextContent("G")
  })

  it("renders the agentLabel's first letter and display name for a custom agentId", () => {
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    useAgentStore.setState({
      sessions: new Map([
        ["s-1", session({ agentId: "custom", agentLabel: "My Custom Agent", model: null, cwd: "/workspace" })],
      ]),
    })

    render(<AgentNavContent />)

    expect(screen.getByTestId("agent-session-badge-s-1")).toHaveTextContent("M")
    expect(screen.getByText("My Custom Agent · My Custom Agent")).toBeInTheDocument()
  })

  it("never falls back to a relative cwd when workspacePath is not absolute", () => {
    const newSession = vi.fn(async () => "s-new")
    useWorkspaceStore.setState({ workspacePath: "." })
    useAgentStore.setState({ newSession })

    render(<AgentNavContent />)

    const button = screen.getByRole("button", { name: "New session" })
    expect(button).toBeDisabled()
    fireEvent.click(button)

    expect(newSession).not.toHaveBeenCalled()
  })
})
