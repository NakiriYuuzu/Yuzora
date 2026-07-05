import { render, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import * as ipc from "../lib/ipc"
import { documentGeneration } from "../editor/documentRegistry"
import { useWorkspaceStore } from "../state/workspaceStore"
import { ExternalChangeBridge } from "./ExternalChangeBridge"

const PATH = "/w/a.ts"

// Capture the fs:external-change listener so a test can inject an event,
// mirroring externalChangeResolver.test.tsx.
let capturedFsListener: (e: { payload: string[] }) => void = () => {}
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
// the real one.
vi.mock("../lib/ipc", () => ({ openFile: vi.fn() }))

beforeEach(() => {
    vi.clearAllMocks()
    capturedFsListener = () => {}
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
    it("clears the external flag and bumps the generation after a successful reload", async () => {
        vi.mocked(ipc.openFile).mockResolvedValue({ kind: "full", content: "x", size: 1 })
        const gen0 = documentGeneration(PATH)
        render(<ExternalChangeBridge />)
        capturedFsListener({ payload: [PATH] })
        await waitFor(() => expect(flaggedTab()?.externallyModified).toBe(false))
        expect(documentGeneration(PATH)).toBe(gen0 + 1)
    })

    it("clears the external flag but leaves the generation untouched when the reload rejects (deleted file)", async () => {
        vi.mocked(ipc.openFile).mockRejectedValue(new Error("deleted"))
        const gen0 = documentGeneration(PATH)
        render(<ExternalChangeBridge />)
        capturedFsListener({ payload: [PATH] })
        // A .then-only chain would skip the settle on reject → the flag would stay
        // true; observing it flip to false proves the rejection is handled.
        await waitFor(() => expect(flaggedTab()?.externallyModified).toBe(false))
        // R3-F1: a failed reload must NOT bump the generation, so the keyed
        // EditorArea pane (and its unsaved buffer) is never remounted away.
        expect(documentGeneration(PATH)).toBe(gen0)
    })
})
