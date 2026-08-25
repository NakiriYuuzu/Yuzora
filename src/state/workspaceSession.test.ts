import { beforeEach, describe, expect, it, vi } from "vitest"

// Mocks for the openWorkspaceAtPath integration tests below (#60 T4c). The
// A→B→A restore behaviour lives in workspaceActions, but its dedicated test
// file belongs to a parallel workstream and sessionRestoreBridge.test.tsx
// mocks @/lib/workspaceActions module-wide — so the session-restore contract
// is pinned here, next to the session storage it exercises.
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }))
vi.mock("@/lib/ipc", () => ({
    openWorkspace: vi.fn(),
    startWatch: vi.fn(),
    openFile: vi.fn(),
    saveFile: vi.fn(),
    allowWorkspaceAssetScope: vi.fn()
}))
vi.mock("@/features/logs/userAction", () => ({ logUserAction: vi.fn() }))
vi.mock("@/editor/saveDocument", () => ({ saveDirtyTab: vi.fn() }))

import { allowWorkspaceAssetScope, openWorkspace, startWatch } from "@/lib/ipc"
import { openWorkspaceAtPath } from "@/lib/workspaceActions"
import { useWorkspaceStore } from "@/state/workspaceStore"

import {
    WORKSPACE_SESSION_MAX_WORKSPACES,
    WORKSPACE_SESSION_STORAGE_KEY,
    WORKSPACE_SESSION_V1_STORAGE_KEY,
    clearWorkspaceSession,
    loadWorkspaceSession,
    loadWorkspaceSessionEntry,
    markWorkspaceSessionActive,
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

    it("returns null on malformed v2 JSON", () => {
        localStorage.setItem(WORKSPACE_SESSION_STORAGE_KEY, "{not json")
        expect(loadWorkspaceSession()).toBeNull()
    })

    it("returns null when the v2 shape is invalid (bad entry)", () => {
        localStorage.setItem(
            WORKSPACE_SESSION_STORAGE_KEY,
            JSON.stringify({
                version: 2,
                lastWorkspacePath: "/ws",
                workspaces: { "/ws": { tabs: ["/ws/a.ts", 42], activePath: null } }
            })
        )
        expect(loadWorkspaceSession()).toBeNull()
    })

    it("clears the last session", () => {
        saveWorkspaceSession(SESSION)
        clearWorkspaceSession()
        expect(loadWorkspaceSession()).toBeNull()
        expect(loadWorkspaceSessionEntry("/ws")).toBeNull()
    })
})

describe("workspaceSession per-workspace entries (v2)", () => {
    it("keeps one entry per workspace; loadWorkspaceSession returns the last one", () => {
        saveWorkspaceSession(SESSION)
        saveWorkspaceSession({
            workspacePath: "/other",
            tabs: ["/other/x.ts"],
            activePath: "/other/x.ts"
        })
        expect(loadWorkspaceSessionEntry("/ws")).toEqual({
            tabs: ["/ws/a.ts", "/ws/b.ts"],
            activePath: "/ws/a.ts"
        })
        expect(loadWorkspaceSessionEntry("/other")).toEqual({
            tabs: ["/other/x.ts"],
            activePath: "/other/x.ts"
        })
        expect(loadWorkspaceSession()?.workspacePath).toBe("/other")
    })

    it("loadWorkspaceSessionEntry returns null for an unknown workspace", () => {
        saveWorkspaceSession(SESSION)
        expect(loadWorkspaceSessionEntry("/never-opened")).toBeNull()
    })

    it("markWorkspaceSessionActive moves the last pointer without touching entries", () => {
        saveWorkspaceSession(SESSION)
        markWorkspaceSessionActive("/other")
        // Cold-start restore now targets /other (no tabs recorded yet)…
        expect(loadWorkspaceSession()).toEqual({
            workspacePath: "/other",
            tabs: [],
            activePath: null
        })
        // …while /ws keeps its tabs for the next switch back.
        expect(loadWorkspaceSessionEntry("/ws")).toEqual({
            tabs: ["/ws/a.ts", "/ws/b.ts"],
            activePath: "/ws/a.ts"
        })
    })

    it("clearWorkspaceSession drops only the stale last workspace's entry", () => {
        saveWorkspaceSession({ workspacePath: "/other", tabs: ["/other/x.ts"], activePath: null })
        saveWorkspaceSession(SESSION)
        clearWorkspaceSession()
        expect(loadWorkspaceSession()).toBeNull()
        expect(loadWorkspaceSessionEntry("/ws")).toBeNull()
        expect(loadWorkspaceSessionEntry("/other")).toEqual({
            tabs: ["/other/x.ts"],
            activePath: null
        })
    })

    it("evicts the least recently saved workspace beyond the cap", () => {
        for (let i = 0; i <= WORKSPACE_SESSION_MAX_WORKSPACES; i++) {
            saveWorkspaceSession({ workspacePath: `/ws${i}`, tabs: [], activePath: null })
        }
        expect(loadWorkspaceSessionEntry("/ws0")).toBeNull()
        expect(loadWorkspaceSessionEntry("/ws1")).not.toBeNull()
        expect(
            loadWorkspaceSessionEntry(`/ws${WORKSPACE_SESSION_MAX_WORKSPACES}`)
        ).not.toBeNull()
    })

    it("re-saving an existing workspace refreshes its recency (not evicted)", () => {
        for (let i = 0; i < WORKSPACE_SESSION_MAX_WORKSPACES; i++) {
            saveWorkspaceSession({ workspacePath: `/ws${i}`, tabs: [], activePath: null })
        }
        // Touch the oldest, then push one more over the cap: /ws1 (now oldest) goes.
        saveWorkspaceSession({ workspacePath: "/ws0", tabs: ["/ws0/a.ts"], activePath: null })
        saveWorkspaceSession({ workspacePath: "/fresh", tabs: [], activePath: null })
        expect(loadWorkspaceSessionEntry("/ws0")?.tabs).toEqual(["/ws0/a.ts"])
        expect(loadWorkspaceSessionEntry("/ws1")).toBeNull()
    })
})

