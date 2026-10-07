import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/ipc", () => ({
    clipboardReadFileList: vi.fn(async () => [] as string[]),
    clipboardWriteWorkspaceFiles: vi.fn(async () => undefined),
    fsCopyPaths: vi.fn(async () => [] as string[]),
    fsMovePaths: vi.fn(async () => [] as string[]),
    fsPasteClipboardFiles: vi.fn(async () => [] as string[])
}))
vi.mock("@/features/logs/userAction", () => ({ logUserAction: vi.fn(async () => undefined) }))
vi.mock("@/state/appDialogStore", () => ({ showAppMessage: vi.fn(async () => undefined) }))
vi.mock("@/state/contextMenuStore", () => ({ retargetOpenDocuments: vi.fn() }))
const tree = vi.hoisted(() => ({
    invalidatePaths: vi.fn(async () => undefined),
    toggleDir: vi.fn(async () => undefined),
    trees: {} as Record<string, { expandedDirs: Set<string> }>
}))
vi.mock("@/state/fileTreeStore", () => ({ useFileTreeStore: { getState: () => tree } }))

import * as ipc from "@/lib/ipc"
import { showAppMessage } from "@/state/appDialogStore"
import { retargetOpenDocuments } from "@/state/contextMenuStore"
import { useFileClipboardStore } from "@/state/fileClipboardStore"
import { remoteFilePath } from "@/lib/runtimeIdentity"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { copyFilesToClipboard, duplicatePath, pasteFiles, pasteTargetDir } from "./fileClipboard"

const W = "/w"

beforeEach(() => {
    useWorkspaceStore.setState({ workspacePath: W })
    tree.trees = { [W]: { expandedDirs: new Set() } }
})
afterEach(() => {
    useFileClipboardStore.setState({ clipboard: null, selection: null })
    vi.clearAllMocks()
})

describe("pasteTargetDir", () => {
    it("pastes into folders, next to files, or at the root", () => {
        expect(pasteTargetDir(W, { path: "/w/src", isDirectory: true })).toBe("/w/src")
        expect(pasteTargetDir(W, { path: "/w/src/a.ts", isDirectory: false })).toBe("/w/src")
        expect(pasteTargetDir(W, { path: "/w/a.ts", isDirectory: false })).toBe(W)
        expect(pasteTargetDir(W, null)).toBe(W)
    })
})

