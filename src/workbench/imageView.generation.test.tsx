import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { OpenFileResult } from "@/lib/types"
import { remoteFilePath } from "@/lib/runtimeIdentity"

const ipc = vi.hoisted(() => ({ open: vi.fn(), snapshot: vi.fn(), remote: vi.fn() }))
vi.mock("@/lib/ipc", () => ({ openFile: ipc.open, openFileSnapshot: ipc.snapshot, readFileBase64: ipc.remote, saveFile: vi.fn() }))
vi.mock("@tauri-apps/api/core", () => ({ convertFileSrc: (path: string) => `asset://localhost/${encodeURIComponent(path)}` }))
vi.mock("./TabBar", () => ({ TabBar: () => null }))
vi.mock("../editor/EditorPane", () => ({ EditorPane: () => null }))
vi.mock("./MarkdownPreview", () => ({ isMarkdownPath: () => false, MarkdownPreview: () => null }))
vi.mock("./SvgSplitView", () => ({ isSvgPath: () => false, SvgSplitView: () => null }))
vi.mock("@/app/panels/PreviewPanel", () => ({ PreviewPanel: () => null }))
vi.mock("@/app/panels/HerdrTerminalPage", () => ({ HerdrTerminalPage: () => null }))

import { EditorArea } from "./EditorArea"
import { clearAll, documentGeneration, reloadDocument } from "@/editor/documentRegistry"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { useUiStore } from "@/state/uiStore"

const workspaceInitial = useWorkspaceStore.getState()
const uiInitial = useUiStore.getState()
const remotePath = remoteFilePath("owned-image-host", "/repo/logo.png")
const oldSource = "data:image/png;base64,b2xk"
const newSource = "data:image/png;base64,bmV3"

function snapshot(size = 3) {
    return { result: { kind: "binary", size } as OpenFileResult, accept: vi.fn() }
}

function deferred<T>() {
    let resolve!: (value: T) => void
    let reject!: (error: Error) => void
    const promise = new Promise<T>((ok, no) => { resolve = ok; reject = no })
    return { promise, resolve, reject }
}

function mountImage(path = remotePath, splits = 1) {
    useWorkspaceStore.setState({
        workspacePath: path === remotePath ? remoteFilePath("owned-image-host", "/repo") : "/repo",
        groups: Array.from({ length: splits }, (_, index) => ({
            id: `owned-image-${index}`,
            tabs: [{ path, name: "logo.png", dirty: false, externallyModified: false, kind: "file" as const }],
            activePath: path
        })),
        activeGroupIndex: 0
    })
    return render(<EditorArea />)
}

async function acceptReload(path = remotePath) {
    await act(async () => {
        await reloadDocument(path, () => true, "reconcile")
        useWorkspaceStore.getState().markExternallyModified(path, false)
    })
}

function loadedImage(width = 100, height = 100) {
    const image = screen.getByTestId("image-view-img") as HTMLImageElement
    Object.defineProperties(image, {
        naturalWidth: { value: width, configurable: true },
        naturalHeight: { value: height, configurable: true }
    })
    fireEvent.load(image)
    return image
}

beforeEach(() => {
    clearAll()
    useWorkspaceStore.setState(workspaceInitial, true)
    useUiStore.setState({ ...uiInitial, mode: "files" }, true)
    ipc.open.mockReset().mockResolvedValue({ kind: "binary", size: 3 })
    ipc.snapshot.mockReset().mockImplementation(async () => snapshot())
    ipc.remote.mockReset().mockResolvedValue({ data: "b2xk", size: 3 })
})

afterEach(() => {
    cleanup()
    clearAll()
    useWorkspaceStore.setState(workspaceInitial, true)
    useUiStore.setState(uiInitial, true)
})

