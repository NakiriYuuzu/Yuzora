import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import { AgentPickerPopover } from "@/app/workbench/AgentPickerPopover"
import { AGENT_SETTINGS_STORAGE_KEY } from "@/app/workbench/settingsStorage"
import type { AuthRequiredState } from "@/state/agentStore"
import { useAgentStore } from "@/state/agentStore"

const originalAgentActions = {
  newSession: useAgentStore.getState().newSession,
}
const originalAuthRequired = useAgentStore.getState().authRequired

// Bun-hosted vitest injects an empty `localStorage` global with no Storage
// methods (see agentNavContent.test.tsx) — install a minimal in-memory Storage
// so settingsStorage reads/writes for real.
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

function authRequired(overrides: Partial<AuthRequiredState> = {}): AuthRequiredState {
  return {
    cwd: "/ws",
    sessionId: null,
    authMethods: [],
    message: "auth required",
    ...overrides,
  }
}

beforeEach(() => {
  installLocalStorage()
  localStorage.clear()
})

afterEach(() => {
  cleanup()
  useAgentStore.setState({
    authRequired: originalAuthRequired,
    ...originalAgentActions,
  })
  vi.clearAllMocks()
})

describe("AgentPickerPopover", () => {
  it("uses dialog semantics and moves initial focus into the popup", () => {
    render(<AgentPickerPopover cwd="/ws" onClose={() => {}} dialogId="agent-picker-test" />)

    const dialog = screen.getByRole("dialog", { name: "Choose agent" })
    expect(dialog).toHaveAttribute("id", "agent-picker-test")
    expect(dialog).toHaveFocus()
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument()
    expect(screen.queryByRole("option")).not.toBeInTheDocument()
  })

  it("renders the three brand preset cards plus a custom entry", () => {
    render(<AgentPickerPopover cwd="/ws" onClose={() => {}} />)

    expect(screen.getByTestId("agent-picker-card-pi")).toHaveTextContent("Pi")
    expect(screen.getByTestId("agent-picker-card-pi")).toHaveTextContent("bunx pi-acp@0.0.31")
    expect(screen.getByTestId("agent-picker-card-claude")).toHaveTextContent("Claude")
    expect(screen.getByTestId("agent-picker-card-codex")).toHaveTextContent("Codex")
    expect(screen.getByTestId("agent-picker-card-custom")).toHaveTextContent("Custom command…")
  })

  it("renders each curated card with its independently selected effective command", () => {
    localStorage.setItem(
      AGENT_SETTINGS_STORAGE_KEY,
      JSON.stringify({
        preset: "pi",
        command: "bunx pi-acp@0.0.31",
        traceEnabled: false,
        presetCommands: {
          pi: { mode: "latest", customCommand: "" },
          claude: { mode: "custom", customCommand: "uvx wrapped-claude" },
          codex: { mode: "verified", customCommand: "" },
        },
      }),
    )

    render(<AgentPickerPopover cwd="/ws" onClose={() => {}} />)

    expect(screen.getByTestId("agent-picker-card-pi")).toHaveTextContent("bunx pi-acp@latest")
    expect(screen.getByTestId("agent-picker-card-claude")).toHaveTextContent("uvx wrapped-claude")
    expect(screen.getByTestId("agent-picker-card-codex")).toHaveTextContent("codex-acp@1.1.2")
  })

  it("highlights the global preset by default", () => {
    localStorage.setItem(
      AGENT_SETTINGS_STORAGE_KEY,
      JSON.stringify({ preset: "codex", command: "", traceEnabled: false })
    )

    render(<AgentPickerPopover cwd="/ws" onClose={() => {}} />)

    expect(screen.getByTestId("agent-picker-card-codex")).toHaveAttribute("data-highlighted", "true")
    expect(screen.getByTestId("agent-picker-card-pi")).toHaveAttribute("data-highlighted", "false")
  })

  it("moves the highlight with ArrowDown/ArrowUp and confirms with Enter", async () => {
    const newSession = vi.fn(async () => "s1")
    useAgentStore.setState({ newSession })
    const onClose = vi.fn()

    render(<AgentPickerPopover cwd="/ws" onClose={onClose} />)
    const dialog = screen.getByRole("dialog", { name: "Choose agent" })

    // default highlight is pi (global default preset)
    fireEvent.keyDown(dialog, { key: "ArrowDown" })
    expect(screen.getByTestId("agent-picker-card-claude")).toHaveAttribute("data-highlighted", "true")

    fireEvent.keyDown(dialog, { key: "ArrowDown" })
    expect(screen.getByTestId("agent-picker-card-codex")).toHaveAttribute("data-highlighted", "true")

    fireEvent.keyDown(dialog, { key: "Enter" })

    expect(newSession).toHaveBeenCalledWith("/ws", "codex")
    expect(onClose).toHaveBeenCalled()
  })

  it("keeps native-button focus and highlighted state aligned", () => {
    const newSession = vi.fn(async () => "s1")
    useAgentStore.setState({ newSession })
    render(<AgentPickerPopover cwd="/ws" onClose={() => {}} />)

    const pi = screen.getByTestId("agent-picker-card-pi")
    const claude = screen.getByTestId("agent-picker-card-claude")
    act(() => claude.focus())
    expect(claude).toHaveFocus()
    expect(claude).toHaveAttribute("data-highlighted", "true")

    fireEvent.keyDown(claude, { key: "ArrowUp" })
    expect(claude).toHaveAttribute("data-highlighted", "true")
    expect(pi).toHaveAttribute("data-highlighted", "false")

    fireEvent.click(claude)
    expect(newSession).toHaveBeenCalledWith("/ws", "claude")
  })

  it("clicking a card creates a session with that agentId regardless of highlight", () => {
    const newSession = vi.fn(async () => "s1")
    useAgentStore.setState({ newSession })
    const onClose = vi.fn()

    render(<AgentPickerPopover cwd="/ws" onClose={onClose} />)
    fireEvent.click(screen.getByTestId("agent-picker-card-claude"))

    expect(newSession).toHaveBeenCalledWith("/ws", "claude")
    expect(onClose).toHaveBeenCalled()
  })

  it("Escape closes the popover when the custom card is not expanded", () => {
    const onClose = vi.fn()
    render(<AgentPickerPopover cwd="/ws" onClose={onClose} />)

    fireEvent.keyDown(document, { key: "Escape" })

    expect(onClose).toHaveBeenCalled()
  })

  it("a mousedown outside the popover closes it", () => {
    const onClose = vi.fn()
    render(<AgentPickerPopover cwd="/ws" onClose={onClose} />)

    fireEvent.mouseDown(document.body)

    expect(onClose).toHaveBeenCalled()
  })

  it("expands the custom command input on click, prefilled empty when the global preset isn't custom", () => {
    render(<AgentPickerPopover cwd="/ws" onClose={() => {}} />)

    expect(screen.queryByTestId("agent-picker-custom-input")).not.toBeInTheDocument()
    fireEvent.click(screen.getByTestId("agent-picker-card-custom").querySelector("button")!)

    expect(screen.getByTestId("agent-picker-custom-input")).toHaveValue("")
  })

  it("prefills the custom input and auto-expands when custom is the global preset", () => {
    localStorage.setItem(
      AGENT_SETTINGS_STORAGE_KEY,
      JSON.stringify({ preset: "custom", command: "uvx my-acp", traceEnabled: false })
    )

    render(<AgentPickerPopover cwd="/ws" onClose={() => {}} />)

    expect(screen.getByTestId("agent-picker-card-custom")).toHaveAttribute("data-highlighted", "true")
    expect(screen.getByTestId("agent-picker-custom-input")).toHaveValue("uvx my-acp")
  })

  it("Escape collapses the expanded custom input, restores dialog focus, then closes", async () => {
    const onClose = vi.fn()
    render(<AgentPickerPopover cwd="/ws" onClose={onClose} />)

    fireEvent.click(screen.getByTestId("agent-picker-card-custom").querySelector("button")!)
    expect(screen.getByTestId("agent-picker-custom-input")).toBeInTheDocument()

    fireEvent.keyDown(document, { key: "Escape" })
    expect(screen.queryByTestId("agent-picker-custom-input")).not.toBeInTheDocument()
    expect(onClose).not.toHaveBeenCalled()
    await waitFor(() => expect(screen.getByRole("dialog", { name: "Choose agent" })).toHaveFocus())

    fireEvent.keyDown(document, { key: "Escape" })
    expect(onClose).toHaveBeenCalled()
  })

  it("saves the typed command as the global custom preset and creates the session via the agentId-omitted path on Enter", () => {
    const newSession = vi.fn(async () => "s1")
    useAgentStore.setState({ newSession })
    const onClose = vi.fn()

    render(<AgentPickerPopover cwd="/ws" onClose={onClose} />)
    fireEvent.click(screen.getByTestId("agent-picker-card-custom").querySelector("button")!)
    fireEvent.change(screen.getByTestId("agent-picker-custom-input"), { target: { value: "uvx my-acp" } })
    fireEvent.keyDown(screen.getByTestId("agent-picker-custom-input"), { key: "Enter" })

    expect(JSON.parse(localStorage.getItem(AGENT_SETTINGS_STORAGE_KEY) ?? "{}")).toMatchObject({
      preset: "custom",
      command: "uvx my-acp",
    })
    expect(newSession).toHaveBeenCalledWith("/ws")
    expect(onClose).toHaveBeenCalled()
  })

  it("saves and creates the session on clicking the confirm button too", () => {
    const newSession = vi.fn(async () => "s1")
    useAgentStore.setState({ newSession })
    const onClose = vi.fn()

    render(<AgentPickerPopover cwd="/ws" onClose={onClose} />)
    fireEvent.click(screen.getByTestId("agent-picker-card-custom").querySelector("button")!)
    fireEvent.change(screen.getByTestId("agent-picker-custom-input"), { target: { value: "uvx my-acp" } })
    fireEvent.click(screen.getByTestId("agent-picker-custom-confirm"))

    expect(newSession).toHaveBeenCalledWith("/ws")
    expect(onClose).toHaveBeenCalled()
  })

  it("shows a needs-login marker only on the card matching the store's authRequired.agentId", () => {
    useAgentStore.setState({ authRequired: authRequired({ agentId: "codex" }) })

    render(<AgentPickerPopover cwd="/ws" onClose={() => {}} />)

    expect(screen.getByTestId("agent-picker-needslogin-codex")).toHaveTextContent("Needs login")
    expect(screen.queryByTestId("agent-picker-needslogin-pi")).not.toBeInTheDocument()
    expect(screen.queryByTestId("agent-picker-needslogin-claude")).not.toBeInTheDocument()
  })

  it("shows no needs-login marker when there is no pending auth", () => {
    useAgentStore.setState({ authRequired: null })

    render(<AgentPickerPopover cwd="/ws" onClose={() => {}} />)

    expect(screen.queryByTestId("agent-picker-needslogin-pi")).not.toBeInTheDocument()
    expect(screen.queryByTestId("agent-picker-needslogin-claude")).not.toBeInTheDocument()
    expect(screen.queryByTestId("agent-picker-needslogin-codex")).not.toBeInTheDocument()
  })
})
