import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import * as ipc from "../lib/ipc"
import { registerView } from "../editor/viewRegistry"
import { documentGeneration } from "../editor/documentRegistry"
import { useUiStore } from "../state/uiStore"
import { useWorkspaceStore } from "../state/workspaceStore"
import { ExternalChangeResolver, maybeInterceptSave } from "./ExternalChangeResolver"

vi.mock("../lib/ipc", () => ({
    openFile: vi.fn(),
    saveFile: vi.fn(async () => 0)
}))

vi.mock("@/features/logs/userAction", () => ({
    logUserAction: vi.fn(async () => undefined)
}))

// Capture the fs:external-change listener callback so tests can inject events,
// mirroring the pattern in askpassHost.test.tsx.
let capturedFsListener: (e: { payload: string[] }) => void = () => {}
vi.mock("@tauri-apps/api/event", () => ({
    listen: vi.fn(async (_e: string, cb: unknown) => {
        capturedFsListener = cb as typeof capturedFsListener
        return () => {}
    })
}))

const PATH = "/w/a.ts"

function mountMainView(doc: string): EditorView {
    const view = new EditorView({ state: EditorState.create({ doc }), parent: document.body })
    registerView(PATH, view)
    return view
}

beforeEach(() => {
    vi.clearAllMocks()
    capturedFsListener = () => {}
})

describe("maybeInterceptSave", () => {
    it("opens resolver only when tab is externally modified", () => {
        useWorkspaceStore.getState().openTab(PATH)
        useWorkspaceStore.getState().markExternallyModified(PATH, true)
        expect(maybeInterceptSave(PATH)).toBe(true)
        expect(useUiStore.getState().resolverPath).toBe(PATH)
        useUiStore.getState().closeResolver()
        useWorkspaceStore.getState().markExternallyModified(PATH, false)
        expect(maybeInterceptSave(PATH)).toBe(false)
        expect(useUiStore.getState().resolverPath).toBe(null)
    })
})

