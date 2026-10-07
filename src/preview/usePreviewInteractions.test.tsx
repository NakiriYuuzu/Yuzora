import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { act, cleanup, renderHook, waitFor } from "@testing-library/react"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import { writeText } from "@tauri-apps/plugin-clipboard-manager"
import { navigateWorkbenchTabs } from "@/lib/workbenchTabNavigation"
import { previewInteractions, previewNavigationState, previewSelectElement } from "@/lib/ipc"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { usePreviewStore } from "@/state/previewStore"
import { usePreviewInteractions } from "./usePreviewInteractions"
import { enqueueNativePreviewOperation } from "./nativePreviewQueue"
import type { PreviewInteractionSnapshot } from "@/lib/previewTypes"

vi.mock("@/lib/platform", async original => ({ ...await original<typeof import("@/lib/platform")>(), isTauri: () => true }))
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn(async () => {}) }))
vi.mock("@/lib/ipc", async original => ({ ...await original<typeof import("@/lib/ipc")>(), previewInteractions: vi.fn(), previewNavigationState: vi.fn(), previewSelectElement: vi.fn() }))
vi.mock("@/lib/workbenchTabNavigation", async original => ({ ...await original<typeof import("@/lib/workbenchTabNavigation")>(), navigateWorkbenchTabs: vi.fn() }))

const url = "https://example.test/"
const target = { workspace: "/workspace", url, nativeSessionId: "owner", external: true, previewVisible: true }
const selection = { url, selector: "#card", html: '<section id="card">Example</section>', text: "Example", width: 200, height: 100, styles: { display: "flex" }, truncated: false }
const nativeClose = vi.fn(async () => {})

beforeEach(() => {
    vi.clearAllMocks()
    clearMocks()
    nativeClose.mockReset().mockResolvedValue(undefined)
    // previewStore is imported by global setup before feature-level mocks.
    // Intercept its real typed IPC wrapper at the existing Tauri mock boundary.
    mockIPC(command => {
        if (command === "preview_close") return nativeClose()
        throw new Error(`Unexpected native command in preview hook test: ${command}`)
    })
    usePreviewStore.getState().reset()
    useWorkspaceStore.setState({ workspacePath: "/workspace", groups: [{ tabs: [], activePath: null }], activeGroupIndex: 0 })
    usePreviewStore.getState().navigate("/workspace", url)
    usePreviewStore.getState().recordNativeOpen("/workspace", url, "owner")
    vi.mocked(previewNavigationState).mockReset().mockResolvedValue({ sessionId: "owner", url, canGoBack: false, canGoForward: false })
    vi.mocked(previewInteractions).mockReset().mockResolvedValue(null)
    vi.mocked(previewSelectElement).mockReset().mockResolvedValue(undefined)
    vi.mocked(navigateWorkbenchTabs).mockReset()
})
afterEach(async () => {
    cleanup()
    await enqueueNativePreviewOperation(async () => {})
    clearMocks()
    vi.useRealTimers()
    vi.restoreAllMocks()
})

function deferredNavigation() {
    let resolve!: (value: Awaited<ReturnType<typeof previewNavigationState>>) => void
    const promise = new Promise<Awaited<ReturnType<typeof previewNavigationState>>>(done => { resolve = done })
    return { promise, resolve }
}

async function settleMicrotasks() {
    for (let i = 0; i < 8; i++) await Promise.resolve()
}

it("copies a validated selection once and reports completion", async () => {
    const { result } = renderHook(() => usePreviewInteractions(target))
    await act(async () => { await result.current.toggleElementSelection() })
    expect(previewSelectElement).toHaveBeenCalledWith("owner", true)
    vi.mocked(previewInteractions).mockResolvedValueOnce({ commands: [], selection, selecting: false })
    await waitFor(() => expect(writeText).toHaveBeenCalledOnce())
    expect(writeText).toHaveBeenCalledWith(expect.stringContaining("Selector: #card"))
    expect(writeText).toHaveBeenCalledWith(expect.stringContaining(`Source: ${url}`))
    expect(result.current.selecting).toBe(false)
    await waitFor(() => expect(result.current.selectionFeedback).toBeTruthy())
})

