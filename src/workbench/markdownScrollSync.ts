export interface SourceAnchor {
    line: number
    previewOffset: number
}

interface AnchorBounds {
    startOffset?: number
    endOffset?: number
}

export type ScrollSyncDriver = "editor" | "preview"

interface ScrollSubscription {
    subscribeScroll: (listener: () => void) => () => void
    readOffset: () => number | null
}

export interface ScrollSyncCoordinatorOptions {
    editor: ScrollSubscription & {
        readSourceLine: () => number | null
        writeSourceLine: (line: number) => number | null
    }
    preview: ScrollSubscription & {
        writeOffset: (offset: number) => number | null
    }
    getAnchors: () => readonly SourceAnchor[]
    requestFrame?: (callback: FrameRequestCallback) => number
    cancelFrame?: (handle: number) => void
    feedbackTolerance?: number
}

export interface ScrollSyncCoordinator {
    getDriver: () => ScrollSyncDriver
    snapshotSourceLine: () => number | null
    resync: (preservedSourceLine?: number) => void
    destroy: () => void
}

interface EditorDocumentLike {
    lines: number
    line: (lineNumber: number) => { from: number }
    lineAt: (position: number) => { number: number }
}

export interface EditorScrollViewLike {
    documentTop: number
    scaleY: number
    scrollDOM: HTMLElement
    state: { doc: EditorDocumentLike }
    lineBlockAtHeight: (height: number) => { from: number }
    lineBlockAt: (position: number) => { top: number }
}

function finite(value: number | null | undefined): value is number {
    return typeof value === "number" && Number.isFinite(value)
}

/**
 * Validate source markers, synthesize document endpoints, and return a sequence
 * that is monotonic in both source line and preview offset.
 */
export function normalizeAnchors(
    anchors: readonly SourceAnchor[],
    documentLineCount: number,
    bounds: AnchorBounds = {}
): SourceAnchor[] {
    if (!Number.isInteger(documentLineCount) || documentLineCount < 1) return []

    const startOffset = finite(bounds.startOffset) ? bounds.startOffset : 0
    const valid = anchors.filter((anchor) =>
        Number.isInteger(anchor.line)
        && anchor.line >= 1
        && anchor.line <= documentLineCount
        && finite(anchor.previewOffset)
    )
    const inferredEnd = valid.reduce(
        (maximum, anchor) => Math.max(maximum, anchor.previewOffset),
        startOffset
    )
    const endOffset = finite(bounds.endOffset)
        ? Math.max(startOffset, bounds.endOffset)
        : Math.max(startOffset, inferredEnd)

    const byLine = new Map<number, number>()
    for (const anchor of valid) {
        const offset = Math.min(endOffset, Math.max(startOffset, anchor.previewOffset))
        const existing = byLine.get(anchor.line)
        if (existing === undefined || offset < existing) byLine.set(anchor.line, offset)
    }

    // Synthetic endpoints deliberately win over a mapped block on the same line.
    // This makes leading/trailing blank space reachable without percentage mapping.
    byLine.set(1, startOffset)
    if (documentLineCount > 1) byLine.set(documentLineCount, endOffset)

    const sorted = [...byLine].map(([line, previewOffset]) => ({ line, previewOffset }))
        .sort((a, b) => a.line - b.line)

    const monotonic: SourceAnchor[] = []
    for (const anchor of sorted) {
        const previous = monotonic.at(-1)
        if (previous && anchor.previewOffset < previous.previewOffset) continue
        monotonic.push(anchor)
    }
    return monotonic
}

export function sourceLineToPreviewOffset(
    sourceLine: number,
    anchors: readonly SourceAnchor[]
): number | null {
    if (!finite(sourceLine) || anchors.length === 0) return null
    if (sourceLine <= anchors[0].line) return anchors[0].previewOffset
    const last = anchors[anchors.length - 1]
    if (sourceLine >= last.line) return last.previewOffset

    for (let index = 1; index < anchors.length; index++) {
        const upper = anchors[index]
        if (sourceLine > upper.line) continue
        const lower = anchors[index - 1]
        const span = upper.line - lower.line
        if (span <= 0) return upper.previewOffset
        const progress = (sourceLine - lower.line) / span
        return lower.previewOffset + progress * (upper.previewOffset - lower.previewOffset)
    }
    return last.previewOffset
}

export function previewOffsetToSourceLine(
    previewOffset: number,
    anchors: readonly SourceAnchor[]
): number | null {
    if (!finite(previewOffset) || anchors.length === 0) return null
    if (previewOffset <= anchors[0].previewOffset) return anchors[0].line
    const last = anchors[anchors.length - 1]
    if (previewOffset >= last.previewOffset) return last.line

    for (let index = 1; index < anchors.length; index++) {
        const upper = anchors[index]
        if (previewOffset > upper.previewOffset) continue
        const lower = anchors[index - 1]
        const span = upper.previewOffset - lower.previewOffset
        if (span <= 0) return upper.line
        const progress = (previewOffset - lower.previewOffset) / span
        return lower.line + progress * (upper.line - lower.line)
    }
    return last.line
}

export function collectPreviewAnchors(
    preview: HTMLElement,
    documentLineCount: number
): SourceAnchor[] {
    const previewRect = preview.getBoundingClientRect()
    const maximumOffset = Math.max(0, preview.scrollHeight - preview.clientHeight)
    const anchors: SourceAnchor[] = []

    for (const marker of preview.querySelectorAll<HTMLElement>(
        '[data-yz-source-anchor="block"][data-yz-source-line]'
    )) {
        const lineText = marker.getAttribute("data-yz-source-line") ?? ""
        if (!/^[1-9]\d*$/.test(lineText)) continue
        const line = Number(lineText)
        const markerRect = marker.getBoundingClientRect()
        const previewOffset = markerRect.top - previewRect.top + preview.scrollTop
        if (!finite(previewOffset)) continue
        anchors.push({ line, previewOffset })
    }

    return normalizeAnchors(anchors, documentLineCount, {
        startOffset: 0,
        endOffset: maximumOffset
    })
}

