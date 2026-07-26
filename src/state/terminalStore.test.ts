import { beforeEach, describe, expect, it } from "vitest"

import {
    MAX_TERMINAL_TABS,
    MAX_TERMINAL_TITLE_LENGTH,
    terminalDisplayTitle,
    terminalInitialState,
    useTerminalStore,
    type TerminalSessionMeta
} from "./terminalStore"

function session(
    sessionId: string,
    workspace = "/ws/a",
    title = sessionId
): TerminalSessionMeta {
    return {
        sessionId,
        title,
        launchStatus: "opening",
        workspace,
        shell: "/bin/zsh",
        cols: 120,
        rows: 30
    }
}

const first = session("term-1", "/ws/a", "Terminal 1")
const second = session("term-2", "/ws/a", "Terminal 2")
const third = session("term-3", "/ws/a", "Terminal 3")
const otherWorkspace = session("term-b1", "/ws/b", "Terminal 1")

beforeEach(() => useTerminalStore.getState().reset())

describe("useTerminalStore", () => {
    it("adds the first tab and assigns it to the only visible pane", () => {
        useTerminalStore.getState().addSession("/ws/a", first, "pane-a")

        expect(useTerminalStore.getState().sessions[first.sessionId]).toEqual(first)
        expect(useTerminalStore.getState().layouts["/ws/a"]).toEqual({
            tabIds: [first.sessionId],
            panes: [{ paneId: "pane-a", sessionId: first.sessionId }],
            activePaneId: "pane-a",
            splitRatio: 0.5,
            nextTerminalNumber: 1,
            renamingSessionId: null
        })
    })

    it("keeps unlimited ordered tabs while New replaces only the focused pane", () => {
        const store = useTerminalStore.getState()
        store.addSession("/ws/a", first, "pane-a")
        store.addSession("/ws/a", second)
        store.addSession("/ws/a", third)

        const state = useTerminalStore.getState()
        expect(state.layouts["/ws/a"].tabIds).toEqual([
            first.sessionId,
            second.sessionId,
            third.sessionId
        ])
        expect(state.layouts["/ws/a"].panes).toEqual([
            { paneId: "pane-a", sessionId: third.sessionId }
        ])
        expect(state.sessionsForWorkspace("/ws/a")).toEqual([first, second, third])
    })

    it("stops adding tabs at MAX_TERMINAL_TABS across all workspaces", () => {
        const store = useTerminalStore.getState()
        // Half the budget in each workspace: a per-workspace cap would never
        // fire here and would leave every extra workspace free to add 64 more.
        const half = MAX_TERMINAL_TABS / 2
        for (let index = 0; index < half; index += 1) {
            store.addSession("/ws/a", session(`term-a-${index}`, "/ws/a"), "pane-a")
        }
        for (let index = 0; index < half; index += 1) {
            store.addSession("/ws/b", session(`term-b-${index}`, "/ws/b"), "pane-b")
        }
        expect(useTerminalStore.getState().layouts["/ws/a"].tabIds).toHaveLength(half)
        expect(useTerminalStore.getState().layouts["/ws/b"].tabIds).toHaveLength(half)

        const overflow = session("term-overflow", "/ws/a")
        store.addSession("/ws/a", overflow)
        store.splitFrom("/ws/a", "pane-a", session("term-split", "/ws/a"))
        // A previously untouched workspace gets no fresh budget either.
        store.addSession("/ws/c", session("term-c", "/ws/c"))

        const state = useTerminalStore.getState()
        expect(state.layouts["/ws/a"].tabIds).toHaveLength(half)
        expect(state.layouts["/ws/b"].tabIds).toHaveLength(half)
        expect(state.layouts["/ws/c"]).toBeUndefined()
        expect(state.sessions[overflow.sessionId]).toBeUndefined()
        expect(state.sessions["term-split"]).toBeUndefined()
        expect(state.sessions["term-c"]).toBeUndefined()
    })

    it("splits once to the right and rejects a third visible pane", () => {
        const store = useTerminalStore.getState()
        store.addSession("/ws/a", first, "pane-a")
        store.splitFrom("/ws/a", "pane-a", second)
        store.splitFrom("/ws/a", "pane-a", third)

        const state = useTerminalStore.getState()
        expect(state.layouts["/ws/a"].tabIds).toEqual([first.sessionId, second.sessionId])
        expect(state.layouts["/ws/a"].panes).toEqual([
            { paneId: "pane-a", sessionId: first.sessionId },
            { paneId: second.sessionId, sessionId: second.sessionId }
        ])
        expect(state.layouts["/ws/a"].activePaneId).toBe(second.sessionId)
        expect(state.sessions[third.sessionId]).toBeUndefined()
    })

    it("selects an already-visible tab by focusing its pane and replaces only the focused pane otherwise", () => {
        const store = useTerminalStore.getState()
        store.addSession("/ws/a", first, "pane-a")
        store.splitFrom("/ws/a", "pane-a", second)
        store.addSession("/ws/a", third)

        expect(useTerminalStore.getState().layouts["/ws/a"].panes).toEqual([
            { paneId: "pane-a", sessionId: first.sessionId },
            { paneId: second.sessionId, sessionId: third.sessionId }
        ])

        store.selectTab("/ws/a", second.sessionId)
        expect(useTerminalStore.getState().layouts["/ws/a"].panes).toEqual([
            { paneId: "pane-a", sessionId: first.sessionId },
            { paneId: second.sessionId, sessionId: second.sessionId }
        ])

        store.selectTab("/ws/a", first.sessionId)
        expect(useTerminalStore.getState().layouts["/ws/a"].activePaneId).toBe("pane-a")
    })

    it("unsplits when a visible tab closes without auto-filling from hidden tabs", () => {
        const store = useTerminalStore.getState()
        store.addSession("/ws/a", first, "pane-a")
        store.splitFrom("/ws/a", "pane-a", second)
        store.addSession("/ws/a", third)

        store.removeSession("/ws/a", first.sessionId)

        const layout = useTerminalStore.getState().layouts["/ws/a"]
        expect(layout.tabIds).toEqual([second.sessionId, third.sessionId])
        expect(layout.panes).toEqual([{ paneId: second.sessionId, sessionId: third.sessionId }])
        expect(layout.activePaneId).toBe(second.sessionId)
    })

    it("selects the right tab neighbor first, then the left, when the only visible tab closes", () => {
        const store = useTerminalStore.getState()
        store.addSession("/ws/a", first, "pane-a")
        store.addSession("/ws/a", second)
        store.addSession("/ws/a", third)
        store.selectTab("/ws/a", second.sessionId)

        store.removeSession("/ws/a", second.sessionId)
        expect(useTerminalStore.getState().layouts["/ws/a"].panes[0].sessionId).toBe(
            third.sessionId
        )

        store.removeSession("/ws/a", third.sessionId)
        expect(useTerminalStore.getState().layouts["/ws/a"].panes[0].sessionId).toBe(
            first.sessionId
        )
    })

    it("closes a hidden tab without changing visible panes or focus", () => {
        const store = useTerminalStore.getState()
        store.addSession("/ws/a", first, "pane-a")
        store.addSession("/ws/a", second)
        const before = useTerminalStore.getState().layouts["/ws/a"]

        store.removeSession("/ws/a", first.sessionId)

        const after = useTerminalStore.getState().layouts["/ws/a"]
        expect(after.panes).toEqual(before.panes)
        expect(after.activePaneId).toBe(before.activePaneId)
    })

    it("reorders tabs independently from visible pane assignments", () => {
        const store = useTerminalStore.getState()
        store.addSession("/ws/a", first, "pane-a")
        store.splitFrom("/ws/a", "pane-a", second)
        store.addSession("/ws/a", third)
        const panes = useTerminalStore.getState().layouts["/ws/a"].panes

        store.reorderTab("/ws/a", third.sessionId, 0)

        expect(useTerminalStore.getState().layouts["/ws/a"].tabIds).toEqual([
            third.sessionId,
            first.sessionId,
            second.sessionId
        ])
        expect(useTerminalStore.getState().layouts["/ws/a"].panes).toEqual(panes)
    })

    it("allocates monotonic default title numbers per workspace without reuse", () => {
        const store = useTerminalStore.getState()

        expect(store.allocateTerminalNumber("/ws/a")).toBe(1)
        expect(store.allocateTerminalNumber("/ws/a")).toBe(2)
        expect(store.allocateTerminalNumber("/ws/b")).toBe(1)
        expect(store.allocateTerminalNumber("/ws/a")).toBe(3)
    })

    it("applies Manual Alias over Shell Title over Default Name and clears each layer with blank input", () => {
        const store = useTerminalStore.getState()
        store.addSession("/ws/a", first)
        store.setShellTitle(first.sessionId, "  dev server\nready  ")
        expect(terminalDisplayTitle(useTerminalStore.getState().sessions[first.sessionId])).toBe(
            "dev server ready"
        )

        store.setManualTitle(first.sessionId, " API ")
        store.setShellTitle(first.sessionId, "new shell title")
        expect(terminalDisplayTitle(useTerminalStore.getState().sessions[first.sessionId])).toBe("API")

        store.setManualTitle(first.sessionId, "  ")
        expect(terminalDisplayTitle(useTerminalStore.getState().sessions[first.sessionId])).toBe(
            "new shell title"
        )
        store.setShellTitle(first.sessionId, "\u0000\n")
        expect(terminalDisplayTitle(useTerminalStore.getState().sessions[first.sessionId])).toBe(
            first.title
        )
    })

    it("caps normalized titles at 128 Unicode code points", () => {
        const store = useTerminalStore.getState()
        store.addSession("/ws/a", first)
        store.setManualTitle(first.sessionId, "🌙".repeat(MAX_TERMINAL_TITLE_LENGTH + 10))

        expect(Array.from(useTerminalStore.getState().sessions[first.sessionId].manualTitle ?? ""))
            .toHaveLength(MAX_TERMINAL_TITLE_LENGTH)
    })

    it("retains and clamps the split ratio for later splits", () => {
        const store = useTerminalStore.getState()
        store.addSession("/ws/a", first)

        store.setSplitRatio("/ws/a", 0.1)
        expect(useTerminalStore.getState().layouts["/ws/a"].splitRatio).toBe(0.2)
        store.setSplitRatio("/ws/a", 0.9)
        expect(useTerminalStore.getState().layouts["/ws/a"].splitRatio).toBe(0.8)
    })

    it("tracks rename and launch state without changing the display-title priority", () => {
        const store = useTerminalStore.getState()
        store.addSession("/ws/a", first)
        store.beginRename("/ws/a", first.sessionId)
        store.setLaunchStatus(first.sessionId, "failed")

        expect(useTerminalStore.getState().layouts["/ws/a"].renamingSessionId).toBe(first.sessionId)
        expect(useTerminalStore.getState().sessions[first.sessionId].launchStatus).toBe("failed")

        store.finishRename("/ws/a", first.sessionId)
        expect(useTerminalStore.getState().layouts["/ws/a"].renamingSessionId).toBeNull()
    })

    it("keeps sessions and counters isolated by workspace", () => {
        const store = useTerminalStore.getState()
        store.addSession("/ws/a", first)
        store.addSession("/ws/b", otherWorkspace)

        expect(store.sessionsForWorkspace("/ws/a")).toEqual([first])
        expect(store.sessionsForWorkspace("/ws/b")).toEqual([otherWorkspace])
    })

    it("reset restores the exported initial state", () => {
        const store = useTerminalStore.getState()
        store.addSession("/ws/a", first)
        store.reset()

        expect(useTerminalStore.getState()).toMatchObject(terminalInitialState)
    })
})