it("never lets page polling arm clipboard access", async () => {
    vi.useFakeTimers()
    vi.mocked(previewInteractions).mockResolvedValue({ commands: [], selection, selecting: true })
    const view = renderHook(() => usePreviewInteractions(target))
    await act(async () => { await vi.advanceTimersByTimeAsync(350) })
    expect(writeText).not.toHaveBeenCalled()
    expect(view.result.current.selecting).toBe(false)
})

it("revokes before a queued cancel lets an in-flight page response settle", async () => {
    const view = renderHook(() => usePreviewInteractions(target))
    await act(async () => { await view.result.current.toggleElementSelection() })
    let finish!: (value: PreviewInteractionSnapshot) => void
    vi.mocked(previewInteractions).mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
    await waitFor(() => expect(finish).toBeTypeOf("function"))
    await act(async () => {
        const cancel = view.result.current.toggleElementSelection()
        finish({ commands: [], selection, selecting: true })
        await cancel
    })
    expect(writeText).not.toHaveBeenCalled()
    expect(view.result.current.selecting).toBe(false)
})

it("does not restore a pending toolbar grant after navigation", async () => {
    const view = renderHook(() => usePreviewInteractions(target))
    let finish!: () => void
    vi.mocked(previewSelectElement).mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
    let enabling!: Promise<void>
    act(() => { enabling = view.result.current.toggleElementSelection() })
    await waitFor(() => expect(finish).toBeTypeOf("function"))
    await act(async () => {
        usePreviewStore.getState().navigate("/workspace", "https://other.test/")
        finish()
        await enabling
    })
    expect(view.result.current.selecting).toBe(false)
    expect(writeText).not.toHaveBeenCalled()
})

it("consumes a toolbar grant before writing even if the page keeps selecting true", async () => {
    vi.useFakeTimers()
    const view = renderHook(() => usePreviewInteractions(target))
    await act(async () => { await view.result.current.toggleElementSelection() })
    vi.mocked(previewInteractions).mockResolvedValue({ commands: [], selection, selecting: true })
    await act(async () => { await vi.advanceTimersByTimeAsync(350) })
    expect(writeText).toHaveBeenCalledOnce()
    expect(view.result.current.selecting).toBe(false)
})

it("cancels native selection before hiding for a dialog", async () => {
    const { result, rerender } = renderHook(({ visible }) => usePreviewInteractions({ ...target, previewVisible: visible }), { initialProps: { visible: true } })
    await act(async () => { await result.current.toggleElementSelection() })
    rerender({ visible: false })
    await waitFor(() => expect(previewSelectElement).toHaveBeenLastCalledWith("owner", false))
    expect(result.current.selecting).toBe(false)
})

it("discards an in-flight selection after the workspace and native owner change", async () => {
    const { result, unmount } = renderHook(() => usePreviewInteractions(target))
    await act(async () => { await result.current.toggleElementSelection() })
    let finish!: (value: PreviewInteractionSnapshot) => void
    vi.mocked(previewInteractions).mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
    await waitFor(() => expect(finish).toBeTypeOf("function"))
    unmount()
    useWorkspaceStore.setState({ workspacePath: "/other" })
    usePreviewStore.getState().recordNativeOpen("/other", url, "new-owner")
    await act(async () => { finish({ commands: [], selection, selecting: false }); await enqueueNativePreviewOperation(async () => {}) })
    expect(writeText).not.toHaveBeenCalled()
})

