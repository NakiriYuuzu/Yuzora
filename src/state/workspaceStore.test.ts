import { beforeEach, describe, expect, it } from "vitest"
import { useWorkspaceStore } from "./workspaceStore"

const initialState = useWorkspaceStore.getState()

describe("workspaceStore", () => {
    beforeEach(() => {
        useWorkspaceStore.setState(initialState, true)
    })

    describe("setActiveGroup", () => {
        it("splitRight 後 setActiveGroup(1) 讓 openTab 開進 groups[1]", () => {
            useWorkspaceStore.getState().splitRight()
            useWorkspaceStore.getState().setActiveGroup(1)
            useWorkspaceStore.getState().openTab("/w/a.ts")

            const state = useWorkspaceStore.getState()
            expect(state.activeGroupIndex).toBe(1)
            expect(state.groups[1].tabs.map((t) => t.path)).toEqual(["/w/a.ts"])
            expect(state.groups[1].activePath).toBe("/w/a.ts")
            expect(state.groups[0].tabs).toEqual([])
        })

        it("越界 index 不變更 activeGroupIndex", () => {
            useWorkspaceStore.getState().setActiveGroup(99)

            expect(useWorkspaceStore.getState().activeGroupIndex).toBe(0)
        })
    })

    describe("requestReveal", () => {
        it("requestReveal opens tab and stores pending line", () => {
            useWorkspaceStore.getState().requestReveal("/w/a.ts", 42)
            const s = useWorkspaceStore.getState()
            expect(s.groups[s.activeGroupIndex].activePath).toBe("/w/a.ts")
            expect(s.pendingReveal).toEqual({ path: "/w/a.ts", line: 42 })
            useWorkspaceStore.getState().consumeReveal()
            expect(useWorkspaceStore.getState().pendingReveal).toBe(null)
        })
    })
})
