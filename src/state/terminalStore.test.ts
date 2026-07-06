import { beforeEach, describe, expect, it } from "vitest"

import { terminalInitialState, useTerminalStore } from "./terminalStore"

const first = {
    sessionId: "term-1",
    title: "zsh",
    workspace: "/ws/a",
    shell: "/bin/zsh",
    cols: 120,
    rows: 30
}

const second = {
    sessionId: "term-2",
    title: "node",
    workspace: "/ws/a",
    shell: "/bin/zsh",
    cols: 100,
    rows: 24
}

const otherWorkspace = {
    sessionId: "term-3",
    title: "other",
    workspace: "/ws/b",
    shell: "/bin/zsh",
    cols: 80,
    rows: 24
}

beforeEach(() => useTerminalStore.getState().reset())

describe("useTerminalStore", () => {
    it("adds sessions as panes and marks the new pane active", () => {
        const s = useTerminalStore.getState()

        s.addSession("/ws/a", first)

        const state = useTerminalStore.getState()
        expect(state.sessions[first.sessionId]).toEqual(first)
        expect(state.layouts["/ws/a"]).toEqual({
            panes: [{ paneId: first.sessionId, sessionId: first.sessionId }],
            activePaneId: first.sessionId,
            splitDirection: null
        })
    })

    it("splits from a pane once and caps the workspace layout at two panes", () => {
        const s = useTerminalStore.getState()
        s.addSession("/ws/a", first, "pane-a")

        s.splitFrom("/ws/a", "pane-a", second, "down")
        s.splitFrom("/ws/a", "pane-a", otherWorkspace)

        const state = useTerminalStore.getState()
        expect(state.layouts["/ws/a"].panes).toEqual([
            { paneId: "pane-a", sessionId: first.sessionId },
            { paneId: second.sessionId, sessionId: second.sessionId }
        ])
        expect(state.sessions[second.sessionId]).toEqual(second)
        expect(state.sessions[otherWorkspace.sessionId]).toBeUndefined()
        expect(state.layouts["/ws/a"].activePaneId).toBe(second.sessionId)
        expect(state.layouts["/ws/a"].splitDirection).toBe("down")
    })

    it("applies the split cap per workspace", () => {
        const s = useTerminalStore.getState()
        s.addSession("/ws/a", first, "pane-a")
        s.splitFrom("/ws/a", "pane-a", second)

        s.addSession("/ws/b", otherWorkspace, "pane-b")
        s.splitFrom("/ws/b", "pane-b", { ...otherWorkspace, sessionId: "term-4", title: "other split" })

        const state = useTerminalStore.getState()
        expect(state.layouts["/ws/a"].panes).toHaveLength(2)
        expect(state.layouts["/ws/b"].panes).toEqual([
            { paneId: "pane-b", sessionId: otherWorkspace.sessionId },
            { paneId: "term-4", sessionId: "term-4" }
        ])
    })

    it("removes a session and reassigns the active pane within the same workspace", () => {
        const s = useTerminalStore.getState()
        s.addSession("/ws/a", first, "pane-a")
        s.splitFrom("/ws/a", "pane-a", second)
        s.addSession("/ws/b", otherWorkspace, "pane-b")

        s.removeSession("/ws/a", second.sessionId)

        const state = useTerminalStore.getState()
        expect(state.sessions[second.sessionId]).toBeUndefined()
        expect(state.layouts["/ws/a"]).toEqual({
            panes: [{ paneId: "pane-a", sessionId: first.sessionId }],
            activePaneId: "pane-a",
            splitDirection: null
        })
        expect(state.layouts["/ws/b"]).toEqual({
            panes: [{ paneId: "pane-b", sessionId: otherWorkspace.sessionId }],
            activePaneId: "pane-b",
            splitDirection: null
        })
    })

    it("clears the active pane when the last session is removed", () => {
        const s = useTerminalStore.getState()
        s.addSession("/ws/a", first, "pane-a")

        s.removeSession("/ws/a", first.sessionId)

        const state = useTerminalStore.getState()
        expect(state.sessions).toEqual({})
        expect(state.layouts["/ws/a"]).toEqual({
            panes: [],
            activePaneId: null,
            splitDirection: null
        })
    })

    it("keeps sessions isolated by workspace", () => {
        const s = useTerminalStore.getState()
        s.addSession("/ws/a", first)
        s.addSession("/ws/b", otherWorkspace)

        const aSessions = useTerminalStore.getState().sessionsForWorkspace("/ws/a")
        const bSessions = useTerminalStore.getState().sessionsForWorkspace("/ws/b")

        expect(aSessions).toEqual([first])
        expect(bSessions).toEqual([otherWorkspace])
    })

    it("reset restores the exported initial state", () => {
        const s = useTerminalStore.getState()
        s.addSession("/ws/a", first)

        s.reset()

        expect(useTerminalStore.getState()).toMatchObject(terminalInitialState)
    })
})
