import { afterEach, expect, test, vi } from "vitest"
import { EditorState } from "@codemirror/state"
import { EditorView } from "@codemirror/view"
import { registerView, unregisterView } from "../editor/viewRegistry"
import { subscribePreviewView } from "./subscribePreviewView"

const path = "/w/throttled.md"
let view: EditorView
let stop: (() => void) | undefined

afterEach(() => {
    stop?.()
    unregisterView(path)
    view?.destroy()
    vi.useRealTimers()
})

function setup() {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "performance"] })
    view = new EditorView({ state: EditorState.create({ doc: "" }) })
    registerView(path, view)
    const refresh = vi.fn()
    stop = subscribePreviewView(path, refresh)
    return refresh
}

async function type() {
    view.dispatch({ changes: { from: 0, insert: "x" } })
    await Promise.resolve()
}

test("leading edit is immediate; typing batches have only one trailing update per 400ms", async () => {
    const refresh = setup()
    await type()
    expect(refresh).toHaveBeenCalledTimes(1)
    for (let i = 0; i < 39; i++) {
        await vi.advanceTimersByTimeAsync(10)
        await type()
    }
    expect(refresh).toHaveBeenCalledTimes(1)
    await vi.advanceTimersByTimeAsync(10)
    expect(refresh).toHaveBeenCalledTimes(2)
    expect(vi.getTimerCount()).toBe(0)
    await vi.advanceTimersByTimeAsync(4000)
    expect(refresh).toHaveBeenCalledTimes(2)
    await type()
    expect(refresh).toHaveBeenCalledTimes(3)
})

test("unsubscribe cancels queued trailing work and detaches the editor listener", async () => {
    const refresh = setup()
    await type()
    await type()
    expect(vi.getTimerCount()).toBe(1)
    stop!()
    expect(vi.getTimerCount()).toBe(0)
    await vi.advanceTimersByTimeAsync(800)
    await type()
    expect(refresh).toHaveBeenCalledTimes(1)
    expect(view.state.facet(EditorView.updateListener)).toHaveLength(0)
})

test("view lifecycle bypasses the typing budget and cancels stale trailing updates", async () => {
    const refresh = setup()
    await type()
    await type()
    unregisterView(path, view)
    await Promise.resolve()
    expect(refresh).toHaveBeenCalledTimes(2)
    expect(vi.getTimerCount()).toBe(0)
    registerView(path, view)
    await Promise.resolve()
    expect(refresh).toHaveBeenCalledTimes(3)
    await vi.advanceTimersByTimeAsync(800)
    expect(refresh).toHaveBeenCalledTimes(3)
})