describe("workspaceSession file-only persistence", () => {
    it("save filters preview and Herdr pseudo paths from a mixed tab set", () => {
        saveWorkspaceSession({
            workspacePath: "/ws",
            tabs: [
                "/ws/a.ts",
                "yuzora://preview",
                "yuzora://markdown-preview/%2Fws%2Fnotes.md",
                "yuzora://herdr/live/term-1",
                "yuzora://herdr/wsl/ubuntu/term-legacy",
                "/ws/b.ts"
            ],
            activePath: "yuzora://herdr/live/term-1"
        })
        expect(loadWorkspaceSession()).toEqual({
            workspacePath: "/ws",
            tabs: ["/ws/a.ts", "/ws/b.ts"],
            activePath: null
        })
    })

    it("load sanitizes stale Herdr/preview paths already on disk", () => {
        localStorage.setItem(
            WORKSPACE_SESSION_STORAGE_KEY,
            JSON.stringify({
                version: 2,
                lastWorkspacePath: "/ws",
                workspaces: {
                    "/ws": {
                        tabs: [
                            "/ws/a.ts",
                            "yuzora://preview",
                            "yuzora://markdown-preview/%2Fws%2Fnotes.md",
                            "yuzora://herdr/live/term-9",
                            "yuzora://herdr/wsl/ubuntu/term-legacy",
                            "/ws/c.ts"
                        ],
                        activePath: "yuzora://markdown-preview/%2Fws%2Fnotes.md"
                    }
                }
            })
        )
        expect(loadWorkspaceSession()).toEqual({
            workspacePath: "/ws",
            tabs: ["/ws/a.ts", "/ws/c.ts"],
            activePath: null
        })
        expect(loadWorkspaceSessionEntry("/ws")).toEqual({
            tabs: ["/ws/a.ts", "/ws/c.ts"],
            activePath: null
        })
    })

    it("round-trips mixed file+preview+Herdr input as files only", () => {
        saveWorkspaceSession({
            workspacePath: "/ws",
            tabs: ["/ws/file.ts", "yuzora://preview", "yuzora://herdr/live/t1"],
            activePath: "/ws/file.ts"
        })
        const loaded = loadWorkspaceSession()
        expect(loaded?.tabs).toEqual(["/ws/file.ts"])
        expect(loaded?.activePath).toBe("/ws/file.ts")
        // Re-save the loaded session must not reintroduce pseudo paths.
        saveWorkspaceSession(loaded!)
        expect(loadWorkspaceSession()?.tabs).toEqual(["/ws/file.ts"])
    })
})

