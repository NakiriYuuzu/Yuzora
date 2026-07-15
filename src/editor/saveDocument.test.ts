import { beforeEach, describe, expect, it, vi } from "vitest"

const getDocument = vi.fn()
const getView = vi.fn()
const saveFile = vi.fn(async (_path: string, _content: string) => 0)
const recentlySavedMark = vi.fn()
const showActionError = vi.fn(async (_action: string, _error: unknown) => undefined)

vi.mock("./documentRegistry", () => ({
    getDocument: (path: string) => getDocument(path)
}))
vi.mock("./viewRegistry", () => ({
    getView: (path: string) => getView(path)
}))
vi.mock("../lib/ipc", () => ({
    saveFile: (path: string, content: string) => saveFile(path, content)
}))
vi.mock("../lib/saveSuppress", () => ({
    recentlySaved: { mark: (path: string) => recentlySavedMark(path) }
}))
vi.mock("../lib/actionFeedback", () => ({
    showActionError: (action: string, error: unknown) => showActionError(action, error)
}))

import { saveDirtyTab } from "./saveDocument"
import { useWorkspaceStore } from "../state/workspaceStore"

const PATH = "/w/a.ts"

function seedTab(lineEnding: "lf" | "crlf" | "mixed", dirty = true) {
    useWorkspaceStore.setState({
        workspacePath: "/w",
        activeGroupIndex: 0,
        groups: [{
            activePath: PATH,
            tabs: [{
                path: PATH,
                name: "a.ts",
                dirty,
                externallyModified: true,
                lineEnding
            }]
        }]
    })
}

beforeEach(() => {
    vi.clearAllMocks()
    getView.mockReturnValue({ state: { doc: { toString: () => "one\ntwo\n" } } })
    getDocument.mockResolvedValue({
        result: { kind: "full", content: "one\ntwo\n", size: 8, lineEnding: "lf" }
    })
    saveFile.mockResolvedValue(123)
    seedTab("lf")
})

describe("saveDirtyTab", () => {
    it("serializes the live normalized editor buffer to CRLF before I/O", async () => {
        seedTab("crlf")
        getDocument.mockResolvedValue({
            result: { kind: "full", content: "one\r\ntwo\r\n", size: 10, lineEnding: "crlf" }
        })

        await expect(saveDirtyTab(PATH)).resolves.toEqual({ kind: "saved" })

        expect(saveFile).toHaveBeenCalledWith(PATH, "one\r\ntwo\r\n")
        expect(recentlySavedMark).toHaveBeenCalledWith(PATH)
        expect(useWorkspaceStore.getState().groups[0].tabs[0]).toMatchObject({
            dirty: false,
            externallyModified: false
        })
    })

    it("uses the mirrored registry buffer when the tab has no live view", async () => {
        getView.mockReturnValue(undefined)
        getDocument.mockResolvedValue({
            result: { kind: "full", content: "disk\nmirror\n", size: 12, lineEnding: "lf" }
        })

        await saveDirtyTab(PATH)

        expect(saveFile).toHaveBeenCalledWith(PATH, "disk\nmirror\n")
    })

    it("blocks Mixed before recentlySaved or I/O and keeps the tab dirty", async () => {
        seedTab("mixed")
        getDocument.mockResolvedValue({
            result: { kind: "full", content: "one\r\ntwo\n", size: 9, lineEnding: "mixed" }
        })

        await expect(saveDirtyTab(PATH)).resolves.toEqual({ kind: "blocked", reason: "mixed" })

        expect(recentlySavedMark).not.toHaveBeenCalled()
        expect(saveFile).not.toHaveBeenCalled()
        expect(showActionError).toHaveBeenCalledTimes(1)
        expect(useWorkspaceStore.getState().groups[0].tabs[0].dirty).toBe(true)
    })

    it("lets an explicit target override detected Mixed and completes the conversion", async () => {
        seedTab("mixed")
        getDocument.mockResolvedValue({
            result: { kind: "full", content: "one\r\ntwo\n", size: 9, lineEnding: "mixed" }
        })
        useWorkspaceStore.getState().setLineEnding(PATH, "crlf")

        await expect(saveDirtyTab(PATH)).resolves.toEqual({ kind: "saved" })

        expect(saveFile).toHaveBeenCalledWith(PATH, "one\r\ntwo\r\n")
        expect(useWorkspaceStore.getState().groups[0].tabs[0].dirty).toBe(false)
    })

    it("reports I/O failure as a typed outcome and keeps dirty state", async () => {
        saveFile.mockRejectedValue(new Error("disk full"))

        await expect(saveDirtyTab(PATH)).resolves.toEqual({ kind: "failed" })

        expect(recentlySavedMark).toHaveBeenCalledWith(PATH)
        expect(showActionError).toHaveBeenCalledTimes(1)
        expect(useWorkspaceStore.getState().groups[0].tabs[0].dirty).toBe(true)
    })

    it("reports an openFile failure as a typed outcome when no live buffer remains", async () => {
        getView.mockReturnValue(undefined)
        getDocument.mockRejectedValue(new Error("not found"))

        await expect(saveDirtyTab(PATH)).resolves.toEqual({ kind: "failed" })

        expect(showActionError).toHaveBeenCalledTimes(1)
        expect(saveFile).not.toHaveBeenCalled()
        expect(useWorkspaceStore.getState().groups[0].tabs[0].dirty).toBe(true)
    })

    it("uses the live buffer to recreate a file after its external reload failed", async () => {
        getDocument.mockRejectedValue(new Error("not found"))
        getView.mockReturnValue({ state: { doc: { toString: () => "recreated\n" } } })

        await expect(saveDirtyTab(PATH)).resolves.toEqual({ kind: "saved" })

        expect(getDocument).not.toHaveBeenCalled()
        expect(saveFile).toHaveBeenCalledWith(PATH, "recreated\n")
        expect(showActionError).not.toHaveBeenCalled()
        expect(useWorkspaceStore.getState().groups[0].tabs[0].dirty).toBe(false)
    })

    it("does not save a live readonly view after a newer generation clears metadata", async () => {
        useWorkspaceStore.getState().hydrateLineEnding(PATH, undefined, 1)
        getDocument.mockResolvedValue({
            result: {
                kind: "nonUtf8Readonly",
                content: "legacy",
                encoding: "windows-1252",
                size: 6
            }
        })

        await expect(saveDirtyTab(PATH)).resolves.toEqual({ kind: "notEditable" })

        expect(saveFile).not.toHaveBeenCalled()
        expect(useWorkspaceStore.getState().groups[0].tabs[0].dirty).toBe(true)
    })
})
