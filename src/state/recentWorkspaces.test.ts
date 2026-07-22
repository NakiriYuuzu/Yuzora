import { beforeEach, describe, expect, it } from "vitest"

import {
    MOVE_OPENED_WORKSPACE_TO_TOP_STORAGE_KEY,
    RECENT_WORKSPACE_PRESENTATIONS_STORAGE_KEY,
    RECENT_WORKSPACES_STORAGE_KEY,
    loadMoveOpenedWorkspaceToTop,
    loadRecentWorkspacePresentations,
    loadRecentWorkspaces,
    normalizeWorkspacePath,
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
const setMoveOpenedWorkspaceToTop = (enabled: boolean) =>
    useRecentWorkspacesStore.getState().setMoveOpenedWorkspaceToTop(enabled)
const presentationFor = (path: string) =>
    useRecentWorkspacesStore.getState().presentationFor(path)
const updatePresentation = (
    path: string,
    patch: { name?: string; glyph?: string; color?: "ocean" }
) => useRecentWorkspacesStore.getState().updatePresentation(path, patch)

beforeEach(() => {
    installLocalStorage()
    localStorage.clear()
    // The store's in-memory list is seeded once at import; re-sync it to the
    // freshly-cleared storage so each test starts empty.
    useRecentWorkspacesStore.setState({
        list: loadRecentWorkspaces(),
        presentations: loadRecentWorkspacePresentations(),
        moveOpenedWorkspaceToTop: loadMoveOpenedWorkspaceToTop()
    })
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

    it("keeps an existing workspace in place when move-opened-to-top is disabled", () => {
        record("/a")
        record("/b")

        setMoveOpenedWorkspaceToTop(false)
        record("/a")

        expect(list()).toEqual(["/b", "/a"])
        expect(localStorage.getItem(MOVE_OPENED_WORKSPACE_TO_TOP_STORAGE_KEY)).toBe("false")
    })

    it("still adds a new workspace when move-opened-to-top is disabled", () => {
        record("/a")
        setMoveOpenedWorkspaceToTop(false)

        record("/b")

        expect(list()).toEqual(["/b", "/a"])
    })

    it("loads a persisted disabled move-opened-to-top preference", () => {
        localStorage.setItem(MOVE_OPENED_WORKSPACE_TO_TOP_STORAGE_KEY, "false")

        expect(loadMoveOpenedWorkspaceToTop()).toBe(false)
    })

    it("dedupes paths that differ only by a trailing slash", () => {
        record("/a/b/")
        record("/a/b")
        expect(list()).toEqual(["/a/b"])
    })

    it("dedupes extended and ordinary Windows aliases while retaining the newest raw path", () => {
        record("C:\\Work\\專案 空間")
        record("\\\\?\\C:\\Work\\專案 空間")

        expect(list()).toEqual(["\\\\?\\C:\\Work\\專案 空間"])
        expect(loadRecentWorkspaces()).toEqual(["\\\\?\\C:\\Work\\專案 空間"])
    })

    it("keeps the existing raw alias in place when MRU promotion is disabled", () => {
        record("\\\\Server\\Share\\Repo")
        setMoveOpenedWorkspaceToTop(false)

        record("\\\\?\\UNC\\Server\\Share\\Repo")

        expect(list()).toEqual(["\\\\Server\\Share\\Repo"])
    })

    it("dedupes drive-root aliases while retaining the newest raw root", () => {
        record("C:\\")
        record("\\\\?\\c:\\")

        expect(list()).toEqual(["\\\\?\\c:\\"])
        expect(loadRecentWorkspaces()).toEqual(["\\\\?\\c:\\"])
    })

    it("keeps an existing drive-root alias when MRU promotion is disabled", () => {
        record("C:\\")
        setMoveOpenedWorkspaceToTop(false)

        record("\\\\?\\c:\\")

        expect(list()).toEqual(["C:\\"])
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

    it("removes a Windows recent entry by an equivalent extended alias", () => {
        record("\\\\Server\\Share\\Repo")

        remove("\\\\?\\UNC\\server\\share\\repo")

        expect(list()).toEqual([])
        expect(loadRecentWorkspaces()).toEqual([])
    })

    it("removes a drive root by an equivalent extended alias", () => {
        record("C:\\")

        remove("\\\\?\\c:\\")

        expect(list()).toEqual([])
        expect(loadRecentWorkspaces()).toEqual([])
    })

    it("stores presentation metadata by canonical workspace path", () => {
        updatePresentation("C:\\Work\\Repo", {
            name: "Studio",
            glyph: "⚡",
            color: "ocean"
        })

        expect(presentationFor("\\\\?\\c:\\work\\repo\\")).toEqual({
            name: "Studio",
            glyph: "⚡",
            color: "ocean"
        })
        expect(loadRecentWorkspacePresentations()).toEqual({
            "c:/work/repo": { name: "Studio", glyph: "⚡", color: "ocean" }
        })
    })

    it("updates presentation fields without discarding the other fields", () => {
        updatePresentation("/work/repo", { name: "Studio", glyph: "⚡" })

        updatePresentation("/work/repo/", { name: "Studio Next" })

        expect(presentationFor("/work/repo")).toEqual({
            name: "Studio Next",
            glyph: "⚡"
        })
    })

    it("removing a recent workspace also clears its presentation metadata", () => {
        record("C:\\Work\\Repo")
        updatePresentation("C:\\Work\\Repo", { name: "Studio" })

        remove("\\\\?\\c:\\work\\repo")

        expect(presentationFor("C:\\Work\\Repo")).toBeUndefined()
        expect(localStorage.getItem(RECENT_WORKSPACE_PRESENTATIONS_STORAGE_KEY)).toBe("{}")
    })

    it("ignores malformed persisted presentation fields", () => {
        localStorage.setItem(RECENT_WORKSPACE_PRESENTATIONS_STORAGE_KEY, JSON.stringify({
            "/good": { name: "Studio", glyph: "🧩", color: "ocean" },
            "/bad-color": { name: "Unsafe", color: "url(javascript:alert(1))" },
            "/not-an-object": "nope"
        }))

        expect(loadRecentWorkspacePresentations()).toEqual({
            "/good": { name: "Studio", glyph: "🧩", color: "ocean" },
            "/bad-color": { name: "Unsafe" }
        })
    })

    it("dedupes persisted drive and UNC aliases while preserving the first MRU raw value", () => {
        localStorage.setItem(RECENT_WORKSPACES_STORAGE_KEY, JSON.stringify([
            "\\\\?\\C:\\Work\\Repo",
            "c:/work/repo/",
            "\\\\Server\\Share\\Repo",
            "\\\\?\\UNC\\server\\share\\repo\\"
        ]))

        expect(loadRecentWorkspaces()).toEqual([
            "\\\\?\\C:\\Work\\Repo",
            "\\\\Server\\Share\\Repo"
        ])
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

describe("normalizeWorkspacePath", () => {
    it("dedupes POSIX paths that differ only by a trailing slash", () => {
        expect(normalizeWorkspacePath("/a/b/")).toBe(normalizeWorkspacePath("/a/b"))
    })

    it("keeps a bare POSIX root as-is", () => {
        expect(normalizeWorkspacePath("/")).toBe("/")
    })

    it("dedupes Windows paths that differ only by a trailing backslash", () => {
        expect(normalizeWorkspacePath("C:\\repo\\")).toBe(normalizeWorkspacePath("C:\\repo"))
    })

    it("preserves ordinary and extended Windows drive roots", () => {
        expect(normalizeWorkspacePath("C:\\")).toBe("C:\\")
        expect(normalizeWorkspacePath("\\\\?\\C:\\")).toBe("\\\\?\\C:\\")
    })
})
