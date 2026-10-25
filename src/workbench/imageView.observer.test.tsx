import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { remoteFilePath } from "@/lib/runtimeIdentity"

const ipc = vi.hoisted(() => ({ open: vi.fn(), remote: vi.fn() }))
vi.mock("@/lib/ipc", () => ({ openFile: ipc.open, readFileBase64: ipc.remote }))
vi.mock("@tauri-apps/api/core", () => ({ convertFileSrc: (path: string) => `owned-image:${path}` }))
vi.mock("react-i18next", () => ({ useTranslation: () => ({ t: (key: string) => key }) }))

import { ImageView } from "./ImageView"

class TrackedResizeObserver {
    static live = new Set<TrackedResizeObserver>()
    targets = new Set<Element>()

    constructor(readonly callback: ResizeObserverCallback) {}

    observe(target: Element) {
        this.targets.add(target)
        TrackedResizeObserver.live.add(this)
    }

    unobserve(target: Element) {
        this.targets.delete(target)
        if (!this.targets.size) TrackedResizeObserver.live.delete(this)
    }

    disconnect() {
        this.targets.clear()
        TrackedResizeObserver.live.delete(this)
    }
}

let width = 500
let height = 400

async function settle() {
    await act(async () => {
        await Promise.resolve()
        await vi.runAllTimersAsync()
    })
}

function loadImage(imageWidth = 1000, imageHeight = 800) {
    const image = screen.getByTestId("image-view-img") as HTMLImageElement
    Object.defineProperties(image, {
        naturalWidth: { configurable: true, value: imageWidth },
        naturalHeight: { configurable: true, value: imageHeight }
    })
    fireEvent.load(image)
    return image
}

beforeEach(() => {
    vi.useFakeTimers()
    vi.stubGlobal("ResizeObserver", TrackedResizeObserver)
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => setTimeout(() => callback(0), 0))
    vi.stubGlobal("cancelAnimationFrame", (id: ReturnType<typeof setTimeout>) => clearTimeout(id))
    width = 500
    height = 400
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(() => ({
        width, height, top: 0, left: 0, bottom: height, right: width, x: 0, y: 0, toJSON() {}
    }))
    ipc.open.mockReset().mockResolvedValue({ kind: "binary", size: 4096 })
    ipc.remote.mockReset().mockResolvedValue({ data: "AAAA", size: 3 })
})

afterEach(async () => {
    cleanup()
    await settle()
    expect(TrackedResizeObserver.live.size).toBe(0)
    expect(vi.getTimerCount()).toBe(0)
    vi.restoreAllMocks()
    vi.unstubAllGlobals()
    vi.useRealTimers()
})

