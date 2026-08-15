import { StrictMode, createRef } from "react"
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import type { GitStatus } from "@/lib/types"

import { buildGitChangeModel, gitChangeId } from "./gitChangeSelection"
import {
    VirtualizedGitChangeList,
    gitChangeVirtualItems,
    type VirtualizedGitChangeListHandle
} from "./VirtualizedGitChangeList"

function status(count: number, prefix = "file"): GitStatus {
    return {
        branch: "main",
        headOid: "0".repeat(40),
        detached: false,
        upstream: null,
        ahead: 0,
        behind: 0,
        staged: [],
        unstaged: Array.from({ length: count }, (_, index) => ({
            path: `${prefix}-${String(index).padStart(4, "0")}.ts`,
            origPath: null,
            status: "M"
        })),
        untracked: [],
        conflicted: [],
        inProgress: null
    }
}

function setViewportGeometry(viewport: HTMLElement, height: number) {
    Object.defineProperty(viewport, "clientHeight", { configurable: true, value: height })
}

function Harness({ snapshot, listRef }: {
    snapshot: GitStatus
    listRef?: React.Ref<VirtualizedGitChangeListHandle>
}) {
    const items = gitChangeVirtualItems(buildGitChangeModel(snapshot))
    return (
        <VirtualizedGitChangeList
            ref={listRef}
            items={items}
            renderItem={(item, _index, style) => (
                <div key={item.key} style={style} data-testid={item.kind === "row" ? `row-${item.key}` : item.key}>
                    {item.kind === "row" ? item.row.path : item.section}
                </div>
            )}
            testId="virtual-list"
            spacerTestId="virtual-spacer"
        />
    )
}

describe("VirtualizedGitChangeList", () => {
    const originalResizeObserver = globalThis.ResizeObserver
    const originalRequestAnimationFrame = globalThis.requestAnimationFrame
    const originalCancelAnimationFrame = globalThis.cancelAnimationFrame

    afterEach(() => {
        vi.restoreAllMocks()
        globalThis.ResizeObserver = originalResizeObserver
        globalThis.requestAnimationFrame = originalRequestAnimationFrame
        globalThis.cancelAnimationFrame = originalCancelAnimationFrame
    })

    it("reranges from the live replacement geometry after ResizeObserver fires", async () => {
        const observations: Array<{ callback: ResizeObserverCallback; disconnected: boolean }> = []
        class ResizeObserverHarness {
            private readonly record: { callback: ResizeObserverCallback; disconnected: boolean }
            constructor(callback: ResizeObserverCallback) {
                this.record = { callback, disconnected: false }
                observations.push(this.record)
            }
            observe() {}
            unobserve() {}
            disconnect() { this.record.disconnected = true }
        }
        let nextRaf = 1
        const rafCallbacks = new Map<number, FrameRequestCallback>()
        globalThis.ResizeObserver = ResizeObserverHarness as unknown as typeof ResizeObserver
        globalThis.requestAnimationFrame = vi.fn((callback: FrameRequestCallback) => {
            const id = nextRaf++
            rafCallbacks.set(id, callback)
            return id
        })
        globalThis.cancelAnimationFrame = vi.fn((id: number) => { rafCallbacks.delete(id) })

        const ref = createRef<VirtualizedGitChangeListHandle>()
        const { rerender } = render(<Harness snapshot={status(40, "old")} listRef={ref} />)
        const viewport = screen.getByTestId("virtual-list").querySelector('[data-slot="scroll-area-viewport"]') as HTMLElement
        setViewportGeometry(viewport, 64)
        act(() => {
            for (const [id, callback] of [...rafCallbacks]) {
                rafCallbacks.delete(id)
                callback(0)
            }
        })

        rerender(<Harness snapshot={status(200, "new")} listRef={ref} />)
        act(() => {
            ref.current?.scrollToKey("c:new-0150.ts")
            observations.at(-1)?.callback([], {} as ResizeObserver)
            for (const [id, callback] of [...rafCallbacks]) {
                rafCallbacks.delete(id)
                callback(0)
            }
        })

        await waitFor(() => expect(screen.getByTestId("row-c:new-0150.ts")).toBeInTheDocument())
        expect(screen.queryByTestId("row-c:new-0000.ts")).toBeNull()
    })

    it("cancels pending RAF and disconnects its viewport observer across StrictMode cleanup", () => {
        const disconnected: boolean[] = []
        const observedElements: Element[] = []
        class ResizeObserverHarness {
            private readonly index: number
            constructor(_callback: ResizeObserverCallback) {
                this.index = disconnected.push(false) - 1
            }
            observe(element: Element) { observedElements[this.index] = element }
            unobserve() {}
            disconnect() { disconnected[this.index] = true }
        }
        let nextRaf = 1
        globalThis.ResizeObserver = ResizeObserverHarness as unknown as typeof ResizeObserver
        const request = vi.fn((_callback: FrameRequestCallback) => nextRaf++)
        const cancel = vi.fn()
        globalThis.requestAnimationFrame = request
        globalThis.cancelAnimationFrame = cancel

        const snapshot = status(50)
        const { unmount } = render(
            <StrictMode>
                <Harness snapshot={snapshot} />
            </StrictMode>
        )
        const viewport = screen.getByTestId("virtual-list").querySelector('[data-slot="scroll-area-viewport"]') as HTMLElement
        fireEvent.scroll(viewport, { target: { scrollTop: 256 } })
        act(() => unmount())

        const disconnectedViewportObservers = observedElements.filter(
            (element, index) => element === viewport && disconnected[index]
        )
        expect(disconnectedViewportObservers.length).toBeGreaterThanOrEqual(2)
        expect(cancel).toHaveBeenCalled()
    })

    it("keeps path-and-side keys distinct in its live key lookup", () => {
        const snapshot = status(1)
        snapshot.staged = [{ path: "partial.ts", origPath: null, status: "M" }]
        snapshot.unstaged = [{ path: "partial.ts", origPath: null, status: "M" }]
        const model = buildGitChangeModel(snapshot)
        expect(model.visualOrder.map(gitChangeId)).toEqual(["s:partial.ts", "c:partial.ts"])
    })
})
