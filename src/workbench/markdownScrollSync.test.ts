import { expect, test, vi } from "vitest"

import {
    collectPreviewAnchors,
    createScrollSyncCoordinator,
    normalizeAnchors,
    previewOffsetToSourceLine,
    readEditorViewportTopLine,
    sourceLineToPreviewOffset,
    writeEditorViewportTopLine
} from "./markdownScrollSync"
import type { EditorScrollViewLike, SourceAnchor } from "./markdownScrollSync"

test("normalizeAnchors validates, sorts, de-duplicates, and synthesizes endpoints", () => {
    expect(normalizeAnchors([
        { line: 5, previewOffset: 400 },
        { line: 3, previewOffset: 180 },
        { line: 3, previewOffset: 160 },
        { line: 4, previewOffset: Number.NaN },
        { line: 0, previewOffset: 10 },
        { line: 9, previewOffset: 500 },
        { line: 2.5, previewOffset: 100 }
    ], 8, { startOffset: 0, endOffset: 700 })).toEqual([
        { line: 1, previewOffset: 0 },
        { line: 3, previewOffset: 160 },
        { line: 5, previewOffset: 400 },
        { line: 8, previewOffset: 700 }
    ])
})

test("normalizeAnchors drops descending DOM offsets and handles one mapped block", () => {
    expect(normalizeAnchors([
        { line: 2, previewOffset: 200 },
        { line: 4, previewOffset: 150 }
    ], 6, { endOffset: 600 })).toEqual([
        { line: 1, previewOffset: 0 },
        { line: 2, previewOffset: 200 },
        { line: 6, previewOffset: 600 }
    ])
    expect(normalizeAnchors([{ line: 4, previewOffset: 300 }], 8, { endOffset: 800 }))
        .toEqual([
            { line: 1, previewOffset: 0 },
            { line: 4, previewOffset: 300 },
            { line: 8, previewOffset: 800 }
        ])
    expect(normalizeAnchors([], 0)).toEqual([])
})

test("source/preview mapping interpolates adjacent anchors and clamps endpoints", () => {
    const anchors: SourceAnchor[] = [
        { line: 1, previewOffset: 0 },
        { line: 5, previewOffset: 200 },
        { line: 10, previewOffset: 700 }
    ]
    expect(sourceLineToPreviewOffset(-10, anchors)).toBe(0)
    expect(sourceLineToPreviewOffset(3, anchors)).toBe(100)
    expect(sourceLineToPreviewOffset(7.5, anchors)).toBe(450)
    expect(sourceLineToPreviewOffset(99, anchors)).toBe(700)
    expect(previewOffsetToSourceLine(-10, anchors)).toBe(1)
    expect(previewOffsetToSourceLine(100, anchors)).toBe(3)
    expect(previewOffsetToSourceLine(450, anchors)).toBe(7.5)
    expect(previewOffsetToSourceLine(999, anchors)).toBe(10)
    expect(sourceLineToPreviewOffset(2, [])).toBeNull()
    expect(previewOffsetToSourceLine(Number.POSITIVE_INFINITY, anchors)).toBeNull()
})

test("collectPreviewAnchors accepts only trusted finite integer markers with synthetic offsets", () => {
    const preview = document.createElement("div")
    Object.defineProperties(preview, {
        clientHeight: { configurable: true, value: 200 },
        scrollHeight: { configurable: true, value: 800 },
        scrollTop: { configurable: true, writable: true, value: 100 }
    })
    preview.getBoundingClientRect = () => ({ top: 20 } as DOMRect)

    function marker(line: string, top: number, trusted = true) {
        const element = document.createElement("p")
        element.setAttribute("data-yz-source-line", line)
        if (trusted) element.setAttribute("data-yz-source-anchor", "block")
        element.getBoundingClientRect = () => ({ top } as DOMRect)
        preview.append(element)
    }

    marker("4", 220)
    marker("4", 230)
    marker("2.5", 100)
    marker("7", Number.NaN)
    marker("3", 160, false) // raw HTML can copy the public line attr, but is untrusted.

    expect(collectPreviewAnchors(preview, 10)).toEqual([
        { line: 1, previewOffset: 0 },
        { line: 4, previewOffset: 300 },
        { line: 10, previewOffset: 600 }
    ])
})

test("CodeMirror adapters use documentTop/line blocks and do not touch selection or focus", () => {
    const scrollDOM = document.createElement("div")
    scrollDOM.getBoundingClientRect = () => ({ top: 40 } as DOMRect)
    const positions = [0, 10, 20, 30, 40]
    const doc = {
        lines: 5,
        line: (number: number) => ({ from: positions[number - 1] }),
        lineAt: (position: number) => ({ number: positions.indexOf(position) + 1 })
    }
    const view: EditorScrollViewLike = {
        documentTop: -20,
        scaleY: 2,
        scrollDOM,
        state: { doc },
        lineBlockAtHeight: vi.fn((height: number) => ({ from: positions[Math.floor(height / 20)] })),
        lineBlockAt: vi.fn((position: number) => ({ top: position * 2 }))
    }

    expect(readEditorViewportTopLine(view)).toBe(2)
    expect(view.lineBlockAtHeight).toHaveBeenCalledWith(30)
    expect(writeEditorViewportTopLine(view, 3.5)).toBe(50)
    expect(scrollDOM.scrollTop).toBe(50)
})