it.each([1, 2, 3, 4, 5])("does not delay a queued close with an obsolete interaction read (sample %s)", async sample => {
    vi.useFakeTimers()
    const navigation = deferredNavigation()
    vi.mocked(previewNavigationState).mockReturnValueOnce(navigation.promise)
    vi.mocked(previewInteractions).mockImplementation(() => new Promise(resolve => {
        setTimeout(() => resolve({ commands: [], selection: null, selecting: false }), 2000)
    }))
    let closeStartedAt: number | null = null
    nativeClose.mockImplementation(async () => { closeStartedAt = Date.now() })
    useWorkspaceStore.getState().openPreviewTab()
    const view = renderHook(() => usePreviewInteractions(target))
    await act(async () => { await vi.advanceTimersByTimeAsync(0); await settleMicrotasks() })
    const navigationReads = vi.mocked(previewNavigationState).mock.calls.length
    // Use the production workspace subscription and shared queue to close it.
    act(() => { useWorkspaceStore.getState().closePreviewTab(); view.unmount() })
    const navigationSettledAt = Date.now()
    await act(async () => {
        navigation.resolve({ sessionId: "owner", url, canGoBack: false, canGoForward: false })
        await settleMicrotasks()
        await vi.advanceTimersByTimeAsync(2000)
        await enqueueNativePreviewOperation(async () => {})
    })
    const metrics = { sample, navigationReads, obsoleteInteractionReads: vi.mocked(previewInteractions).mock.calls.length,
        closeCalls: nativeClose.mock.calls.length,
        closeDelayAfterNavigationMs: closeStartedAt === null ? null : closeStartedAt - navigationSettledAt,
        timersAfterClose: vi.getTimerCount(), nativeSessionAfterClose: usePreviewStore.getState().nativeSession }
    if (import.meta.env.YUZORA_PERF_MEASURE) console.info("PREVIEW_CLOSE_MEASUREMENT", JSON.stringify(metrics))
    expect(metrics).toMatchObject({ navigationReads: 1, obsoleteInteractionReads: 0, closeCalls: 1,
        closeDelayAfterNavigationMs: 0, timersAfterClose: 0, nativeSessionAfterClose: null })
    expect(writeText).not.toHaveBeenCalled()
})

it.each([1, 2, 3, 4, 5])("releases polling and key listeners across 100 close cycles after warmup (sample %s)", async sample => {
    vi.useFakeTimers()
    const listeners = new Set<EventListenerOrEventListenerObject>()
    const add = document.addEventListener.bind(document)
    const remove = document.removeEventListener.bind(document)
    vi.spyOn(document, "addEventListener").mockImplementation((type, listener, options) => {
        if (type === "keydown" && options === true) listeners.add(listener)
        add(type, listener, options)
    })
    vi.spyOn(document, "removeEventListener").mockImplementation((type, listener, options) => {
        if (type === "keydown" && options === true) listeners.delete(listener)
        remove(type, listener, options)
    })
    let warmListeners = -1
    let warmTimers = -1
    let interactionReadsAfterWarmup = 0
    for (let cycle = 0; cycle < 110; cycle++) {
        useWorkspaceStore.getState().openPreviewTab()
        usePreviewStore.getState().recordNativeOpen("/workspace", url, "owner")
        const navigation = deferredNavigation()
        vi.mocked(previewNavigationState).mockReturnValueOnce(navigation.promise)
        const view = renderHook(() => usePreviewInteractions(target))
        await act(async () => { await vi.advanceTimersByTimeAsync(0); await settleMicrotasks() })
        act(() => { useWorkspaceStore.getState().closePreviewTab(); view.unmount() })
        await act(async () => {
            navigation.resolve({ sessionId: "owner", url, canGoBack: false, canGoForward: false })
            await settleMicrotasks()
            await enqueueNativePreviewOperation(async () => {})
        })
        cleanup()
        expect(usePreviewStore.getState().nativeSession).toBeNull()
        expect(listeners.size).toBe(0)
        expect(vi.getTimerCount()).toBe(0)
        if (cycle === 9) {
            warmListeners = listeners.size
            warmTimers = vi.getTimerCount()
            interactionReadsAfterWarmup = vi.mocked(previewInteractions).mock.calls.length
        }
    }
    const metrics = { sample, warmupCycles: 10, measuredCycles: 100, warmListeners, finalListeners: listeners.size,
        warmTimers, finalTimers: vi.getTimerCount(), closeCalls: nativeClose.mock.calls.length,
        obsoleteInteractionReads: vi.mocked(previewInteractions).mock.calls.length - interactionReadsAfterWarmup }
    if (import.meta.env.YUZORA_PERF_MEASURE) console.info("PREVIEW_CLOSE_LIFECYCLE", JSON.stringify(metrics))
    expect(metrics).toMatchObject({ warmListeners: 0, finalListeners: 0, warmTimers: 0, finalTimers: 0,
        closeCalls: 110, obsoleteInteractionReads: 0 })
})

