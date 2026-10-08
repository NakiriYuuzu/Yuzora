import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import * as ipc from "../lib/ipc"
import { registerView } from "../editor/viewRegistry"
import { documentGeneration } from "../editor/documentRegistry"
import { useUiStore } from "../state/uiStore"
import { useWorkspaceStore } from "../state/workspaceStore"
import { ExternalChangeResolver, maybeInterceptSave } from "./ExternalChangeResolver"

const showActionError = vi.fn(async (_action: string, _error: unknown) => undefined)

vi.mock("../lib/ipc", () => {
    const openFile = vi.fn()
    return {
        openFile,
        openFileSnapshot: async (path: string) => ({ result: await openFile(path), accept: () => {} }),
        saveFile: vi.fn(async () => 0)
    }
})

vi.mock("../lib/actionFeedback", () => ({
    showActionError: (action: string, error: unknown) => showActionError(action, error)
}))

vi.mock("@/features/logs/userAction", () => ({
    logUserAction: vi.fn(async () => undefined)
}))

// Capture the fs:external-change listener callback so tests can inject events,
// mirroring the pattern in askpassHost.test.tsx.
let capturedFsListener: (e: {
    payload: { workspaceRoot: string; paths: string[] }
}) => void = () => {}
vi.mock("@tauri-apps/api/event", () => ({
    listen: vi.fn(async (_e: string, cb: unknown) => {
        capturedFsListener = cb as typeof capturedFsListener
        return () => {}
    })
}))

const PATH = "/w/a.ts"
const initialWorkspaceState = useWorkspaceStore.getState()
const initialUiState = useUiStore.getState()

function mountMainView(doc: string, path = PATH): EditorView {
    const view = new EditorView({ state: EditorState.create({ doc }), parent: document.body })
    registerView(path, view)
    return view
}