describe("ImageView viewport lifetime", () => {
    it.each([
        { imageWidth: 6000, imageHeight: 4000, expectedWidth: 376, expectedHeight: 251 },
        { imageWidth: 1000, imageHeight: 10000, expectedWidth: 28, expectedHeight: 276 },
        { imageWidth: 20000, imageHeight: 1000, expectedWidth: 376, expectedHeight: 19 },
        { imageWidth: 40, imageHeight: 20, expectedWidth: 40, expectedHeight: 20 }
    ])("fits $imageWidth×$imageHeight within a400×300 viewport without upscaling", async ({ imageWidth, imageHeight, expectedWidth, expectedHeight }) => {
        width = 400
        height = 300
        render(<ImageView path="/owned/large.png" />)
        await settle()
        const image = loadImage(imageWidth, imageHeight)
        expect(image.style.width).toBe(`${expectedWidth}px`)
        expect(image.style.height).toBe(`${expectedHeight}px`)
    })

    it("does not generate negative dimensions when the viewport is smaller than Fit padding", async () => {
        width = 10
        height = 10
        render(<ImageView path="/owned/large.png" />)
        await settle()
        const image = loadImage(6000, 4000)
        expect(image.style.width).toBe("1px")
        expect(image.style.height).toBe("1px")
    })

    it.each(["button", "wheel"])("keeps sub10percent Fit responsive after zoomout by %s", async (gesture) => {
        width = 400
        height = 300
        render(<ImageView path="/owned/large.png" />)
        await settle()
        const image = loadImage(6000, 4000)
        const originalWidth = image.style.width
        const viewport = document.querySelector('[data-slot="scroll-area-viewport"]')!
        if (gesture === "button") fireEvent.click(screen.getByRole("button", { name: "imageViewer.zoomOut" }))
        else expect(fireEvent.wheel(viewport, { ctrlKey: true, deltaY: 1 })).toBe(false)
        expect(image.style.width).toBe(originalWidth)
        width = 500
        height = 400
        act(() => {
            for (const observer of TrackedResizeObserver.live) {
                if (!observer.targets.has(viewport)) continue
                observer.callback([{
                    target: viewport,
                    contentRect: viewport.getBoundingClientRect(),
                    borderBoxSize: [{ inlineSize: width, blockSize: height }],
                    contentBoxSize: [{ inlineSize: width, blockSize: height }],
                    devicePixelContentBoxSize: []
                }], observer as unknown as ResizeObserver)
            }
        })
        await settle()
        expect(image.style.width).toBe("476px")
        expect(image.style.height).toBe("317px")
    })

    it("keeps manual minimum zoom and allows returning to smaller Fit", async () => {
        width = 400
        height = 300
        render(<ImageView path="/owned/large.png" />)
        await settle()
        const image = loadImage(6000, 4000)
        fireEvent.click(screen.getByRole("button", { name: "imageViewer.zoomActual" }))
        expect(image.style.width).toBe("6000px")
        for (let i = 0; i < 30; i++) fireEvent.click(screen.getByRole("button", { name: "imageViewer.zoomOut" }))
        expect(image.style.width).toBe("600px")
        fireEvent.click(screen.getByRole("button", { name: "imageViewer.zoomFit" }))
        expect(image.style.width).toBe("376px")
        fireEvent.click(screen.getByRole("button", { name: "imageViewer.zoomIn" }))
        expect(image.style.width).toBe("600px")
    })

    it("releases the removed viewport after a decode error while the error view stays mounted", async () => {
        render(<ImageView path="/owned/image.png" />)
        await settle()
        const image = loadImage()
        const viewport = document.querySelector('[data-slot="scroll-area-viewport"]')!
        expect([...TrackedResizeObserver.live].some(observer => observer.targets.has(viewport))).toBe(true)

        fireEvent.error(image)
        await settle()

        expect(screen.getByText("imageViewer.loadError")).toBeInTheDocument()
        expect(viewport.isConnected).toBe(false)
        expect(TrackedResizeObserver.live.size).toBe(0)
    })

    it("releases the viewport when a remote file read fails", async () => {
        ipc.remote.mockRejectedValueOnce(new Error("Owned remote read failure"))
        const path = remoteFilePath("owned-host", "/repo/image.png")
        render(<ImageView path={path} />)
        await settle()

        expect(screen.getByText("imageViewer.loadError")).toBeInTheDocument()
        expect(ipc.remote).toHaveBeenCalledWith(path, 8 * 1024 * 1024)
        expect(TrackedResizeObserver.live.size).toBe(0)
    })

    it("keeps resizing and zooming the current viewport after metadata failure", async () => {
        ipc.open.mockRejectedValueOnce(new Error("Owned metadata unavailable"))
        render(<ImageView path="/owned/image.png" />)
        await settle()
        const image = loadImage()
        expect(image.style.width).toBe("470px")
        const viewport = document.querySelector('[data-slot="scroll-area-viewport"]')!
        width = 250
        height = 200
        act(() => {
            for (const observer of TrackedResizeObserver.live) {
                if (!observer.targets.has(viewport)) continue
                observer.callback([{
                    target: viewport,
                    contentRect: viewport.getBoundingClientRect(),
                    borderBoxSize: [{ inlineSize: width, blockSize: height }],
                    contentBoxSize: [{ inlineSize: width, blockSize: height }],
                    devicePixelContentBoxSize: []
                }], observer as unknown as ResizeObserver)
            }
        })
        await settle()
        expect(image.style.width).toBe("220px")
        fireEvent.wheel(viewport, { ctrlKey: true, deltaY: -1 })
        expect(image.style.width).toBe("275px")
        fireEvent.click(screen.getByRole("button", { name: "imageViewer.zoomFit" }))
        expect(image.style.width).toBe("220px")
        expect(screen.queryByText("imageViewer.loadError")).not.toBeInTheDocument()
    })

    it("observes each replacement viewport across100keyed error and recovery cycles", async () => {
        const view = render(<ImageView key="initial" path="/owned/initial.png" />)
        await settle()
        for (let cycle = 0; cycle < 100; cycle++) {
            fireEvent.error(loadImage())
            await settle()
            expect(TrackedResizeObserver.live.size).toBe(0)
            width = cycle % 2 ? 500 : 250
            height = cycle % 2 ? 400 : 200
            const path = `/owned/image-${cycle}.png`
            view.rerender(<ImageView key={path} path={path} />)
            await settle()
            expect(loadImage().style.width).toBe(cycle % 2 ? "470px" : "220px")
            for (const observer of TrackedResizeObserver.live) {
                expect([...observer.targets].every(target => target.isConnected)).toBe(true)
            }
        }
        expect(ipc.open).toHaveBeenCalledTimes(101)
    })

    it("ignores a remote read that resolves after the image view closes", async () => {
        let resolveRead!: (value: { data: string; size: number }) => void
        ipc.remote.mockReturnValueOnce(new Promise(resolve => { resolveRead = resolve }))
        const view = render(<ImageView path={remoteFilePath("owned-host", "/repo/image.png")} />)
        await settle()
        view.unmount()
        await act(async () => { resolveRead({ data: "AAAA", size: 3 }) })
        await settle()

        expect(screen.queryByTestId("image-view")).not.toBeInTheDocument()
        expect(TrackedResizeObserver.live.size).toBe(0)
        expect(ipc.remote).toHaveBeenCalledTimes(1)
    })
})