it("preserves the normal 100 ms cadence and current-owner shortcuts", async () => {
    vi.useFakeTimers()
    vi.mocked(previewInteractions).mockResolvedValueOnce({ commands: ["nextTab"], selection: null, selecting: false })
    const view = renderHook(() => usePreviewInteractions(target))
    await act(async () => { await vi.advanceTimersByTimeAsync(0); await enqueueNativePreviewOperation(async () => {}) })
    expect(previewNavigationState).toHaveBeenCalledTimes(1)
    expect(previewInteractions).toHaveBeenCalledExactlyOnceWith("owner", expect.any(Array))
    expect(navigateWorkbenchTabs).toHaveBeenCalledExactlyOnceWith({ direction: 1 })
    await act(async () => { await vi.advanceTimersByTimeAsync(99) })
    expect(previewInteractions).toHaveBeenCalledTimes(1)
    await act(async () => { await vi.advanceTimersByTimeAsync(1); await enqueueNativePreviewOperation(async () => {}) })
    expect(previewInteractions).toHaveBeenCalledTimes(2)
    view.unmount()
    expect(vi.getTimerCount()).toBe(0)
})

it("preserves draining an in-flight interaction when the same owner is temporarily hidden", async () => {
    vi.useFakeTimers()
    const navigation = deferredNavigation()
    vi.mocked(previewNavigationState).mockReturnValueOnce(navigation.promise)
    vi.mocked(previewInteractions).mockResolvedValueOnce({ commands: ["nextTab"], selection: null, selecting: false })
    const { rerender } = renderHook(({ visible }) => usePreviewInteractions({ ...target, previewVisible: visible }), { initialProps: { visible: true } })
    await act(async () => { await vi.advanceTimersByTimeAsync(0); await settleMicrotasks() })
    rerender({ visible: false })
    await act(async () => {
        navigation.resolve({ sessionId: "owner", url, canGoBack: false, canGoForward: false })
        await enqueueNativePreviewOperation(async () => {})
    })
    expect(previewInteractions).toHaveBeenCalledExactlyOnceWith("owner", expect.any(Array))
    expect(nativeClose).not.toHaveBeenCalled()
    expect(navigateWorkbenchTabs).not.toHaveBeenCalled()
    expect(usePreviewStore.getState().nativeSession?.sessionId).toBe("owner")
    expect(vi.getTimerCount()).toBe(0)
})

it("lets a replacement owner poll without an extra evaluation of the previous owner", async () => {
    vi.useFakeTimers()
    const navigation = deferredNavigation()
    vi.mocked(previewNavigationState).mockReturnValueOnce(navigation.promise)
    const { rerender, unmount } = renderHook(({ id }) => usePreviewInteractions({ ...target, nativeSessionId: id }), { initialProps: { id: "owner" } })
    await act(async () => { await vi.advanceTimersByTimeAsync(0); await settleMicrotasks() })
    usePreviewStore.getState().recordNativeOpen("/workspace", url, "new-owner")
    vi.mocked(previewNavigationState).mockResolvedValue({ sessionId: "new-owner", url, canGoBack: false, canGoForward: false })
    rerender({ id: "new-owner" })
    await act(async () => {
        navigation.resolve({ sessionId: "owner", url, canGoBack: false, canGoForward: false })
        await enqueueNativePreviewOperation(async () => {})
    })
    expect(previewInteractions).toHaveBeenCalledExactlyOnceWith("new-owner", expect.any(Array))
    expect(previewNavigationState).toHaveBeenCalledTimes(2)
    unmount()
    expect(vi.getTimerCount()).toBe(0)
})

it("settles a failed close and leaves the shared queue usable without the obsolete read", async () => {
    vi.useFakeTimers()
    const navigation = deferredNavigation()
    vi.mocked(previewNavigationState).mockReturnValueOnce(navigation.promise)
    nativeClose.mockRejectedValueOnce(new Error("close failed"))
    useWorkspaceStore.getState().openPreviewTab()
    const view = renderHook(() => usePreviewInteractions(target))
    await act(async () => { await vi.advanceTimersByTimeAsync(0); await settleMicrotasks() })
    act(() => { useWorkspaceStore.getState().closePreviewTab(); view.unmount() })
    let followed = false
    await act(async () => {
        navigation.resolve({ sessionId: "owner", url, canGoBack: false, canGoForward: false })
        await enqueueNativePreviewOperation(async () => { followed = true })
    })
    expect(previewInteractions).not.toHaveBeenCalled()
    expect(nativeClose).toHaveBeenCalledTimes(1)
    expect(usePreviewStore.getState().nativeRequest).toBeNull()
    expect(followed).toBe(true)
})
