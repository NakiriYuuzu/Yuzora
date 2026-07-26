import { beforeEach, describe, expect, it, vi } from "vitest"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import { AgentAuthRequiredError, type AgentConnection, type AgentAuthMethod } from "@/agent/acpConnection"
import { AGENT_SETTINGS_STORAGE_KEY } from "@/app/workbench/settingsStorage"
import { MAX_TERMINAL_TABS, useTerminalStore } from "@/state/terminalStore"
import { useUiStore } from "@/state/uiStore"
import { createAgentStore, __test_terminalLoginShellCommand as build } from "./agentStore"

// The Bun-hosted test runtime injects an empty `localStorage` global with no
// Storage methods (see gitStore.test.ts). Install a minimal in-memory Storage
// so persistence is exercised for real.
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
        }
    }
    Object.defineProperty(globalThis, "localStorage", {
        value: mock,
        configurable: true,
        writable: true
    })
}

describe("terminal login command honors the selected preset", () => {
    beforeEach(() => {
        installLocalStorage()
        localStorage.clear()
    })

    it("uses the claude adapter binary when preset=claude", () => {
        localStorage.setItem(AGENT_SETTINGS_STORAGE_KEY,
            JSON.stringify({ preset: "claude", command: "", traceEnabled: false }))
        const cmd = build({ id: "l", name: "Login", type: "terminal", args: ["--login"], env: {} })
        expect(cmd).toContain("@agentclientprotocol/claude-agent-acp@0.62.0")
    })

    it("uses the picker-selected agent's login command, not the global preset (F1)", async () => {
        localStorage.setItem(AGENT_SETTINGS_STORAGE_KEY,
            JSON.stringify({ preset: "claude", command: "", traceEnabled: false }))
        const authMethod: AgentAuthMethod = {
            id: "codex_terminal_login",
            name: "Login",
            type: "terminal",
            args: ["--login"],
            env: {}
        }
        const connection: AgentConnection = {
            newSession: vi.fn(async () => {
                throw new AgentAuthRequiredError({ authMethods: [authMethod], cwd: "/ws", sessionId: null })
            }),
            loadSession: vi.fn(async () => {}),
            listSessions: vi.fn(async () => []),
            prompt: vi.fn(),
            cancel: vi.fn()
        }
        const store = createAgentStore({ connection })

        await store.getState().newSession("/ws", "codex").catch(() => undefined)
        expect(store.getState().authRequired?.agentId).toBe("codex")

        store.getState().beginTerminalLogin()

        const sessions = Object.values(useTerminalStore.getState().sessions) as Array<
            { shellArgs?: string[] }
        >
        const added = sessions.at(-1)
        expect(added?.shellArgs?.[1]).toContain("@agentclientprotocol/codex-acp@1.1.7")
        expect(added?.shellArgs?.[1]).not.toContain("claude-agent-acp")
    })

    it("uses an explicitly selected Latest command through the shared resolver", () => {
        localStorage.setItem(AGENT_SETTINGS_STORAGE_KEY, JSON.stringify({
            preset: "claude",
            command: "",
            traceEnabled: false,
            presetCommands: {
                claude: { mode: "latest", customCommand: "" }
            }
        }))

        const cmd = build({ id: "l", name: "Login", type: "terminal", args: ["--login"], env: {} })

        expect(cmd).toContain("@agentclientprotocol/claude-agent-acp@0.62.0")
    })

    it("uses a curated preset's untrusted Custom command for that explicit login attempt", async () => {
        localStorage.setItem(AGENT_SETTINGS_STORAGE_KEY, JSON.stringify({
            preset: "pi",
            command: "",
            traceEnabled: false,
            presetCommands: {
                codex: { mode: "custom", customCommand: "uvx wrapped-codex" }
            }
        }))
        const authMethod: AgentAuthMethod = {
            id: "codex_terminal_login",
            name: "Login",
            type: "terminal",
            args: ["--login"],
            env: {}
        }
        const connection: AgentConnection = {
            newSession: vi.fn(async () => {
                throw new AgentAuthRequiredError({ authMethods: [authMethod], cwd: "/ws" })
            }),
            loadSession: vi.fn(async () => {}),
            listSessions: vi.fn(async () => []),
            prompt: vi.fn(),
            cancel: vi.fn()
        }
        const store = createAgentStore({ connection })

        await store.getState().newSession("/ws", "codex").catch(() => undefined)
        store.getState().beginTerminalLogin()

        const sessions = Object.values(useTerminalStore.getState().sessions) as Array<{ shellArgs?: string[] }>
        expect(sessions.at(-1)?.shellArgs?.[1]).toContain("uvx wrapped-codex --login")
        expect(store.getState().authRequired?.agentIdentity?.trustedAgentId).toBeNull()
    })
})

describe("terminal login respects the terminal tab limit", () => {
  beforeEach(() => {
    clearMocks()
    installLocalStorage()
    localStorage.clear()
    useTerminalStore.getState().reset()
    useUiStore.setState({ terminalOpen: false })
  })

  it("reports an error instead of opening an empty drawer when the workspace is full", async () => {
    const dialogs: string[] = []
    mockIPC((cmd, payload) => {
      if (cmd === "plugin:dialog|message") {
        dialogs.push(JSON.stringify(payload))
        return "Ok"
      }
      return undefined
    })
    const authMethod: AgentAuthMethod = {
      id: "codex_terminal_login",
      name: "Login",
      type: "terminal",
      args: ["--login"],
      env: {}
    }
    const connection: AgentConnection = {
      newSession: vi.fn(async () => {
        throw new AgentAuthRequiredError({ authMethods: [authMethod], cwd: "/ws", sessionId: null })
      }),
      loadSession: vi.fn(async () => {}),
      listSessions: vi.fn(async () => []),
      prompt: vi.fn(),
      cancel: vi.fn()
    }
    const store = createAgentStore({ connection })
    await store.getState().newSession("/ws", "codex").catch(() => undefined)

    for (let index = 0; index < MAX_TERMINAL_TABS; index += 1) {
      useTerminalStore.getState().addSession("/ws", {
        sessionId: `filler-${index}`,
        title: `Terminal ${index + 1}`,
        launchStatus: "running",
        workspace: "/ws",
        shell: "",
        cols: 80,
        rows: 24
      })
    }
    const before = Object.keys(useTerminalStore.getState().sessions).length

    store.getState().beginTerminalLogin()

    await vi.waitFor(() => expect(dialogs).toHaveLength(1))
    expect(dialogs[0]).toContain(String(MAX_TERMINAL_TABS))
    expect(Object.keys(useTerminalStore.getState().sessions)).toHaveLength(before)
    // The drawer must not be opened onto a login terminal that never appeared.
    expect(useUiStore.getState().terminalOpen).toBe(false)
  })
})