describe("copy, cut and paste", () => {
    it("copies on the OS clipboard too and pastes the app copy into the clicked folder", async () => {
        await copyFilesToClipboard(W, ["/w/a.ts"], "copy")
        expect(ipc.clipboardWriteWorkspaceFiles).toHaveBeenCalledWith(W, ["/w/a.ts"])
        vi.mocked(ipc.clipboardReadFileList).mockResolvedValueOnce(["/w/a.ts"])
        vi.mocked(ipc.fsCopyPaths).mockResolvedValueOnce(["/w/src/a.ts"])

        expect(await pasteFiles(W, { path: "/w/src", isDirectory: true })).toEqual(["/w/src/a.ts"])
        expect(ipc.fsCopyPaths).toHaveBeenCalledWith(W, ["/w/a.ts"], "/w/src")
        expect(tree.invalidatePaths).toHaveBeenCalledWith(W, ["/w/src/a.ts"])
        expect(tree.toggleDir).toHaveBeenCalledWith(W, "/w/src")
        // A copy can be pasted again.
        expect(useFileClipboardStore.getState().clipboard?.mode).toBe("copy")
    })

    it("moves a cut once, re-pointing open editors", async () => {
        await copyFilesToClipboard(W, ["/w/a.ts"], "cut")
        vi.mocked(ipc.fsMovePaths).mockResolvedValueOnce(["/w/src/a.ts"])
        await pasteFiles(W, { path: "/w/src/b.ts", isDirectory: false })

        expect(ipc.fsMovePaths).toHaveBeenCalledWith(W, ["/w/a.ts"], "/w/src")
        expect(retargetOpenDocuments).toHaveBeenCalledWith("/w/a.ts", "/w/src/a.ts")
        expect(tree.invalidatePaths).toHaveBeenCalledWith(W, ["/w/a.ts", "/w/src/a.ts"])
        expect(useFileClipboardStore.getState().clipboard).toBeNull()
    })

    it("pastes files copied in Finder once the OS clipboard no longer holds the app copy", async () => {
        await copyFilesToClipboard(W, ["/w/a.ts"], "copy")
        vi.mocked(ipc.clipboardReadFileList).mockResolvedValueOnce(["/Users/me/Desktop/photo.png"])
        vi.mocked(ipc.fsPasteClipboardFiles).mockResolvedValueOnce(["/w/photo.png"])

        expect(await pasteFiles(W, null)).toEqual(["/w/photo.png"])
        expect(ipc.fsPasteClipboardFiles).toHaveBeenCalledWith(W, W)
        expect(ipc.fsCopyPaths).not.toHaveBeenCalled()
    })

    it("treats another workspace's copy as external files", async () => {
        useFileClipboardStore.setState({ clipboard: { workspacePath: "/other", paths: ["/other/x.ts"], mode: "cut" } })
        vi.mocked(ipc.clipboardReadFileList).mockResolvedValueOnce(["/other/x.ts"])
        await pasteFiles(W, null)
        expect(ipc.fsPasteClipboardFiles).toHaveBeenCalledWith(W, W)
        expect(ipc.fsMovePaths).not.toHaveBeenCalled()
    })

    it("does nothing when neither clipboard holds files", async () => {
        expect(await pasteFiles(W, null)).toEqual([])
        expect(ipc.fsCopyPaths).not.toHaveBeenCalled()
        expect(ipc.fsPasteClipboardFiles).not.toHaveBeenCalled()
    })

    it("shows the entries a failed Finder paste already copied", async () => {
        vi.mocked(ipc.clipboardReadFileList).mockResolvedValueOnce(["/Users/me/a.png", "/Users/me/huge"])
        vi.mocked(ipc.fsPasteClipboardFiles).mockRejectedValueOnce("copy-limit-reached")
        expect(await pasteFiles(W, { path: "/w/src", isDirectory: true })).toEqual([])
        expect(tree.invalidatePaths).toHaveBeenCalledWith(W, ["/w/src"])
        expect(showAppMessage).toHaveBeenCalled()
    })

    it("explains backend refusals", async () => {
        await copyFilesToClipboard(W, ["/w/src"], "copy")
        vi.mocked(ipc.fsCopyPaths).mockRejectedValueOnce("copy-into-itself")
        expect(await pasteFiles(W, { path: "/w/src/inner", isDirectory: true })).toEqual([])
        expect(showAppMessage).toHaveBeenCalledWith(expect.objectContaining({
            description: "A folder cannot be copied into itself."
        }))
    })

    it("duplicates next to the original", async () => {
        vi.mocked(ipc.fsCopyPaths).mockResolvedValueOnce(["/w/src/a copy.ts"])
        expect(await duplicatePath(W, "/w/src/a.ts")).toEqual(["/w/src/a copy.ts"])
        expect(ipc.fsCopyPaths).toHaveBeenCalledWith(W, ["/w/src/a.ts"], "/w/src")
    })
})

describe("remote workspaces", () => {
    const R = remoteFilePath("host-1", "/srv/app")
    const A = remoteFilePath("host-1", "/srv/app/a.ts", "/srv/app")

    it("keep copies inside the app and never touch the OS clipboard", async () => {
        useWorkspaceStore.setState({ workspacePath: R })
        tree.trees = { [R]: { expandedDirs: new Set() } }
        await copyFilesToClipboard(R, [A], "copy")
        expect(ipc.clipboardWriteWorkspaceFiles).not.toHaveBeenCalled()
        await pasteFiles(R, null)
        expect(ipc.clipboardReadFileList).not.toHaveBeenCalled()
        expect(ipc.fsCopyPaths).toHaveBeenCalledWith(R, [A], R)
    })
})
