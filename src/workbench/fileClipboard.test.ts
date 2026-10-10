import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/ipc", () => ({
    clipboardReadFileList: vi.fn(async () => [] as string[]),
    clipboardWriteWorkspaceFiles: vi.fn(async () => undefined),
    fsCopyPaths: vi.fn(async () => [] as string[]),
    fsImportDroppedPaths: vi.fn(async () => [] as string[]),
    fsMovePaths: vi.fn(async () => [] as string[]),
    fsPasteClipboardFiles: vi.fn(async () => [] as string[]),
    wslImportOsFiles: vi.fn(async () => [] as string[])
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
import { useHostStore } from "@/state/hostStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { copyFilesToClipboard, duplicatePath, importDroppedFiles, moveFilesTo, pasteFiles, pasteTargetDir } from "./fileClipboard"

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

    it("explains that Finder files cannot be pasted instead of silently ignoring them", async () => {
        useWorkspaceStore.setState({ workspacePath: R })
        tree.trees = { [R]: { expandedDirs: new Set() } }
        vi.mocked(ipc.clipboardReadFileList).mockResolvedValueOnce(["/Users/me/a.png"])
        expect(await pasteFiles(R, null)).toEqual([])
        expect(ipc.fsPasteClipboardFiles).not.toHaveBeenCalled()
        expect(showAppMessage).toHaveBeenCalledWith(expect.objectContaining({
            description: expect.stringContaining("not supported yet")
        }))
    })

    it("stays quiet when a remote paste has nothing on either clipboard", async () => {
        useWorkspaceStore.setState({ workspacePath: R })
        expect(await pasteFiles(R, null)).toEqual([])
        expect(showAppMessage).not.toHaveBeenCalled()
    })

    it("refuses a Finder drop with the same explanation", async () => {
        useWorkspaceStore.setState({ workspacePath: R })
        expect(await importDroppedFiles(R, R, ["/Users/me/a.png"])).toEqual([])
        expect(ipc.fsImportDroppedPaths).not.toHaveBeenCalled()
        expect(showAppMessage).toHaveBeenCalledWith(expect.objectContaining({
            title: "Could not import",
            description: expect.stringContaining("not supported yet")
        }))
    })
})

