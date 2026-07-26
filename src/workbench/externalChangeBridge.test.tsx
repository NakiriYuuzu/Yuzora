import { render, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import * as ipc from "../lib/ipc"
import { documentGeneration } from "../editor/documentRegistry"
import { useFileTreeStore } from "../state/fileTreeStore"
import { useWorkspaceStore } from "../state/workspaceStore"
import { ExternalChangeBridge } from "./ExternalChangeBridge"

const PATH = "/w/a.ts"

// Capture the fs:external-change listener so a test can inject an event,
// mirroring externalChangeResolver.test.tsx.
let capturedFsListener: (e: {
    payload: { workspaceRoot: string; paths: string[] }
}) => void = () => {}
vi.mock("@tauri-apps/api/event", () => ({
    listen: vi.fn(async (_e: string, cb: unknown) => {
        capturedFsListener = cb as typeof capturedFsListener
        return () => {}
    })
}))

// Force a reload plan for PATH regardless of the planner's dirty/saveSuppress
// heuristics — this suite exercises only the reload-settle behaviour.
vi.mock("../lib/externalChange", () => ({
    handleExternalChange: vi.fn(() => ({ markModified: [], reload: [PATH] }))
}))

// Mock only the disk read, not documentRegistry: reloadDocument and
// documentGeneration run for real so the generation-bump timing under test is
// the real one. listDir backs fileTreeStore's precise invalidation (#59 T4b).
vi.mock("../lib/ipc", () => ({ openFile: vi.fn(), listDir: vi.fn() }))

beforeEach(() => {
    vi.clearAllMocks()
    capturedFsListener = () => {}
    // 事件過濾比對 live workspacePath（#57 T3）——測試裡的事件都掛在 /w 底下。
    useWorkspaceStore.setState({ workspacePath: "/w", treeRevision: 0 })
    useFileTreeStore.setState({ trees: {}, preciseRevision: null })
    useWorkspaceStore.getState().openTab(PATH)
    // Start flagged so the settle can be observed flipping it back to false.
    useWorkspaceStore.getState().markExternallyModified(PATH, true)
})

function flaggedTab() {
    return useWorkspaceStore
        .getState()
        .groups[0].tabs.find((t) => t.path === PATH)
}

describe("ExternalChangeBridge reload settling", () => {
    it("bumps the shared tree revision for every external filesystem event", () => {
        const revision = useWorkspaceStore.getState().treeRevision
        render(<ExternalChangeBridge />)
        capturedFsListener({ payload: { workspaceRoot: "/w", paths: [PATH] } })
        expect(useWorkspaceStore.getState().treeRevision).toBe(revision + 1)
    })

    // #57 T3 AC4：切換 gap 內舊 workspace watcher 的殘留事件不得打進新
    // workspace——樹不刷新、不嘗試 reload（openFile 不被呼叫）。
    it("drops events from a stale workspace root (#57 T3)", () => {
        const revision = useWorkspaceStore.getState().treeRevision
        render(<ExternalChangeBridge />)
        capturedFsListener({ payload: { workspaceRoot: "/old", paths: [PATH] } })
        expect(useWorkspaceStore.getState().treeRevision).toBe(revision)
        expect(ipc.openFile).not.toHaveBeenCalled()
        expect(ipc.listDir).not.toHaveBeenCalled()
        expect(flaggedTab()?.externallyModified).toBe(true)
    })

    // #59 T4b：外部變更走精準失效——只 re-list payload 對應的已快取目錄，
    // 展開狀態保留，且 marker 記下 bump 後的 revision（FileTree 據此跳過
    // 整樹 revalidate）。
    it("applies a precise invalidation to fileTreeStore instead of a full-tree refresh (#59 T4b)", async () => {
        const updated = [
            { name: "a.ts", path: "/w/src/a.ts", isDir: false },
            { name: "new.ts", path: "/w/src/new.ts", isDir: false }
        ]
        vi.mocked(ipc.listDir).mockResolvedValue(updated)
        useFileTreeStore.setState({
            trees: {
                "/w": {
                    rootNodes: [{ name: "src", path: "/w/src", isDir: true }],
                    childrenByDir: { "/w/src": [{ name: "a.ts", path: "/w/src/a.ts", isDir: false }] },
                    expandedDirs: new Set(["/w/src"]),
                    scrollTop: 0
                }
            }
        })
        render(<ExternalChangeBridge />)
        capturedFsListener({ payload: { workspaceRoot: "/w", paths: ["/w/src/new.ts"] } })
        await waitFor(() =>
            expect(useFileTreeStore.getState().trees["/w"]?.childrenByDir["/w/src"]).toEqual(updated)
        )
        // 只 re-list 受影響目錄（root 未被 re-list），展開狀態保留。
        expect(ipc.listDir).toHaveBeenCalledTimes(1)
        expect(ipc.listDir).toHaveBeenCalledWith("/w/src")
        expect(useFileTreeStore.getState().trees["/w"]?.expandedDirs.has("/w/src")).toBe(true)
        // treeRevision 照 bump（mention index 相容），且 marker 對應該 revision。
        expect(useWorkspaceStore.getState().treeRevision).toBe(1)
        expect(useFileTreeStore.getState().consumePreciseRevision("/w", 1)).toBe(true)
    })

    it("clears the external flag and bumps the generation after a successful reload", async () => {
        vi.mocked(ipc.openFile).mockResolvedValue({ kind: "full", content: "x", size: 1, lineEnding: "lf" })
        const gen0 = documentGeneration(PATH)
        render(<ExternalChangeBridge />)
        capturedFsListener({ payload: { workspaceRoot: "/w", paths: [PATH] } })
        await waitFor(() => expect(flaggedTab()?.externallyModified).toBe(false))
        expect(documentGeneration(PATH)).toBe(gen0 + 1)
    })

    it("clears the external flag but leaves the generation untouched when the reload rejects (deleted file)", async () => {
        vi.mocked(ipc.openFile).mockRejectedValue(new Error("deleted"))
        const gen0 = documentGeneration(PATH)
        render(<ExternalChangeBridge />)
        capturedFsListener({ payload: { workspaceRoot: "/w", paths: [PATH] } })
        // A .then-only chain would skip the settle on reject → the flag would stay
        // true; observing it flip to false proves the rejection is handled.
        await waitFor(() => expect(flaggedTab()?.externallyModified).toBe(false))
        // R3-F1: a failed reload must NOT bump the generation, so the keyed
        // EditorArea pane (and its unsaved buffer) is never remounted away.
        expect(documentGeneration(PATH)).toBe(gen0)
    })
})