describe("workspaceSession v1 migration", () => {
    it("reads a legacy v1 session", () => {
        localStorage.setItem(WORKSPACE_SESSION_V1_STORAGE_KEY, JSON.stringify(SESSION))
        expect(loadWorkspaceSession()).toEqual(SESSION)
        expect(loadWorkspaceSessionEntry("/ws")).toEqual({
            tabs: ["/ws/a.ts", "/ws/b.ts"],
            activePath: "/ws/a.ts"
        })
    })

    it("returns null on malformed v1 JSON", () => {
        localStorage.setItem(WORKSPACE_SESSION_V1_STORAGE_KEY, "{not json")
        expect(loadWorkspaceSession()).toBeNull()
    })

    it("returns null when the v1 shape is invalid (missing workspacePath)", () => {
        localStorage.setItem(
            WORKSPACE_SESSION_V1_STORAGE_KEY,
            JSON.stringify({ tabs: [], activePath: null })
        )
        expect(loadWorkspaceSession()).toBeNull()
    })

    it("first save migrates to v2 and removes the v1 key, keeping the v1 entry", () => {
        localStorage.setItem(WORKSPACE_SESSION_V1_STORAGE_KEY, JSON.stringify(SESSION))
        saveWorkspaceSession({ workspacePath: "/other", tabs: ["/other/x.ts"], activePath: null })
        expect(localStorage.getItem(WORKSPACE_SESSION_V1_STORAGE_KEY)).toBeNull()
        expect(loadWorkspaceSessionEntry("/ws")).toEqual({
            tabs: ["/ws/a.ts", "/ws/b.ts"],
            activePath: "/ws/a.ts"
        })
        expect(loadWorkspaceSession()?.workspacePath).toBe("/other")
    })

    it("an existing v2 store wins over a lingering v1 key", () => {
        saveWorkspaceSession(SESSION)
        localStorage.setItem(
            WORKSPACE_SESSION_V1_STORAGE_KEY,
            JSON.stringify({ workspacePath: "/stale", tabs: ["/stale/z.ts"], activePath: null })
        )
        expect(loadWorkspaceSession()).toEqual(SESSION)
        expect(loadWorkspaceSessionEntry("/stale")).toBeNull()
    })
})

// #60 T4c：切回 workspace 時 openWorkspaceAtPath 從 per-workspace map 立即
// 還原 tabs 與 active tab（檔案內容由 EditorPane 背景載入，不在此 await）。
describe("openWorkspaceAtPath per-workspace tab 還原 (A→B→A)", () => {
    const tabPaths = () => useWorkspaceStore.getState().groups[0].tabs.map((t) => t.path)
    const activePath = () => useWorkspaceStore.getState().groups[0].activePath

    beforeEach(() => {
        useWorkspaceStore.setState({
            workspacePath: null,
            groups: [{ tabs: [], activePath: null }],
            activeGroupIndex: 0,
            pendingReveal: null
        })
        // Canonicalization is identity here so session keys match the input path.
        vi.mocked(openWorkspace).mockReset().mockImplementation(async (p: string) => ({
            canonicalPath: p,
            capabilityId: `ws-${p}`
        }))
        vi.mocked(startWatch).mockReset().mockResolvedValue(undefined)
        vi.mocked(allowWorkspaceAssetScope).mockReset().mockResolvedValue(undefined)
    })

    it("切回 workspace 時還原 entry 的 tabs 與 active tab", async () => {
        saveWorkspaceSession({
            workspacePath: "/a",
            tabs: ["/a/1.ts", "/a/2.ts"],
            activePath: "/a/2.ts"
        })
        await openWorkspaceAtPath("/a")
        expect(tabPaths()).toEqual(["/a/1.ts", "/a/2.ts"])
        expect(activePath()).toBe("/a/2.ts")
    })

    it("A→B→A：每個 workspace 還原自己的 tabs", async () => {
        saveWorkspaceSession({ workspacePath: "/a", tabs: ["/a/1.ts"], activePath: "/a/1.ts" })
        saveWorkspaceSession({ workspacePath: "/b", tabs: ["/b/1.ts"], activePath: "/b/1.ts" })

        await openWorkspaceAtPath("/a")
        expect(tabPaths()).toEqual(["/a/1.ts"])

        await openWorkspaceAtPath("/b")
        expect(tabPaths()).toEqual(["/b/1.ts"])
        expect(activePath()).toBe("/b/1.ts")

        await openWorkspaceAtPath("/a")
        expect(tabPaths()).toEqual(["/a/1.ts"])
        expect(activePath()).toBe("/a/1.ts")
    })

    it("無 entry 的 workspace 開啟後沒有分頁", async () => {
        await openWorkspaceAtPath("/fresh")
        expect(tabPaths()).toEqual([])
        expect(activePath()).toBeNull()
    })

    it("restoreSessionTabs: false（冷啟路徑）不從 map 還原", async () => {
        saveWorkspaceSession({ workspacePath: "/a", tabs: ["/a/1.ts"], activePath: "/a/1.ts" })
        await openWorkspaceAtPath("/a", { restoreSessionTabs: false })
        expect(tabPaths()).toEqual([])
    })

    it("entry 的 activePath 不在 tabs 時，active 落在最後開啟的分頁", async () => {
        saveWorkspaceSession({
            workspacePath: "/a",
            tabs: ["/a/1.ts", "/a/2.ts"],
            activePath: "/a/gone.ts"
        })
        await openWorkspaceAtPath("/a")
        expect(tabPaths()).toEqual(["/a/1.ts", "/a/2.ts"])
        expect(activePath()).toBe("/a/2.ts")
    })
})