function coordinatorHarness() {
    const editorListeners = new Set<() => void>()
    const previewListeners = new Set<() => void>()
    const frames = new Map<number, FrameRequestCallback>()
    let nextFrame = 1
    let editorOffset = 0
    let editorLine = 1
    let previewOffset = 0
    const editorWrites = vi.fn((line: number) => {
        editorLine = line
        editorOffset = line * 10
        return editorOffset
    })
    const previewWrites = vi.fn((offset: number) => {
        previewOffset = offset
        return previewOffset
    })
    const cancelFrame = vi.fn((id: number) => void frames.delete(id))
    const anchors = [
        { line: 1, previewOffset: 0 },
        { line: 11, previewOffset: 1000 }
    ]
    const coordinator = createScrollSyncCoordinator({
        editor: {
            subscribeScroll: (listener) => {
                editorListeners.add(listener)
                return () => void editorListeners.delete(listener)
            },
            readOffset: () => editorOffset,
            readSourceLine: () => editorLine,
            writeSourceLine: editorWrites
        },
        preview: {
            subscribeScroll: (listener) => {
                previewListeners.add(listener)
                return () => void previewListeners.delete(listener)
            },
            readOffset: () => previewOffset,
            writeOffset: previewWrites
        },
        getAnchors: () => anchors,
        requestFrame: (callback) => {
            const id = nextFrame++
            frames.set(id, callback)
            return id
        },
        cancelFrame,
        feedbackTolerance: 2
    })

    return {
        coordinator,
        editorWrites,
        previewWrites,
        cancelFrame,
        editorListeners,
        previewListeners,
        setEditor(line: number, offset = line * 10) {
            editorLine = line
            editorOffset = offset
        },
        setPreview(offset: number) {
            previewOffset = offset
        },
        emitEditor() {
            editorListeners.forEach((listener) => listener())
        },
        emitPreview() {
            previewListeners.forEach((listener) => listener())
        },
        flushFrame() {
            const entry = frames.entries().next().value as [number, FrameRequestCallback] | undefined
            if (!entry) return
            frames.delete(entry[0])
            entry[1](0)
        },
        get frameCount() {
            return frames.size
        }
    }
}

test("coordinator coalesces to the latest unsuppressed driver", () => {
    const harness = coordinatorHarness()
    harness.setEditor(6)
    harness.emitEditor()
    harness.setPreview(800)
    harness.emitPreview()
    expect(harness.frameCount).toBe(1)

    harness.flushFrame()
    expect(harness.editorWrites).toHaveBeenCalledWith(9)
    expect(harness.previewWrites).not.toHaveBeenCalled()
    expect(harness.coordinator.getDriver()).toBe("preview")
})

test("coordinator suppresses matching programmatic feedback but keeps reverse user input", () => {
    const harness = coordinatorHarness()
    harness.setEditor(6)
    harness.emitEditor()
    harness.flushFrame()
    expect(harness.previewWrites).toHaveBeenLastCalledWith(500)

    harness.setPreview(501)
    harness.emitPreview()
    expect(harness.frameCount).toBe(0)
    expect(harness.editorWrites).not.toHaveBeenCalled()

    harness.setEditor(8)
    harness.emitEditor()
    harness.flushFrame()
    expect(harness.previewWrites).toHaveBeenLastCalledWith(700)

    // A user's immediate reverse scroll differs from the expected target, so it
    // becomes the new driver instead of being swallowed as feedback.
    harness.setPreview(300)
    harness.emitPreview()
    harness.flushFrame()
    expect(harness.editorWrites).toHaveBeenLastCalledWith(4)
})

test("coordinator resyncs preserved preview-driver position after anchor rebuild", () => {
    const harness = coordinatorHarness()
    harness.setPreview(400)
    harness.emitPreview()
    harness.flushFrame()
    harness.editorWrites.mockClear()
    harness.previewWrites.mockClear()

    harness.coordinator.resync(7)
    expect(harness.previewWrites).toHaveBeenCalledWith(600)
    expect(harness.editorWrites).toHaveBeenCalledWith(7)
})

test("coordinator cleanup removes listeners, cancels RAF, and prevents ghost sync", () => {
    const harness = coordinatorHarness()
    harness.setEditor(4)
    harness.emitEditor()
    expect(harness.frameCount).toBe(1)
    harness.coordinator.destroy()

    expect(harness.cancelFrame).toHaveBeenCalledTimes(1)
    expect(harness.editorListeners.size).toBe(0)
    expect(harness.previewListeners.size).toBe(0)
    harness.flushFrame()
    expect(harness.previewWrites).not.toHaveBeenCalled()
})