describe("ExternalChangeResolver", () => {
    it("take-disk then resolve-and-save writes disk text and clears state", async () => {
        const main = mountMainView("mine")
        vi.mocked(ipc.openFile).mockResolvedValue({ kind: "full", content: "disk", size: 4 })
        useWorkspaceStore.getState().openTab(PATH)
        useWorkspaceStore.getState().markExternallyModified(PATH, true)
        useUiStore.getState().openResolver(PATH)
        render(<ExternalChangeResolver />)
        fireEvent.click(await screen.findByRole("button", { name: "全部採用磁碟版" }))
        fireEvent.click(screen.getByRole("button", { name: "解決並存檔" }))
        await waitFor(() => expect(ipc.saveFile).toHaveBeenCalledWith(PATH, "disk"))
        expect(main.state.doc.toString()).toBe("disk")
        expect(useUiStore.getState().resolverPath).toBe(null)
        const tab = useWorkspaceStore.getState().groups[0].tabs.find((t) => t.path === PATH)
        expect(tab?.externallyModified).toBe(false)
    })

    it("cancel keeps buffer and flags untouched", async () => {
        const main = mountMainView("mine")
        vi.mocked(ipc.openFile).mockResolvedValue({ kind: "full", content: "disk", size: 4 })
        useWorkspaceStore.getState().openTab(PATH)
        useWorkspaceStore.getState().markExternallyModified(PATH, true)
        useUiStore.getState().openResolver(PATH)
        render(<ExternalChangeResolver />)
        fireEvent.click(await screen.findByRole("button", { name: "取消" }))
        expect(ipc.saveFile).not.toHaveBeenCalled()
        expect(main.state.doc.toString()).toBe("mine")
        const tab = useWorkspaceStore.getState().groups[0].tabs.find((t) => t.path === PATH)
        expect(tab?.externallyModified).toBe(true)
    })

    it("deleted-on-disk falls back to two-option mode", async () => {
        mountMainView("mine")
        vi.mocked(ipc.openFile).mockRejectedValue("not found")
        useUiStore.getState().openResolver(PATH)
        const { container } = render(<ExternalChangeResolver />)
        expect(await screen.findByRole("button", { name: "保留我的（覆寫存檔）" })).toBeInTheDocument()
        expect(screen.getByRole("button", { name: "丟棄並關閉分頁" })).toBeInTheDocument()
        expect(container.querySelector(".cm-editor")).toBeNull()
    })

    // Finding #1: keepAll pulls the merge view's `original` up to the buffer, so
    // a subsequent takeDisk that reads getOriginalDoc would silently write the
    // buffer instead of the disk content. takeDisk must use the immutable disk
    // ref captured at open time.
    it("keep-all then take-disk still resolves to disk text", async () => {
        const main = mountMainView("mine")
        vi.mocked(ipc.openFile).mockResolvedValue({ kind: "full", content: "disk", size: 4 })
        useWorkspaceStore.getState().openTab(PATH)
        useWorkspaceStore.getState().markExternallyModified(PATH, true)
        useUiStore.getState().openResolver(PATH)
        render(<ExternalChangeResolver />)
        fireEvent.click(await screen.findByRole("button", { name: "全部保留我的" }))
        fireEvent.click(screen.getByRole("button", { name: "全部採用磁碟版" }))
        fireEvent.click(screen.getByRole("button", { name: "解決並存檔" }))
        await waitFor(() => expect(ipc.saveFile).toHaveBeenCalledWith(PATH, "disk"))
        expect(main.state.doc.toString()).toBe("disk")
        expect(useUiStore.getState().resolverPath).toBe(null)
    })

    // Finding #3: keepAll path had zero runtime assertions. Buffer differs from
    // disk -> 全部保留我的 -> resolve-and-save writes the buffer verbatim.
    it("keep-all then resolve-and-save writes buffer text", async () => {
        const main = mountMainView("mine")
        vi.mocked(ipc.openFile).mockResolvedValue({ kind: "full", content: "disk", size: 4 })
        useWorkspaceStore.getState().openTab(PATH)
        useWorkspaceStore.getState().markExternallyModified(PATH, true)
        useUiStore.getState().openResolver(PATH)
        render(<ExternalChangeResolver />)
        fireEvent.click(await screen.findByRole("button", { name: "全部保留我的" }))
        fireEvent.click(screen.getByRole("button", { name: "解決並存檔" }))
        await waitFor(() => expect(ipc.saveFile).toHaveBeenCalledWith(PATH, "mine"))
        expect(main.state.doc.toString()).toBe("mine")
        const tab = useWorkspaceStore.getState().groups[0].tabs.find((t) => t.path === PATH)
        expect(tab?.externallyModified).toBe(false)
    })

    // Finding #2: saveFile failure must not silently swallow errors. The
    // resolver stays open (resolverPath unchanged), externallyModified stays
    // true, and an error message appears so the user can retry or cancel.
    it("save failure keeps resolver open and shows an error", async () => {
        mountMainView("mine")
        vi.mocked(ipc.openFile).mockResolvedValue({ kind: "full", content: "disk", size: 4 })
        vi.mocked(ipc.saveFile).mockRejectedValue(new Error("disk full"))
        useWorkspaceStore.getState().openTab(PATH)
        useWorkspaceStore.getState().markExternallyModified(PATH, true)
        useUiStore.getState().openResolver(PATH)
        render(<ExternalChangeResolver />)
        fireEvent.click(await screen.findByRole("button", { name: "全部採用磁碟版" }))
        fireEvent.click(screen.getByRole("button", { name: "解決並存檔" }))
        await waitFor(() => expect(ipc.saveFile).toHaveBeenCalled())
        expect(await screen.findByText(/存檔失敗/)).toBeInTheDocument()
        expect(useUiStore.getState().resolverPath).toBe(PATH)
        const tab = useWorkspaceStore.getState().groups[0].tabs.find((t) => t.path === PATH)
        expect(tab?.externallyModified).toBe(true)
    })

    // m5: the degraded "採用磁碟版（重新載入）" path reloads from disk and must
    // clear the dirty flag too — otherwise the tab stays marked dirty even though
    // its buffer now matches disk.
    it("degraded take-disk-reload clears the dirty flag", async () => {
        mountMainView("mine")
        // Disk load is binary → resolver shows the two-option degraded mode.
        vi.mocked(ipc.openFile).mockResolvedValue({ kind: "binary", size: 10 })
        useWorkspaceStore.getState().openTab(PATH)
        useWorkspaceStore.getState().markDirty(PATH, true)
        useWorkspaceStore.getState().markExternallyModified(PATH, true)
        useUiStore.getState().openResolver(PATH)
        render(<ExternalChangeResolver />)
        fireEvent.click(await screen.findByRole("button", { name: "採用磁碟版（重新載入）" }))
        await waitFor(() => {
            const tab = useWorkspaceStore.getState().groups[0].tabs.find((t) => t.path === PATH)
            expect(tab?.dirty).toBe(false)
        })
        const tab = useWorkspaceStore.getState().groups[0].tabs.find((t) => t.path === PATH)
        expect(tab?.externallyModified).toBe(false)
    })

    // R2B-F1: the file can be deleted between opening the resolver (disk read
    // was binary → degraded reload button shown) and clicking reload. The reload
    // then rejects; the dialog must still settle — close, clear the external flag
    // — instead of hanging, and keep dirty so a re-save can recreate the file.
    it("degraded take-disk-reload settles (closes) when the reload rejects mid-flight", async () => {
        mountMainView("mine")
        // Open sees a binary disk → degraded two-option mode with the reload
        // button; the reload's fresh read then fails (file deleted meanwhile).
        vi.mocked(ipc.openFile)
            .mockResolvedValueOnce({ kind: "binary", size: 10 })
            .mockRejectedValue("not found")
        useWorkspaceStore.getState().openTab(PATH)
        useWorkspaceStore.getState().markDirty(PATH, true)
        useWorkspaceStore.getState().markExternallyModified(PATH, true)
        useUiStore.getState().openResolver(PATH)
        render(<ExternalChangeResolver />)
        const reloadBtn = await screen.findByRole("button", { name: "採用磁碟版（重新載入）" })
        const genBefore = documentGeneration(PATH)
        fireEvent.click(reloadBtn)
        await waitFor(() => expect(useUiStore.getState().resolverPath).toBe(null))
        const tab = useWorkspaceStore.getState().groups[0].tabs.find((t) => t.path === PATH)
        expect(tab?.externallyModified).toBe(false)
        // Buffer still differs from the (absent) disk → keep dirty for a re-save.
        expect(tab?.dirty).toBe(true)
        // R3-F1: the failed reload must leave the generation untouched, so the
        // keyed EditorArea pane (and its unsaved buffer) is never remounted away.
        expect(documentGeneration(PATH)).toBe(genBefore)
    })

    // Finding #3: fs:external-change rebuild path. Injecting a disk-rechange
    // event for this path surfaces the "磁碟版已再次變更" hint.
    it("fs:external-change for this path shows the re-changed hint", async () => {
        mountMainView("mine")
        vi.mocked(ipc.openFile).mockResolvedValue({ kind: "full", content: "disk", size: 4 })
        useWorkspaceStore.getState().openTab(PATH)
        useWorkspaceStore.getState().markExternallyModified(PATH, true)
        useUiStore.getState().openResolver(PATH)
        render(<ExternalChangeResolver />)
        await screen.findByRole("button", { name: "解決並存檔" })
        vi.mocked(ipc.openFile).mockResolvedValue({ kind: "full", content: "disk2", size: 5 })
        capturedFsListener({ payload: [PATH] })
        expect(await screen.findByText("磁碟版已再次變更")).toBeInTheDocument()
    })
})
