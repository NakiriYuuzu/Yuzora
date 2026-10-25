import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { LOCAL_PDF_MAX_BYTES, REMOTE_PDF_MAX_BYTES } from "./pdfDocument"
import { PDF_TO_CSS_UNITS, layoutPages, pageScrollTop } from "./pdfLayout"
import { clearRetainedViewStatesForTest } from "./viewRetention"

const loader = vi.hoisted(() => ({
    readPdfBytes: vi.fn(),
    openPdfDocument: vi.fn()
}))
vi.mock("./pdfDocument", async (importOriginal) => ({
    ...(await importOriginal<typeof import("./pdfDocument")>()),
    ...loader
}))

import { PdfView, isPdfPath } from "./PdfView"

// US Letter: 612×792 pt.
const PAGE = { width: 612 * PDF_TO_CSS_UNITS, height: 792 * PDF_TO_CSS_UNITS }

function fakePage(fail = false, size = { width: 612, height: 792 }) {
    return {
        getViewport: ({ scale }: { scale: number }) => ({ width: size.width * scale, height: size.height * scale }),
        render: vi.fn(() => ({
            promise: fail ? Promise.reject(new Error("broken page")) : Promise.resolve(),
            cancel: vi.fn()
        })),
        cleanup: vi.fn()
    }
}

function fakeHandle(numPages = 3, options: { failPage?: number; size?: { width: number; height: number } } = {}) {
    const pages = Array.from({ length: numPages }, (_, i) => fakePage(options.failPage === i + 1, options.size))
    const doc = { numPages, getPage: vi.fn((n: number) => Promise.resolve(pages[n - 1])) }
    // Idempotent like the real handle; `released` counts actual releases.
    const released = vi.fn()
    let done = false
    const destroy = () => {
        if (done) return
        done = true
        released()
    }
    return { doc, pages, destroy, released }
}

let frames = new Map<number, FrameRequestCallback>()
let nextFrame = 0
function flushFrames() {
    act(() => {
        const pending = [...frames.values()]
        frames = new Map()
        for (const callback of pending) callback(0)
    })
}

const prototype = HTMLElement.prototype
// Exactly one page width plus padding by default: fit width is 100%.
let viewportSize = { width: PAGE.width + 32, height: 600 }
const clientWidth = Object.getOwnPropertyDescriptor(prototype, "clientWidth")
const clientHeight = Object.getOwnPropertyDescriptor(prototype, "clientHeight")

beforeEach(() => {
    loader.readPdfBytes.mockReset().mockResolvedValue({ kind: "ok", bytes: new Uint8Array([37]), size: 1 })
    loader.openPdfDocument.mockReset()
    frames = new Map()
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
        frames.set(++nextFrame, callback)
        return nextFrame
    })
    vi.stubGlobal("cancelAnimationFrame", (id: number) => frames.delete(id))
    clearRetainedViewStatesForTest()
    viewportSize = { width: PAGE.width + 32, height: 600 }
    Object.defineProperty(prototype, "clientWidth", { configurable: true, get: () => viewportSize.width })
    Object.defineProperty(prototype, "clientHeight", { configurable: true, get: () => viewportSize.height })
})

afterEach(() => {
    cleanup()
    vi.unstubAllGlobals()
    if (clientWidth) Object.defineProperty(prototype, "clientWidth", clientWidth)
    if (clientHeight) Object.defineProperty(prototype, "clientHeight", clientHeight)
})

function viewport() {
    return screen.getByTestId("pdf-view").querySelector<HTMLElement>("[data-slot=scroll-area-viewport]")!
}

async function renderReady(handle = fakeHandle()) {
    loader.openPdfDocument.mockResolvedValue(handle)
    const view = render(<PdfView path="/w/spec.pdf" generation={0} />)
    await screen.findByRole("toolbar")
    return { ...view, handle }
}

describe("isPdfPath", () => {
    it("matches .pdf case-insensitively", () => {
        expect(isPdfPath("/w/spec.pdf")).toBe(true)
        expect(isPdfPath("/w/SPEC.PDF")).toBe(true)
        expect(isPdfPath("/w/spec.pdf.txt")).toBe(false)
        expect(isPdfPath("/w/pdf")).toBe(false)
    })
})