beforeEach(() => {
    useWorkspaceStore.setState(initialWorkspaceState, true)
    useUiStore.setState(initialUiState, true)
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
    it("shows an extended Windows path without the raw prefix while loading the raw target", async () => {
        const rawPath = String.raw`\\?\C:\Work\中文 workspace\a.ts`
        const displayPath = String.raw`C:\Work\中文 workspace\a.ts`
        mountMainView("mine", rawPath)
        vi.mocked(ipc.openFile).mockResolvedValue({ kind: "full", content: "disk", size: 4, lineEnding: "lf" })
        useUiStore.getState().openResolver(rawPath)

        render(<ExternalChangeResolver />)

        expect(await screen.findByText(displayPath)).toBeInTheDocument()
        expect(screen.queryByText(rawPath)).not.toBeInTheDocument()
        expect(ipc.openFile).toHaveBeenCalledWith(rawPath)
    })

    it("take-disk then resolve-and-save writes disk text and clears state", async () => {
        const main = mountMainView("mine")
        vi.mocked(ipc.openFile).mockResolvedValue({ kind: "full", content: "disk", size: 4, lineEnding: "lf" })
        useWorkspaceStore.getState().openTab(PATH)
        useWorkspaceStore.getState().hydrateLineEnding(PATH, "lf", documentGeneration(PATH))
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
        vi.mocked(ipc.openFile).mockResolvedValue({ kind: "full", content: "disk", size: 4, lineEnding: "lf" })
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
        vi.mocked(ipc.openFile).mockResolvedValue({ kind: "full", content: "disk", size: 4, lineEnding: "lf" })
        useWorkspaceStore.getState().openTab(PATH)
        useWorkspaceStore.getState().hydrateLineEnding(PATH, "lf", documentGeneration(PATH))
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
        vi.mocked(ipc.openFile).mockResolvedValue({ kind: "full", content: "disk", size: 4, lineEnding: "lf" })
        useWorkspaceStore.getState().openTab(PATH)
        useWorkspaceStore.getState().hydrateLineEnding(PATH, "lf", documentGeneration(PATH))
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

    it("preserves the tab CRLF target when resolving and saving a normalized merge buffer", async () => {
        const main = mountMainView("mine\nline\n")
        vi.mocked(ipc.openFile).mockResolvedValue({
            kind: "full",
            content: "disk\r\nline\r\n",
            size: 12,
            lineEnding: "crlf"
        })
        const store = useWorkspaceStore.getState()
        store.openTab(PATH)
        store.hydrateLineEnding(PATH, "crlf", documentGeneration(PATH))
        store.markDirty(PATH, true)
        store.markExternallyModified(PATH, true)
        useUiStore.getState().openResolver(PATH)
        render(<ExternalChangeResolver />)

        fireEvent.click(await screen.findByRole("button", { name: "全部保留我的" }))
        fireEvent.click(screen.getByRole("button", { name: "解決並存檔" }))

        await waitFor(() => expect(ipc.saveFile).toHaveBeenCalledWith(PATH, "mine\r\nline\r\n"))
        expect(main.state.doc.toString()).toBe("mine\nline\n")
        expect(useUiStore.getState().resolverPath).toBe(null)
    })

    it("blocks Mixed resolver saves before I/O and keeps dirty external state", async () => {
        mountMainView("mine\nline\n")
        vi.mocked(ipc.openFile).mockResolvedValue({
            kind: "full",
            content: "disk\r\nline\n",
            size: 11,
            lineEnding: "mixed"
        })
        const store = useWorkspaceStore.getState()
        store.openTab(PATH)
        store.hydrateLineEnding(PATH, "mixed", documentGeneration(PATH))
        store.markDirty(PATH, true)
        store.markExternallyModified(PATH, true)
        useUiStore.getState().openResolver(PATH)
        render(<ExternalChangeResolver />)

        fireEvent.click(await screen.findByRole("button", { name: "全部保留我的" }))
        fireEvent.click(screen.getByRole("button", { name: "解決並存檔" }))

        await waitFor(() => expect(showActionError).toHaveBeenCalledTimes(1))
        expect(ipc.saveFile).not.toHaveBeenCalled()
        expect(useUiStore.getState().resolverPath).toBe(PATH)
        const tab = useWorkspaceStore.getState().groups[0].tabs.find((candidate) => candidate.path === PATH)
        expect(tab?.dirty).toBe(true)
        expect(tab?.externallyModified).toBe(true)
    })

    it("allows a Mixed resolver save after explicit CRLF selection", async () => {
        mountMainView("mine\nline\n")
        vi.mocked(ipc.openFile).mockResolvedValue({
            kind: "full",
            content: "disk\r\nline\n",
            size: 11,
            lineEnding: "mixed"
        })
        const store = useWorkspaceStore.getState()
        store.openTab(PATH)
        store.hydrateLineEnding(PATH, "mixed", documentGeneration(PATH))
        store.setLineEnding(PATH, "crlf")
        store.markExternallyModified(PATH, true)
        useUiStore.getState().openResolver(PATH)
        render(<ExternalChangeResolver />)

        fireEvent.click(await screen.findByRole("button", { name: "全部保留我的" }))
        fireEvent.click(screen.getByRole("button", { name: "解決並存檔" }))

        await waitFor(() => expect(ipc.saveFile).toHaveBeenCalledWith(PATH, "mine\r\nline\r\n"))
        expect(useUiStore.getState().resolverPath).toBe(null)
        expect(showActionError).not.toHaveBeenCalled()
    })

    // Finding #2: saveFile failure must not silently swallow errors. The
    // resolver stays open (resolverPath unchanged), externallyModified stays
    // true, and an error message appears so the user can retry or cancel.
    it("save failure keeps resolver open and shows an error", async () => {
        mountMainView("mine")
        vi.mocked(ipc.openFile).mockResolvedValue({ kind: "full", content: "disk", size: 4, lineEnding: "lf" })
        vi.mocked(ipc.saveFile).mockRejectedValue(new Error("disk full"))
        useWorkspaceStore.getState().openTab(PATH)
        useWorkspaceStore.getState().hydrateLineEnding(PATH, "lf", documentGeneration(PATH))
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

    it("keeps the resolver merge view and progress on an unchanged sibling notification", async () => {
        mountMainView("mine")
        vi.mocked(ipc.openFile).mockResolvedValue({ kind: "full", content: "disk", size: 4, lineEnding: "lf" })
        useWorkspaceStore.setState({ workspacePath: "/w" })
        useWorkspaceStore.getState().openTab(PATH)
        useUiStore.getState().openResolver(PATH)
        const { container } = render(<ExternalChangeResolver />)
        await screen.findByRole("button", { name: "解決並存檔" })
        const editor = container.ownerDocument.querySelector(".external-resolver-merge .cm-editor")!
        const view = EditorView.findFromDOM(editor as HTMLElement)!
        view.dispatch({ changes: { from: 0, insert: "in progress " } })
        await act(async () => capturedFsListener({ payload: { workspaceRoot: "/w", paths: ["/w"] } }))
        expect(ipc.openFile).toHaveBeenCalledTimes(2)
        expect(container.ownerDocument.querySelector(".external-resolver-merge .cm-editor")).toBe(editor)
        expect(view.state.doc.toString()).toBe("in progress mine")
        expect(screen.queryByText("磁碟版已再次變更")).not.toBeInTheDocument()
    })

    // Finding #3: fs:external-change rebuild path. Injecting a disk-rechange
    // event for this path surfaces the "磁碟版已再次變更" hint.
    it("fs:external-change for this path shows the re-changed hint", async () => {
        mountMainView("mine")
        vi.mocked(ipc.openFile).mockResolvedValue({ kind: "full", content: "disk", size: 4, lineEnding: "lf" })
        useWorkspaceStore.setState({ workspacePath: "/w" })
        useWorkspaceStore.getState().openTab(PATH)
        useWorkspaceStore.getState().markExternallyModified(PATH, true)
        useUiStore.getState().openResolver(PATH)
        render(<ExternalChangeResolver />)
        await screen.findByRole("button", { name: "解決並存檔" })
        vi.mocked(ipc.openFile).mockResolvedValue({ kind: "full", content: "disk2", size: 5, lineEnding: "lf" })
        capturedFsListener({ payload: { workspaceRoot: "/w", paths: [PATH] } })
        expect(await screen.findByText("磁碟版已再次變更")).toBeInTheDocument()
    })

    it.each([
        { path: "/w/target/a.txt", root: "/w", directory: "/w/target", matches: true },
        { path: "/w/targetX/a.txt", root: "/w", directory: "/w/target", matches: false },
        { path: String.raw`C:\Work\Target\a.txt`, root: "C:/Work", directory: "c:/work/target", matches: true },
    ])("coalesced $directory matches $path: $matches", async ({ path, root, directory, matches }) => {
        mountMainView("mine", path)
        vi.mocked(ipc.openFile).mockResolvedValue({ kind: "full", content: "disk", size: 4, lineEnding: "lf" })
        useWorkspaceStore.setState({ workspacePath: root })
        useWorkspaceStore.getState().openTab(path)
        useUiStore.getState().openResolver(path)
        render(<ExternalChangeResolver />)
        await screen.findByRole("button", { name: "解決並存檔" })
        expect(ipc.openFile).toHaveBeenCalledTimes(1)
        vi.mocked(ipc.openFile).mockResolvedValue({ kind: "full", content: "changed", size: 7, lineEnding: "lf" })
        capturedFsListener({ payload: { workspaceRoot: root, paths: [directory] } })
        if (matches) {
            expect(await screen.findByText("磁碟版已再次變更")).toBeInTheDocument()
            expect(ipc.openFile).toHaveBeenLastCalledWith(path)
            expect(ipc.openFile).toHaveBeenCalledTimes(2)
        } else {
            expect(ipc.openFile).toHaveBeenCalledTimes(1)
            expect(screen.queryByText("磁碟版已再次變更")).not.toBeInTheDocument()
        }
    })

    // #57 T3 AC4：resolver 開著時，舊 workspace watcher 的殘留事件（root 不符）
    // 不得觸發 rebuild——不重讀磁碟、不顯示再次變更提示。
    it("fs:external-change from a stale workspace root is dropped (#57 T3)", async () => {
        mountMainView("mine")
        vi.mocked(ipc.openFile).mockResolvedValue({ kind: "full", content: "disk", size: 4, lineEnding: "lf" })
        useWorkspaceStore.setState({ workspacePath: "/w" })
        useWorkspaceStore.getState().openTab(PATH)
        useWorkspaceStore.getState().markExternallyModified(PATH, true)
        useUiStore.getState().openResolver(PATH)
        render(<ExternalChangeResolver />)
        await screen.findByRole("button", { name: "解決並存檔" })
        expect(ipc.openFile).toHaveBeenCalledTimes(1)
        capturedFsListener({ payload: { workspaceRoot: "/old", paths: [PATH] } })
        // 事件被丟棄：不重讀磁碟，也不冒出提示。
        expect(ipc.openFile).toHaveBeenCalledTimes(1)
        expect(screen.queryByText("磁碟版已再次變更")).not.toBeInTheDocument()
    })

    // #123 item 2：resolver 開著時磁碟版變成不可合併，必須撤下 merge view 並進入
    // degraded，不得再提供以舊內容運作的 merge 動作。
    it.each([
        { name: "binary", next: { kind: "binary", size: 9 } as const, text: "磁碟版無法比對差異（二進位或過大）。" },
        { name: "tooLarge", next: { kind: "tooLarge", size: 99999999 } as const, text: "磁碟版無法比對差異（二進位或過大）。" },
        { name: "unreadable", next: null, text: "無法讀取磁碟版本（檔案可能已刪除或暫時無法存取）。" },
    ])("fs:external-change to $name while open drops the merge view and degrades", async ({ next, text }) => {
        mountMainView("mine")
        vi.mocked(ipc.openFile).mockResolvedValue({ kind: "full", content: "disk", size: 4, lineEnding: "lf" })
        useWorkspaceStore.setState({ workspacePath: "/w" })
        useWorkspaceStore.getState().openTab(PATH)
        useWorkspaceStore.getState().markExternallyModified(PATH, true)
        useUiStore.getState().openResolver(PATH)
        render(<ExternalChangeResolver />)
        await screen.findByRole("button", { name: "解決並存檔" })
        if (next) vi.mocked(ipc.openFile).mockResolvedValue(next)
        else vi.mocked(ipc.openFile).mockRejectedValue(new Error("missing"))
        capturedFsListener({ payload: { workspaceRoot: "/w", paths: [PATH] } })
        expect(await screen.findByText(text)).toBeInTheDocument()
        expect(screen.queryByRole("button", { name: "解決並存檔" })).not.toBeInTheDocument()
        expect(screen.queryByRole("button", { name: "全部採用磁碟版" })).not.toBeInTheDocument()
        expect(document.querySelector(".external-resolver-merge")).toBeNull()
        expect(document.querySelector(".cm-mergeView, .cm-deletedChunk, .cm-changedLine")).toBeNull()
        expect(ipc.saveFile).not.toHaveBeenCalled()
    })

    async function openWithFullDisk() {
        mountMainView("mine")
        vi.mocked(ipc.openFile).mockResolvedValue({ kind: "full", content: "disk", size: 4, lineEnding: "lf" })
        useWorkspaceStore.setState({ workspacePath: "/w" })
        useWorkspaceStore.getState().openTab(PATH)
        useWorkspaceStore.getState().markExternallyModified(PATH, true)
        useUiStore.getState().openResolver(PATH)
        render(<ExternalChangeResolver />)
        await screen.findByRole("button", { name: "解決並存檔" })
        const view = EditorView.findFromDOM(document.querySelector(".external-resolver-merge .cm-editor") as HTMLElement)!
        return view
    }
    const fire = () => capturedFsListener({ payload: { workspaceRoot: "/w", paths: [PATH] } })
    const mergeDoc = () =>
        EditorView.findFromDOM(document.querySelector(".external-resolver-merge .cm-editor") as HTMLElement)!.state.doc.toString()

    // Codex review 3: an older read finishing after a newer one must be ignored.
    it("ignores an older binary read that finishes after a newer text read", async () => {
        await openWithFullDisk()
        let resolveOld!: (v: Awaited<ReturnType<typeof ipc.openFile>>) => void
        vi.mocked(ipc.openFile)
            .mockImplementationOnce(() => new Promise((r) => { resolveOld = r }))
            .mockResolvedValueOnce({ kind: "full", content: "disk2", size: 5, lineEnding: "lf" })
        fire()
        fire()
        expect(await screen.findByText("磁碟版已再次變更")).toBeInTheDocument()
        await act(async () => resolveOld({ kind: "binary", size: 9 }))
        expect(screen.getByRole("button", { name: "解決並存檔" })).toBeInTheDocument()
        expect(screen.queryByText("磁碟版無法比對差異（二進位或過大）。")).not.toBeInTheDocument()
        expect(document.querySelector(".external-resolver-merge .cm-editor")).not.toBeNull()
    })

    // Codex review 2: degraded must recover and keep the in-progress edit.
    it("recovers from binary degraded when a later read returns text, keeping in-progress edits", async () => {
        const view = await openWithFullDisk()
        view.dispatch({ changes: { from: 0, insert: "wip " } })
        vi.mocked(ipc.openFile).mockResolvedValue({ kind: "binary", size: 9 })
        fire()
        await screen.findByText("磁碟版無法比對差異（二進位或過大）。")
        vi.mocked(ipc.openFile).mockResolvedValue({ kind: "full", content: "disk3", size: 5, lineEnding: "lf" })
        fire()
        expect(await screen.findByRole("button", { name: "解決並存檔" })).toBeInTheDocument()
        expect(screen.getByRole("button", { name: "全部保留我的" })).toBeEnabled()
        expect(screen.getByRole("button", { name: "全部採用磁碟版" })).toBeEnabled()
        expect(screen.queryByText("磁碟版無法比對差異（二進位或過大）。")).not.toBeInTheDocument()
        expect(mergeDoc()).toBe("wip mine")
        fireEvent.click(screen.getByRole("button", { name: "全部採用磁碟版" }))
        expect(mergeDoc()).toBe("disk3")
    })

    // Codex review 1: a read failure is "unreadable", not "deleted", and recovers.
    it("read failure enters unreadable with only cancel/keep-mine, then recovers", async () => {
        const view = await openWithFullDisk()
        view.dispatch({ changes: { from: 0, insert: "wip " } })
        vi.mocked(ipc.openFile).mockRejectedValue(new Error("EACCES"))
        fire()
        expect(await screen.findByRole("button", { name: "保留我的（覆寫存檔）" })).toBeInTheDocument()
        expect(screen.getByRole("button", { name: "取消" })).toBeInTheDocument()
        for (const name of ["丟棄並關閉分頁", "全部採用磁碟版", "解決並存檔", "採用磁碟版（重新載入）"]) {
            expect(screen.queryByRole("button", { name })).not.toBeInTheDocument()
        }
        // Same content as before the failure must still restore.
        vi.mocked(ipc.openFile).mockResolvedValue({ kind: "full", content: "disk", size: 4, lineEnding: "lf" })
        fire()
        expect(await screen.findByRole("button", { name: "解決並存檔" })).toBeInTheDocument()
        expect(screen.queryByRole("button", { name: "保留我的（覆寫存檔）" })).not.toBeInTheDocument()
        expect(mergeDoc()).toBe("wip mine")
    })
})