describe("WSL host workspaces", () => {
    const WSL = remoteFilePath("wsl-1", "/home/me/app")
    const config = { hostId: "wsl-1", label: "Ubuntu", kind: "wsl" as const, distro: "Ubuntu", helper: "h", binary: "b" }
    beforeEach(() => {
        useHostStore.setState({ configs: { "wsl-1": config } })
        useWorkspaceStore.setState({ workspacePath: WSL })
        tree.trees = { [WSL]: { expandedDirs: new Set() } }
    })
    afterEach(() => useHostStore.setState({ configs: {} }))

    it("imports a Finder drop through the WSL command and selects the first new item", async () => {
        const created = [remoteFilePath("wsl-1", "/home/me/app/a.png", "/home/me/app")]
        vi.mocked(ipc.wslImportOsFiles).mockResolvedValueOnce(created)
        expect(await importDroppedFiles(WSL, WSL, ["C:\\Users\\me\\a.png"])).toEqual(created)
        expect(ipc.wslImportOsFiles).toHaveBeenCalledWith(WSL, "Ubuntu", WSL, ["C:\\Users\\me\\a.png"])
        expect(ipc.fsImportDroppedPaths).not.toHaveBeenCalled()
        expect(tree.invalidatePaths).toHaveBeenCalledWith(WSL, created)
        expect(useFileClipboardStore.getState().selection).toEqual({ workspacePath: WSL, path: created[0] })
    })

    it("pastes the OS clipboard through the WSL command, with a null path list", async () => {
        vi.mocked(ipc.clipboardReadFileList).mockResolvedValueOnce(["C:\\Users\\me\\a.png"])
        vi.mocked(ipc.wslImportOsFiles).mockResolvedValueOnce(["x"])
        expect(await pasteFiles(WSL, null)).toEqual(["x"])
        expect(ipc.wslImportOsFiles).toHaveBeenCalledWith(WSL, "Ubuntu", WSL, null)
        expect(ipc.fsPasteClipboardFiles).not.toHaveBeenCalled()
        expect(showAppMessage).not.toHaveBeenCalled()
    })

    it("lets a different OS clipboard win over an internal WSL copy", async () => {
        const A = remoteFilePath("wsl-1", "/home/me/app/a.ts", "/home/me/app")
        await copyFilesToClipboard(WSL, [A], "copy")
        vi.mocked(ipc.clipboardReadFileList).mockResolvedValueOnce(["C:\\Users\\me\\b.png"])
        vi.mocked(ipc.wslImportOsFiles).mockResolvedValueOnce(["x"])
        expect(await pasteFiles(WSL, null)).toEqual(["x"])
        expect(ipc.wslImportOsFiles).toHaveBeenCalledWith(WSL, "Ubuntu", WSL, null)
        expect(ipc.fsCopyPaths).not.toHaveBeenCalled()
    })

    it("keeps an internal WSL copy over an Explorer list that was already on the clipboard", async () => {
        const A = remoteFilePath("wsl-1", "/home/me/app/a.ts", "/home/me/app")
        vi.mocked(ipc.clipboardReadFileList).mockResolvedValueOnce(["C:\\Users\\me\\old.png"])
        await copyFilesToClipboard(WSL, [A], "copy")
        vi.mocked(ipc.clipboardReadFileList).mockResolvedValueOnce(["C:\\Users\\me\\old.png"])
        vi.mocked(ipc.fsCopyPaths).mockResolvedValueOnce([A])
        await pasteFiles(WSL, null)
        expect(ipc.fsCopyPaths).toHaveBeenCalledWith(WSL, [A], WSL)
        expect(ipc.wslImportOsFiles).not.toHaveBeenCalled()
    })

    it("records a WSL copy at once, so a paste before the OS snapshot lands uses it", async () => {
        const A = remoteFilePath("wsl-1", "/home/me/app/a.ts", "/home/me/app")
        let snapshot!: (paths: string[]) => void
        vi.mocked(ipc.clipboardReadFileList).mockReturnValueOnce(new Promise((resolve) => { snapshot = resolve }))
        const copying = copyFilesToClipboard(WSL, [A], "copy")
        expect(useFileClipboardStore.getState().clipboard?.paths).toEqual([A])
        vi.mocked(ipc.clipboardReadFileList).mockResolvedValueOnce(["C:\\Users\\me\\old.png"])
        vi.mocked(ipc.fsCopyPaths).mockResolvedValueOnce([A])
        await pasteFiles(WSL, null)
        expect(ipc.fsCopyPaths).toHaveBeenCalledWith(WSL, [A], WSL)
        expect(ipc.wslImportOsFiles).not.toHaveBeenCalled()
        snapshot(["C:\\Users\\me\\old.png"])
        await copying
    })

    it("keeps the later of two rapid WSL copies when their OS snapshots resolve out of order", async () => {
        const A = remoteFilePath("wsl-1", "/home/me/app/a.ts", "/home/me/app")
        const B = remoteFilePath("wsl-1", "/home/me/app/b.ts", "/home/me/app")
        let first!: (paths: string[]) => void
        vi.mocked(ipc.clipboardReadFileList).mockReturnValueOnce(new Promise((resolve) => { first = resolve }))
        const copyingA = copyFilesToClipboard(WSL, [A], "copy")
        vi.mocked(ipc.clipboardReadFileList).mockResolvedValueOnce([])
        await copyFilesToClipboard(WSL, [B], "copy")
        first([])
        await copyingA
        expect(useFileClipboardStore.getState().clipboard?.paths).toEqual([B])
    })

    it("maps WSL backend refusals to readable messages", async () => {
        vi.mocked(ipc.wslImportOsFiles).mockRejectedValueOnce("wsl-helper-outdated")
        expect(await importDroppedFiles(WSL, WSL, ["C:\\a.png"])).toEqual([])
        expect(showAppMessage).toHaveBeenCalledWith(expect.objectContaining({
            description: expect.stringContaining("helper is out of date")
        }))
    })

    it("maps the other WSL import refusals to readable messages, not raw codes", async () => {
        for (const code of ["wsl-runtime-disabled-open-settings", "import-source-invalid", "wsl-path-conversion-failed", "wsl-identity-changed-or-not-wsl2", "host-not-wsl-distribution"]) {
            vi.mocked(showAppMessage).mockClear()
            vi.mocked(ipc.wslImportOsFiles).mockRejectedValueOnce(`Error: ${code}`)
            expect(await importDroppedFiles(WSL, WSL, ["C:\\a.png"])).toEqual([])
            const description = vi.mocked(showAppMessage).mock.calls[0][0].description as string
            expect(description).not.toContain(code)
            expect(description).not.toBe("")
        }
    })

    it("still refuses SSH hosts that merely share the remote URI scheme", async () => {
        const SSH = remoteFilePath("ssh-1", "/srv/app")
        useHostStore.setState({ configs: { "ssh-1": { ...config, hostId: "ssh-1", kind: "ssh", distro: undefined } } })
        useWorkspaceStore.setState({ workspacePath: SSH })
        expect(await importDroppedFiles(SSH, SSH, ["/Users/me/a.png"])).toEqual([])
        expect(ipc.wslImportOsFiles).not.toHaveBeenCalled()
        expect(showAppMessage).toHaveBeenCalledWith(expect.objectContaining({
            description: expect.stringContaining("SSH remote workspace")
        }))
    })
})

