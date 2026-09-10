import { beforeEach, describe, expect, it } from "vitest"
import { useWorkspaceStore } from "./workspaceStore"
import type { HerdrSnapshot } from "@/lib/herdrTypes"
import { herdrPagePath } from "@/lib/herdrPages"

const initial = useWorkspaceStore.getState()
const scope = JSON.stringify(["host-a", "default"])
function snapshot(runtime = scope, space = "space-a", tabId = "tab-a", terminalId = "terminal-a"): HerdrSnapshot {
    return { herdrSessionId: runtime, focusedWorkspaceId: space, focusedTabId: tabId, tabs: [{ id: tabId, workspaceId: space, terminalId, label: tabId, paneCount: 1, focused: true, order: 0, status: "idle", active: true }], spaces: [], terminals: [], agents: [], protocol: 20, version: "0.8.2", raw: {} } as HerdrSnapshot
}
function open(runtime = scope, tabId: string | null = "tab-a", terminalId = "terminal-a") {
    useWorkspaceStore.getState().openHerdrTerminalPage({ herdrSessionId: runtime, terminalId, herdrTabId: tabId, herdrWorkspaceId: "space-a" })
    return herdrPagePath(runtime, terminalId)
}
const paths = () => useWorkspaceStore.getState().groups.flatMap((group) => group.tabs.map((tab) => tab.path))
beforeEach(() => useWorkspaceStore.setState(initial, true))

describe("dismissed Herdr work surfaces", () => {
    it("stays closed across Space switches and reconnect hydration until explicitly reopened", () => {
        const a = open()
        useWorkspaceStore.getState().closeTabsByPath([a])
        useWorkspaceStore.getState().hydrateHerdrPagesFromSnapshot(snapshot(scope, "space-b", "tab-b", "terminal-b"))
        useWorkspaceStore.getState().hydrateHerdrPagesFromSnapshot(snapshot())
        useWorkspaceStore.getState().openHerdrTerminalPage({ herdrSessionId: scope, terminalId: "terminal-a", herdrTabId: "tab-a", restore: true })
        expect(paths()).not.toContain(a)
        open()
        expect(paths()).toContain(a)
        expect(useWorkspaceStore.getState().dismissedHerdrPages).toEqual({})
    })
    it("uses full host and named session identity, not shared terminal or tab labels", () => {
        const a = open()
        useWorkspaceStore.getState().closeTab(0, a)
        for (const runtime of [JSON.stringify(["host-b", "default"]), JSON.stringify(["host-a", "other"])]) {
            useWorkspaceStore.getState().hydrateHerdrPagesFromSnapshot(snapshot(runtime))
            expect(paths()).toContain(herdrPagePath(runtime, "terminal-a"))
        }
        useWorkspaceStore.getState().hydrateHerdrPagesFromSnapshot(snapshot())
        expect(paths()).not.toContain(a)
    })
    it("matches legacy terminal identity and clears it when an explicit open supplies the tab id", () => {
        const a = open(scope, null)
        useWorkspaceStore.getState().closeTabsByPath([a])
        useWorkspaceStore.getState().hydrateHerdrPagesFromSnapshot(snapshot())
        expect(paths()).not.toContain(a)
        open()
        expect(paths()).toContain(a)
        expect(useWorkspaceStore.getState().dismissedHerdrPages).toEqual({})
    })
    it("bulk close dismisses only actual removed pages and retains pinned surfaces", () => {
        const a = open()
        const b = open(scope, "tab-b", "terminal-b")
        useWorkspaceStore.getState().toggleTabPinned(0, b)
        useWorkspaceStore.getState().closeAllTabs(0)
        useWorkspaceStore.getState().hydrateHerdrPagesFromSnapshot(snapshot())
        expect(paths()).not.toContain(a)
        expect(paths()).toContain(b)
        expect(Object.keys(useWorkspaceStore.getState().dismissedHerdrPages)).toHaveLength(1)
    })
    it("stable tab identity remains dismissed if the runtime rotates its terminal id", () => {
        useWorkspaceStore.getState().closeTabsByPath([open()])
        useWorkspaceStore.getState().hydrateHerdrPagesFromSnapshot(snapshot(scope, "space-a", "tab-a", "new-terminal"))
        expect(paths()).toHaveLength(0)
    })
})