describe("PdfView", () => {
    it("shows the page count, current page and fit-width zoom", async () => {
        await renderReady()
        expect(loader.readPdfBytes).toHaveBeenCalledWith("/w/spec.pdf")
        expect(screen.getByTestId("pdf-view-page-count")).toHaveTextContent("/ 3")
        expect(screen.getByRole("textbox", { name: "Page number" })).toHaveValue("1")
        expect(screen.getByTestId("pdf-view-zoom")).toHaveTextContent("100%")
        expect(screen.getByRole("button", { name: "Fit width" })).toHaveAttribute("aria-pressed", "true")
        expect(screen.getByRole("button", { name: "Previous page" })).toBeDisabled()
    })

    it("mounts only pages near the viewport", async () => {
        const { handle } = await renderReady(fakeHandle(40))
        expect(screen.getAllByTestId("pdf-view-page").map((page) => page.dataset.pageNumber)).toEqual(["1", "2"])
        flushFrames()
        // Page 1 was also fetched once while loading, for its size.
        await waitFor(() => expect(handle.pages[1].render).toHaveBeenCalledTimes(1))
        expect(handle.doc.getPage.mock.calls.map(([page]) => page).sort()).toEqual([1, 1, 2])
    })

    it("renders each page into a canvas at the device pixel ratio", async () => {
        const { handle } = await renderReady()
        flushFrames()
        const [first] = screen.getAllByTestId("pdf-view-page")
        await waitFor(() => expect(first.querySelector("canvas")).not.toBeNull())
        const call = handle.pages[0].render.mock.calls[0] as unknown as [{ canvas: HTMLCanvasElement; viewport: { width: number } }]
        expect(call[0].canvas).toBe(first.querySelector("canvas"))
        expect(call[0].viewport.width).toBeCloseTo(PAGE.width)
    })

    it("marks a page that fails to render", async () => {
        await renderReady(fakeHandle(2, { failPage: 1 }))
        flushFrames()
        expect(await screen.findByText("This page could not be rendered")).toBeInTheDocument()
    })

    it("navigates with next page and the page number field", async () => {
        await renderReady()
        const layout = layoutPages([PAGE, PAGE, PAGE], 1)
        fireEvent.click(screen.getByRole("button", { name: "Next page" }))
        expect(viewport().scrollTop).toBe(pageScrollTop(layout, 1))
        fireEvent.scroll(viewport())
        flushFrames()
        expect(screen.getByRole("textbox", { name: "Page number" })).toHaveValue("2")

        const input = screen.getByRole("textbox", { name: "Page number" })
        fireEvent.change(input, { target: { value: "99" } })
        fireEvent.keyDown(input, { key: "Enter" })
        expect(viewport().scrollTop).toBe(pageScrollTop(layout, 2))
    })

    it("keeps the page number and the page buttons in step on short slides", async () => {
        // 16:9 slides at fit width in a tall, narrow pane: several pages per screen.
        viewportSize = { width: 500, height: 900 }
        await renderReady(fakeHandle(10, { size: { width: 960, height: 540 } }))
        const page = () => screen.getByRole("textbox", { name: "Page number" })
        const settle = () => {
            fireEvent.scroll(viewport())
            flushFrames()
        }
        expect(page()).toHaveValue("1")
        for (const expected of ["2", "3", "4"]) {
            fireEvent.click(screen.getByRole("button", { name: "Next page" }))
            settle()
            expect(page()).toHaveValue(expected)
        }
        fireEvent.click(screen.getByRole("button", { name: "Previous page" }))
        settle()
        expect(page()).toHaveValue("3")
        // Scrolling by hand drops the pin and follows the most visible page.
        viewport().scrollTop = 0
        settle()
        expect(page()).toHaveValue("1")
    })

    it("keeps its place while the editor surface is hidden", async () => {
        await renderReady(fakeHandle(10))
        const input = screen.getByRole("textbox", { name: "Page number" })
        fireEvent.change(input, { target: { value: "6" } })
        fireEvent.keyDown(input, { key: "Enter" })
        const reading = viewport().scrollTop
        // display: none reports a zero box, and the engine may reset scrolling.
        viewportSize = { width: 0, height: 0 }
        viewport().scrollTop = 0
        fireEvent.scroll(viewport())
        flushFrames()
        expect(screen.getByTestId("pdf-view-zoom")).toHaveTextContent("100%")
        viewportSize = { width: PAGE.width + 32, height: 600 }
        fireEvent.scroll(viewport())
        flushFrames()
        expect(viewport().scrollTop).toBe(reading)
        expect(screen.getByRole("textbox", { name: "Page number" })).toHaveValue("6")
    })

    it("keeps the open document when a reload does not parse yet", async () => {
        const { rerender, handle } = await renderReady()
        fireEvent.click(screen.getByRole("button", { name: "Zoom in" }))
        loader.openPdfDocument.mockRejectedValue(Object.assign(new Error("Invalid PDF structure."), { name: "InvalidPDFException" }))
        rerender(<PdfView path="/w/spec.pdf" generation={1} />)
        await waitFor(() => expect(loader.openPdfDocument).toHaveBeenCalledTimes(2))
        await act(async () => {})
        expect(screen.queryByText("Could not open this PDF")).toBeNull()
        expect(screen.getByTestId("pdf-view-zoom")).toHaveTextContent("125%")
        expect(handle.released).not.toHaveBeenCalled()
    })

    it("zooms in steps and returns to fit width", async () => {
        await renderReady()
        fireEvent.click(screen.getByRole("button", { name: "Zoom in" }))
        expect(screen.getByTestId("pdf-view-zoom")).toHaveTextContent("125%")
        expect(screen.getByRole("button", { name: "Fit width" })).toHaveAttribute("aria-pressed", "false")
        fireEvent.click(screen.getByRole("button", { name: "Zoom out" }))
        fireEvent.click(screen.getByRole("button", { name: "Zoom out" }))
        expect(screen.getByTestId("pdf-view-zoom")).toHaveTextContent("80%")
        fireEvent.click(screen.getByRole("button", { name: "Fit width" }))
        expect(screen.getByTestId("pdf-view-zoom")).toHaveTextContent("100%")
    })

    it("comes back to the zoom it was left at after a remount", async () => {
        const first = await renderReady()
        fireEvent.click(screen.getByRole("button", { name: "Zoom in" }))
        expect(screen.getByTestId("pdf-view-zoom")).toHaveTextContent("125%")
        first.unmount()

        await renderReady()
        expect(screen.getByTestId("pdf-view-zoom")).toHaveTextContent("125%")
        expect(screen.getByRole("button", { name: "Fit width" })).toHaveAttribute("aria-pressed", "false")
    })

    it("comes back in fit-width mode, refitted to the pane", async () => {
        const first = await renderReady()
        fireEvent.click(screen.getByRole("button", { name: "Zoom in" }))
        fireEvent.click(screen.getByRole("button", { name: "Fit width" }))
        first.unmount()

        viewportSize = { width: (PAGE.width + 32) * 2, height: 600 }
        await renderReady()
        expect(screen.getByRole("button", { name: "Fit width" })).toHaveAttribute("aria-pressed", "true")
        expect(screen.getByTestId("pdf-view-zoom")).not.toHaveTextContent("100%")
    })

    it("keeps zoom and scroll per document", async () => {
        const first = await renderReady()
        fireEvent.click(screen.getByRole("button", { name: "Zoom in" }))
        first.unmount()

        loader.openPdfDocument.mockResolvedValue(fakeHandle())
        render(<PdfView path="/w/other.pdf" generation={0} />)
        await screen.findByRole("toolbar")
        expect(screen.getByTestId("pdf-view-zoom")).toHaveTextContent("100%")
    })

    it("restores the scroll position on the same page once the layout is measured", async () => {
        const first = await renderReady(fakeHandle(10))
        fireEvent.click(screen.getByRole("button", { name: "Zoom in" }))
        const input = screen.getByRole("textbox", { name: "Page number" })
        fireEvent.change(input, { target: { value: "6" } })
        fireEvent.keyDown(input, { key: "Enter" })
        const reading = viewport().scrollTop
        expect(reading).toBeGreaterThan(0)
        fireEvent.scroll(viewport())
        flushFrames()
        first.unmount()

        await renderReady(fakeHandle(10))
        expect(screen.getByTestId("pdf-view-zoom")).toHaveTextContent("125%")
        expect(viewport().scrollTop).toBe(reading)
        fireEvent.scroll(viewport())
        flushFrames()
        expect(screen.getByRole("textbox", { name: "Page number" })).toHaveValue("6")
    })

    it("explains a file over the preview limit", async () => {
        loader.readPdfBytes.mockResolvedValue({ kind: "tooLarge", size: 70 * 1024 * 1024, limit: LOCAL_PDF_MAX_BYTES })
        render(<PdfView path="/w/scan.pdf" />)
        expect(await screen.findByText("PDF is too large to preview")).toBeInTheDocument()
        expect(screen.getByText("“scan.pdf” is 70.0 MB; the preview limit is 50.0 MB")).toBeInTheDocument()
        expect(loader.openPdfDocument).not.toHaveBeenCalled()
    })

    it("explains an oversized remote file without a known size", async () => {
        loader.readPdfBytes.mockResolvedValue({ kind: "tooLarge", size: null, limit: REMOTE_PDF_MAX_BYTES })
        render(<PdfView path="/w/scan.pdf" />)
        expect(await screen.findByText("“scan.pdf” exceeds the preview limit of 8.0 MB")).toBeInTheDocument()
    })

    it("explains password-protected PDFs", async () => {
        loader.openPdfDocument.mockRejectedValue(Object.assign(new Error("No password given"), { name: "PasswordException" }))
        render(<PdfView path="/w/secret.pdf" />)
        expect(await screen.findByText("This PDF is password-protected")).toBeInTheDocument()
    })

    it("explains unreadable files", async () => {
        loader.openPdfDocument.mockRejectedValue(Object.assign(new Error("Invalid PDF structure."), { name: "InvalidPDFException" }))
        render(<PdfView path="/w/broken.pdf" />)
        expect(await screen.findByText("Could not open this PDF")).toBeInTheDocument()
    })

    it("reloads in place on a new generation and releases the previous document", async () => {
        const { rerender, handle: first } = await renderReady()
        fireEvent.click(screen.getByRole("button", { name: "Zoom in" }))
        let resolveNext!: (handle: ReturnType<typeof fakeHandle>) => void
        loader.openPdfDocument.mockReturnValue(new Promise((resolve) => { resolveNext = resolve }))

        rerender(<PdfView path="/w/spec.pdf" generation={1} />)
        await waitFor(() => expect(loader.openPdfDocument).toHaveBeenCalledTimes(2))
        // The old document stays on screen until the new one is ready.
        expect(screen.getByRole("toolbar")).toBeInTheDocument()
        expect(first.released).not.toHaveBeenCalled()

        const second = fakeHandle(5)
        await act(async () => resolveNext(second))
        await waitFor(() => expect(screen.getByTestId("pdf-view-page-count")).toHaveTextContent("/ 5"))
        expect(first.released).toHaveBeenCalledTimes(1)
        expect(second.released).not.toHaveBeenCalled()
        expect(screen.getByTestId("pdf-view-zoom")).toHaveTextContent("125%")

        cleanup()
        expect(second.released).toHaveBeenCalledTimes(1)
    })

    it("releases a document that finishes loading after unmount", async () => {
        let resolveLoad!: (handle: ReturnType<typeof fakeHandle>) => void
        loader.openPdfDocument.mockReturnValue(new Promise((resolve) => { resolveLoad = resolve }))
        const { unmount } = render(<PdfView path="/w/spec.pdf" />)
        await waitFor(() => expect(loader.openPdfDocument).toHaveBeenCalled())
        unmount()
        const late = fakeHandle()
        await act(async () => resolveLoad(late))
        await waitFor(() => expect(late.released).toHaveBeenCalledTimes(1))
    })
})