describe("importDroppedFiles", () => {
    it("imports into the target folder, expands it and selects the first new item", async () => {
        vi.mocked(ipc.fsImportDroppedPaths).mockResolvedValueOnce(["/w/src/a.png", "/w/src/b.png"])
        expect(await importDroppedFiles(W, "/w/src", ["/Users/me/a.png", "/Users/me/b.png"])).toEqual(["/w/src/a.png", "/w/src/b.png"])
        expect(ipc.fsImportDroppedPaths).toHaveBeenCalledWith(W, ["/Users/me/a.png", "/Users/me/b.png"], "/w/src")
        expect(tree.invalidatePaths).toHaveBeenCalledWith(W, ["/w/src/a.png", "/w/src/b.png"])
        expect(tree.toggleDir).toHaveBeenCalledWith(W, "/w/src")
        expect(useFileClipboardStore.getState().selection).toEqual({ workspacePath: W, path: "/w/src/a.png" })
    })

    it("does not toggle the workspace root and ignores empty or stale requests", async () => {
        vi.mocked(ipc.fsImportDroppedPaths).mockResolvedValueOnce(["/w/a.png"])
        await importDroppedFiles(W, W, ["/Users/me/a.png"])
        expect(tree.toggleDir).not.toHaveBeenCalled()
        expect(await importDroppedFiles(W, W, [])).toEqual([])
        useWorkspaceStore.setState({ workspacePath: "/other" })
        expect(await importDroppedFiles(W, W, ["/Users/me/a.png"])).toEqual([])
        expect(ipc.fsImportDroppedPaths).toHaveBeenCalledTimes(1)
    })

    it("reports backend refusals and refreshes the target", async () => {
        vi.mocked(ipc.fsImportDroppedPaths).mockRejectedValueOnce("dropped-paths-stale")
        expect(await importDroppedFiles(W, "/w/src", ["/Users/me/a.png"])).toEqual([])
        expect(tree.invalidatePaths).toHaveBeenCalledWith(W, ["/w/src"])
        expect(showAppMessage).toHaveBeenCalledWith(expect.objectContaining({
            title: "Could not import",
            description: "The dropped files expired. Drag them in again."
        }))
    })
})

describe("moveFilesTo", () => {
    it("moves into the folder, re-points open editors and leaves the clipboard alone", async () => {
        useFileClipboardStore.setState({ clipboard: { workspacePath: W, paths: ["/w/keep.ts"], mode: "copy" } })
        vi.mocked(ipc.fsMovePaths).mockResolvedValueOnce(["/w/src/a.ts"])

        expect(await moveFilesTo(W, ["/w/a.ts"], "/w/src")).toEqual(["/w/src/a.ts"])
        expect(ipc.fsMovePaths).toHaveBeenCalledWith(W, ["/w/a.ts"], "/w/src")
        expect(retargetOpenDocuments).toHaveBeenCalledWith("/w/a.ts", "/w/src/a.ts")
        expect(tree.invalidatePaths).toHaveBeenCalledWith(W, ["/w/a.ts", "/w/src/a.ts"])
        expect(tree.toggleDir).toHaveBeenCalledWith(W, "/w/src")
        expect(useFileClipboardStore.getState().clipboard).toEqual({ workspacePath: W, paths: ["/w/keep.ts"], mode: "copy" })
        expect(ipc.clipboardReadFileList).not.toHaveBeenCalled()
    })

    it("does nothing once the workspace changed", async () => {
        useWorkspaceStore.setState({ workspacePath: "/other" })
        expect(await moveFilesTo(W, ["/w/a.ts"], "/w/src")).toEqual([])
        expect(ipc.fsMovePaths).not.toHaveBeenCalled()
    })

    it("reports a refused move and refreshes what it touched", async () => {
        vi.mocked(ipc.fsMovePaths).mockRejectedValueOnce("move-into-itself")
        expect(await moveFilesTo(W, ["/w/src"], "/w/src/inner")).toEqual([])
        expect(tree.invalidatePaths).toHaveBeenCalledWith(W, ["/w/src/inner", "/w/src"])
        expect(showAppMessage).toHaveBeenCalledWith(expect.objectContaining({
            title: "Could not move",
            description: "A folder cannot be moved into itself."
        }))
    })
})
