import { beforeEach, describe, expect, it } from "vitest"

import {
    RECENT_WORKSPACES_STORAGE_KEY,
    loadRecentWorkspaces,
    useRecentWorkspacesStore
} from "./recentWorkspaces"

// The Bun-hosted test runtime injects an empty `localStorage` global with no
// Storage methods (see gitStore.test.ts). Install a minimal in-memory Storage
// so persistence is exercised for real rather than always hitting the
// try/catch fallback.
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

const record = (p: string) => useRecentWorkspacesStore.getState().record(p)
const remove = (p: string) => useRecentWorkspacesStore.getState().remove(p)
const list = () => useRecentWorkspacesStore.getState().list

beforeEach(() => {
    installLocalStorage()
    localStorage.clear()
    // The store's in-memory list is seeded once at import; re-sync it to the
    // freshly-cleared storage so each test starts empty.
    useRecentWorkspacesStore.setState({ list: loadRecentWorkspaces() })
})

describe("useRecentWorkspacesStore", () => {
    it("records a workspace as the most recent entry (store + storage)", () => {
        record("/a")
        expect(list()).toEqual(["/a"])
        expect(loadRecentWorkspaces()).toEqual(["/a"])
    })

    it("moves a re-opened workspace back to the front (MRU order)", () => {
        record("/a")
        record("/b")
        record("/a")
        expect(list()).toEqual(["/a", "/b"])
    })

    it("dedupes paths that differ only by a trailing slash", () => {
        record("/a/b/")
        record("/a/b")
        expect(list()).toEqual(["/a/b"])
    })

    it("caps the list at 10 entries, dropping the oldest", () => {
        for (let i = 0; i < 12; i++) record(`/w${i}`)
        expect(list()).toHaveLength(10)
        expect(list()[0]).toBe("/w11")
        expect(list()).not.toContain("/w0")
        expect(list()).not.toContain("/w1")
    })

    it("removes an entry (e.g. after a failed open)", () => {
        record("/a")
        record("/b")
        remove("/a")
        expect(list()).toEqual(["/b"])
        expect(loadRecentWorkspaces()).toEqual(["/b"])
    })

    it("removing a path not in the list is a no-op", () => {
        record("/a")
        remove("/missing")
        expect(list()).toEqual(["/a"])
    })

    it("falls back to an empty list on malformed stored JSON", () => {
        localStorage.setItem(RECENT_WORKSPACES_STORAGE_KEY, "{not json")
        expect(loadRecentWorkspaces()).toEqual([])
    })

    it("falls back to an empty list when the stored JSON isn't an array", () => {
        localStorage.setItem(RECENT_WORKSPACES_STORAGE_KEY, JSON.stringify({ oops: true }))
        expect(loadRecentWorkspaces()).toEqual([])
    })
})