export function readEditorViewportTopLine(view: EditorScrollViewLike): number | null {
    const scaleY = finite(view.scaleY) && view.scaleY > 0 ? view.scaleY : 1
    const viewportTop = view.scrollDOM.getBoundingClientRect().top
    const heightFromDocumentTop = (viewportTop - view.documentTop) / scaleY
    if (!finite(heightFromDocumentTop)) return null
    const block = view.lineBlockAtHeight(Math.max(0, heightFromDocumentTop))
    const line = view.state.doc.lineAt(block.from).number
    return Number.isInteger(line) && line >= 1 ? line : null
}

export function writeEditorViewportTopLine(
    view: EditorScrollViewLike,
    sourceLine: number
): number | null {
    if (!finite(sourceLine) || view.state.doc.lines < 1) return null
    const clamped = Math.min(view.state.doc.lines, Math.max(1, sourceLine))
    const lowerLine = Math.floor(clamped)
    const upperLine = Math.ceil(clamped)
    const lower = view.lineBlockAt(view.state.doc.line(lowerLine).from).top
    const upper = view.lineBlockAt(view.state.doc.line(upperLine).from).top
    if (!finite(lower) || !finite(upper)) return null
    const target = lower + (upper - lower) * (clamped - lowerLine)
    view.scrollDOM.scrollTop = target
    return finite(view.scrollDOM.scrollTop) ? view.scrollDOM.scrollTop : null
}

export function createScrollSyncCoordinator(
    options: ScrollSyncCoordinatorOptions
): ScrollSyncCoordinator {
    const tolerance = finite(options.feedbackTolerance)
        ? Math.max(0, options.feedbackTolerance)
        : 2
    const requestFrame = options.requestFrame ?? ((callback) => requestAnimationFrame(callback))
    const cancelFrame = options.cancelFrame ?? ((handle) => cancelAnimationFrame(handle))
    let driver: ScrollSyncDriver = "editor"
    let frame: number | null = null
    let destroyed = false
    let lastSourceLine: number | null = null
    const expected: Record<ScrollSyncDriver, number | null> = {
        editor: null,
        preview: null
    }

    function setExpected(side: ScrollSyncDriver, target: number | null) {
        expected[side] = finite(target) ? target : null
    }

    function sourceLineFor(side: ScrollSyncDriver): number | null {
        if (side === "editor") return options.editor.readSourceLine()
        const offset = options.preview.readOffset()
        return finite(offset)
            ? previewOffsetToSourceLine(offset, options.getAnchors())
            : null
    }

    function runSync() {
        frame = null
        if (destroyed) return
        const sourceLine = sourceLineFor(driver)
        if (!finite(sourceLine)) return
        lastSourceLine = sourceLine

        if (driver === "editor") {
            const target = sourceLineToPreviewOffset(sourceLine, options.getAnchors())
            if (finite(target)) setExpected("preview", options.preview.writeOffset(target))
        } else {
            setExpected("editor", options.editor.writeSourceLine(sourceLine))
        }
    }

    function schedule() {
        if (destroyed || frame !== null) return
        frame = requestFrame(runSync)
    }

    function onScroll(side: ScrollSyncDriver) {
        if (destroyed) return
        const current = side === "editor"
            ? options.editor.readOffset()
            : options.preview.readOffset()
        const target = expected[side]
        if (target !== null) {
            expected[side] = null
            if (finite(current) && Math.abs(current - target) <= tolerance) return
        }

        driver = side
        const sourceLine = sourceLineFor(side)
        if (finite(sourceLine)) lastSourceLine = sourceLine
        schedule()
    }

    const unsubscribeEditor = options.editor.subscribeScroll(() => onScroll("editor"))
    const unsubscribePreview = options.preview.subscribeScroll(() => onScroll("preview"))

    return {
        getDriver: () => driver,
        snapshotSourceLine: () => {
            const current = sourceLineFor(driver)
            return finite(current) ? current : lastSourceLine
        },
        resync: (preservedSourceLine) => {
            if (destroyed) return
            const sourceLine = finite(preservedSourceLine)
                ? preservedSourceLine
                : sourceLineFor(driver) ?? lastSourceLine
            if (!finite(sourceLine)) return
            lastSourceLine = sourceLine

            if (driver === "preview" && finite(preservedSourceLine)) {
                const previewTarget = sourceLineToPreviewOffset(sourceLine, options.getAnchors())
                if (finite(previewTarget)) {
                    setExpected("preview", options.preview.writeOffset(previewTarget))
                }
            }

            if (driver === "preview") {
                setExpected("editor", options.editor.writeSourceLine(sourceLine))
            } else {
                const previewTarget = sourceLineToPreviewOffset(sourceLine, options.getAnchors())
                if (finite(previewTarget)) {
                    setExpected("preview", options.preview.writeOffset(previewTarget))
                }
            }
        },
        destroy: () => {
            if (destroyed) return
            destroyed = true
            unsubscribeEditor()
            unsubscribePreview()
            if (frame !== null) cancelFrame(frame)
            frame = null
            expected.editor = null
            expected.preview = null
        }
    }
}
