import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, expect, it, vi } from "vitest"
import { openUrl } from "@tauri-apps/plugin-opener"
import { MarkdownDocumentPreview } from "./MarkdownDocumentPreview"
import { renderMarkdownDocument } from "./markdownDocumentRender"

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(async () => {}) }))
afterEach(() => { cleanup(); vi.unstubAllGlobals(); vi.clearAllMocks() })

it("renders complete ordinary Markdown without section navigation and shares sanitizer/link restrictions", async () => {
    render(<MarkdownDocumentPreview content={'# Title\n\n[External](https://example.com) [Relative](./file.md)\n\n<div style="position:fixed" class="fixed" data-yz-source-line="999"><script>bad()</script><form>bad form</form><span>Tail</span></div>'} />)
    await screen.findByText("Tail")
    const preview = screen.getByTestId("markdown-document-preview")
    expect(preview.querySelector("script, form, [style*=fixed], [data-yz-source-line], .fixed")).toBeNull()
    expect(screen.queryByRole("button", { name: /section/i })).toBeNull()
    fireEvent.click(screen.getByText("External"))
    fireEvent.click(screen.getByText("Relative"))
    expect(openUrl).toHaveBeenCalledExactlyOnceWith("https://example.com")
})

it("mounts nearby batches and preserves measured placeholder height when a batch leaves the viewport", async () => {
    const observers: { element?: Element; cb: (entries: { isIntersecting: boolean }[]) => void }[] = []
    const resizeObservers: { element?: Element; cb: () => void }[] = []
    vi.stubGlobal("ResizeObserver", class {
        item: typeof resizeObservers[number]
        constructor(cb: () => void) { this.item = { cb }; resizeObservers.push(this.item) }
        observe(element: Element) { this.item.element = element }
        unobserve() {}
        disconnect() {}
    })
    vi.stubGlobal("IntersectionObserver", class {
        item: typeof observers[number]
        constructor(cb: typeof observers[number]["cb"]) { this.item = { cb }; observers.push(this.item) }
        observe(element: Element) { this.item.element = element }
        disconnect() {}
    })
    const content = "first ordinary paragraph\n\n".repeat(1600) + "\n# FINAL TAIL\n"
    render(<MarkdownDocumentPreview content={content} />)
    await waitFor(() => expect(observers.length).toBeGreaterThan(1))
    expect(screen.queryByText("FINAL TAIL")).toBeNull()
    const last = observers.at(-1)!
    act(() => last.cb([{ isIntersecting: true }]))
    await screen.findByText("FINAL TAIL")
    const first = observers[0]
    vi.spyOn(first.element!, "getBoundingClientRect").mockReturnValue({ height: 500 } as DOMRect)
    act(() => resizeObservers.find((observer) => observer.element === first.element)!.cb())
    act(() => first.cb([{ isIntersecting: false }]))
    expect(first.element!.textContent).toBe("")
    expect((first.element as HTMLElement).style.height).toBe("500px")
})

it("retains all content when IntersectionObserver is unavailable", async () => {
    vi.stubGlobal("IntersectionObserver", undefined)
    render(<MarkdownDocumentPreview content={"ordinary paragraph\n\n".repeat(2000) + "# FULL TAIL"} />)
    await screen.findByText("FULL TAIL")
})

it("ignores retired worker output and mounts the complete current result", async () => {
    const workers: FakeWorker[] = []
    class FakeWorker {
        onmessage: ((event: MessageEvent) => void) | null = null
        onerror: ((event: ErrorEvent) => void) | null = null
        postMessage = vi.fn()
        terminate = vi.fn()
        constructor() { workers.push(this) }
    }
    vi.stubGlobal("Worker", FakeWorker)
    const old = "old paragraph\n\n".repeat(10000)
    const latest = "new paragraph\n\n".repeat(10000)
    const { rerender } = render(<MarkdownDocumentPreview content={old} />)
    await waitFor(() => expect(workers).toHaveLength(1))
    rerender(<MarkdownDocumentPreview content={latest} />)
    expect(workers[0].terminate).toHaveBeenCalledOnce()
    act(() => workers[0].onmessage!(new MessageEvent("message", { data: renderMarkdownDocument("# Old result") })))
    expect(screen.queryByText("Old result")).toBeNull()
    act(() => workers[1].onmessage!(new MessageEvent("message", { data: renderMarkdownDocument("# Current result") })))
    await screen.findByText("Current result")
})

it("reports worker startup failure without attempting a large main-thread render", async () => {
    vi.stubGlobal("Worker", class { constructor() { throw new Error("Worker unavailable") } })
    render(<MarkdownDocumentPreview content={"# Original source\n\n".repeat(10000)} />)
    expect((await screen.findByRole("alert")).textContent).toContain("original content remains available")
    expect(screen.queryByRole("heading")).toBeNull()
})