describe("accepted image generations through EditorArea", () => {
    it("reloads a same-path same-size remote image only after accepting its new generation", async () => {
        mountImage()
        const oldImage = await screen.findByTestId("image-view-img")
        expect(oldImage).toHaveAttribute("src", oldSource)
        const generation = documentGeneration(remotePath)
        const pending = deferred<ReturnType<typeof snapshot>>()
        ipc.snapshot.mockReturnValueOnce(pending.promise)
        let reload!: Promise<unknown>
        act(() => { reload = reloadDocument(remotePath, () => true, "reconcile") })
        expect(documentGeneration(remotePath)).toBe(generation)
        expect(ipc.remote).toHaveBeenCalledTimes(1)
        expect(screen.getByTestId("image-view-img")).toBe(oldImage)
        ipc.remote.mockResolvedValueOnce({ data: "bmV3", size: 3 })
        await act(async () => {
            pending.resolve(snapshot())
            await reload
            useWorkspaceStore.getState().markExternallyModified(remotePath, false)
        })
        expect(documentGeneration(remotePath)).toBeGreaterThan(generation)
        await waitFor(() => expect(screen.getByTestId("image-view-img")).toHaveAttribute("src", newSource))
        expect(ipc.remote).toHaveBeenCalledTimes(2)
    })

    it("keeps the image, zoom and read count on ordinary same-generation notifications", async () => {
        mountImage()
        await screen.findByTestId("image-view-img")
        const image = loadedImage()
        fireEvent.click(screen.getByRole("button", { name: "Actual size (1:1)" }))
        fireEvent.click(screen.getByRole("button", { name: "Zoom in" }))
        act(() => useWorkspaceStore.getState().markExternallyModified(remotePath, false))
        expect(screen.getByTestId("image-view-img")).toBe(image)
        expect(screen.getByTestId("image-view-meta")).toHaveTextContent("125%")
        expect(ipc.remote).toHaveBeenCalledTimes(1)
    })

    it.each(["failed", "rejected"] as const)("does not replace the image after a %s reload", async (outcome) => {
        mountImage()
        const image = await screen.findByTestId("image-view-img")
        const generation = documentGeneration(remotePath)
        const pending = deferred<ReturnType<typeof snapshot>>()
        ipc.snapshot.mockReturnValueOnce(pending.promise)
        let valid = true
        const result = reloadDocument(remotePath, () => valid).catch((error: unknown) => error)
        const fresh = snapshot()
        await act(async () => {
            if (outcome === "failed") pending.reject(new Error("owned read failure"))
            else { valid = false; pending.resolve(fresh) }
            expect(await result).toBeInstanceOf(Error)
            useWorkspaceStore.getState().markExternallyModified(remotePath, false)
        })
        expect(fresh.accept).not.toHaveBeenCalled()
        expect(documentGeneration(remotePath)).toBe(generation)
        expect(screen.getByTestId("image-view-img")).toBe(image)
        expect(ipc.remote).toHaveBeenCalledTimes(1)
    })

    it.each(["success", "failure"] as const)("ignores an older image read's late %s after accepting a new generation", async (outcome) => {
        const old = deferred<{ data: string; size: number }>()
        ipc.remote.mockReturnValueOnce(old.promise).mockResolvedValueOnce({ data: "bmV3", size: 3 })
        mountImage()
        await acceptReload()
        await waitFor(() => expect(screen.getByTestId("image-view-img")).toHaveAttribute("src", newSource))
        await act(async () => {
            if (outcome === "success") old.resolve({ data: "b2xk", size: 3 })
            else old.reject(new Error("old owned read failure"))
        })
        expect(screen.getByTestId("image-view-img")).toHaveAttribute("src", newSource)
        expect(screen.queryByText("Could not load this image")).not.toBeInTheDocument()
    })

    it("recovers from a remote image read error after a successful reload", async () => {
        ipc.remote.mockRejectedValueOnce(new Error("owned image unavailable"))
        mountImage()
        await screen.findByText("Could not load this image")
        ipc.remote.mockResolvedValueOnce({ data: "bmV3", size: 3 })
        await acceptReload()
        await waitFor(() => expect(screen.getByTestId("image-view-img")).toHaveAttribute("src", newSource))
        expect(screen.queryByText("Could not load this image")).not.toBeInTheDocument()
    })

    it("versions a local asset URL after accepted reload, preserves its encoded path and recovers decode errors", async () => {
        const path = "/repo/logo ?#中文.png"
        mountImage(path)
        const oldImage = screen.getByTestId("image-view-img") as HTMLImageElement
        const oldUrl = oldImage.src
        fireEvent.error(oldImage)
        expect(screen.getByText("Could not load this image")).toBeInTheDocument()
        await acceptReload(path)
        const image = await screen.findByTestId("image-view-img") as HTMLImageElement
        expect(image.src).not.toBe(oldUrl)
        expect(new URL(image.src).pathname).toBe(new URL(oldUrl).pathname)
        expect(decodeURIComponent(new URL(image.src).pathname.slice(1))).toBe(path)
        loadedImage(300, 200)
        expect(screen.getByTestId("image-view-meta")).toHaveTextContent("300×200")
    })

    it("updates both splits after accepting a new generation", async () => {
        mountImage(remotePath, 2)
        await waitFor(() => expect(screen.getAllByTestId("image-view-img")).toHaveLength(2))
        ipc.remote.mockResolvedValue({ data: "bmV3", size: 3 })
        await acceptReload()
        await waitFor(() => {
            for (const image of screen.getAllByTestId("image-view-img")) expect(image).toHaveAttribute("src", newSource)
        })
        expect(ipc.remote).toHaveBeenCalledTimes(4)
    })
})
