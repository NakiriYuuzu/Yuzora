import { beforeEach, describe, expect, it } from "vitest"

import {
    WORKSPACE_SESSION_STORAGE_KEY,
    clearWorkspaceSession,
    loadWorkspaceSession,
    saveWorkspaceSession,
    type WorkspaceSession
} from "./workspaceSession"

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

beforeEach(() => {
    installLocalStorage()
    localStorage.clear()
})

const SESSION: WorkspaceSession = {
    workspacePath: "/ws",
    tabs: ["/ws/a.ts", "/ws/b.ts"],
    activePath: "/ws/a.ts"
}

describe("workspaceSession", () => {
    it("round-trips a saved session", () => {
        saveWorkspaceSession(SESSION)
        expect(loadWorkspaceSession()).toEqual(SESSION)
    })

    it("round-trips a session with no active tab", () => {
        const s: WorkspaceSession = { workspacePath: "/ws", tabs: [], activePath: null }
        saveWorkspaceSession(s)
        expect(loadWorkspaceSession()).toEqual(s)
    })

    it("returns null when nothing is stored", () => {
        expect(loadWorkspaceSession()).toBeNull()
    })

    it("returns null on malformed JSON", () => {
        localStorage.setItem(WORKSPACE_SESSION_STORAGE_KEY, "{not json")
        expect(loadWorkspaceSession()).toBeNull()
    })

    it("returns null when the shape is invalid (missing workspacePath)", () => {
        localStorage.setItem(
            WORKSPACE_SESSION_STORAGE_KEY,
            JSON.stringify({ tabs: [], activePath: null })
        )
        expect(loadWorkspaceSession()).toBeNull()
    })

    it("returns null when tabs contains a non-string", () => {
        localStorage.setItem(
            WORKSPACE_SESSION_STORAGE_KEY,
            JSON.stringify({ workspacePath: "/ws", tabs: ["/ws/a.ts", 42], activePath: null })
        )
        expect(loadWorkspaceSession()).toBeNull()
    })

    it("clears a stored session", () => {
        saveWorkspaceSession(SESSION)
        clearWorkspaceSession()
        expect(loadWorkspaceSession()).toBeNull()
    })
})
