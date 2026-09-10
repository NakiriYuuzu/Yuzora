import { beforeEach, expect, it, vi } from "vitest"
import { useWorkspaceStore } from "./workspaceStore"
import { loadWorkspaceSessionEntry, saveWorkspaceSession } from "./workspaceSession"

beforeEach(() => { useWorkspaceStore.getState().setWorkspace("/pin"); const data = new Map<string, string>(); vi.stubGlobal("localStorage", { getItem: (key: string) => data.get(key) ?? null, setItem: (key: string, value: string) => data.set(key, value), removeItem: (key: string) => data.delete(key) }) })
it("bulk closes preserve dirty pinned tabs and their active path", () => {
    const store = useWorkspaceStore.getState()
    store.openTab("/pin/a.md"); store.openTab("/pin/b.md"); store.openTab("/pin/c.md")
    store.markDirty("/pin/b.md", true)
    store.toggleTabPinned(0, "/pin/b.md")
    store.closeOtherTabs(0, "/pin/a.md")
    expect(useWorkspaceStore.getState().groups[0].tabs.map((tab) => tab.path)).toEqual(["/pin/b.md", "/pin/a.md"])
    store.closeAllTabs(0)
    expect(useWorkspaceStore.getState().groups[0]).toMatchObject({ activePath: "/pin/b.md", tabs: [{ path: "/pin/b.md", pinned: true, dirty: true }] })
})
it("persists only pins belonging to actual restored files and accepts old unpinned sessions", () => {
    saveWorkspaceSession({ workspacePath: "/pin", tabs: ["/pin/a.md"], activePath: "/pin/a.md", pinnedPaths: ["/pin/a.md", "yuzora://preview", "/missing"] })
    expect(loadWorkspaceSessionEntry("/pin")?.pinnedPaths).toEqual(["/pin/a.md"])
    saveWorkspaceSession({ workspacePath: "/old", tabs: ["/old/a"], activePath: null })
    expect(loadWorkspaceSessionEntry("/old")).toEqual({ tabs: ["/old/a"], activePath: null })
})
