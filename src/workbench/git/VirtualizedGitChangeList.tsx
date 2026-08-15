import {
    forwardRef,
    useCallback,
    useEffect,
    useImperativeHandle,
    useLayoutEffect,
    useMemo,
    useRef,
    useState,
    type CSSProperties,
    type ReactNode,
    type UIEvent
} from "react"

import { ScrollArea } from "@/components/ui/scroll-area"

import {
    GIT_SECTION_ORDER,
    gitChangeId,
    type GitChangeModel,
    type GitChangeRow,
    type GitSectionKey
} from "./gitChangeSelection"

export const GIT_CHANGE_ROW_HEIGHT = 32
export const GIT_CHANGE_HEADER_HEIGHT = 30
export const GIT_CHANGE_OVERSCAN = 10
const DEFAULT_VIEWPORT_HEIGHT = 600

export type GitChangeVirtualItem =
    | { kind: "section"; key: string; section: GitSectionKey; rows: readonly GitChangeRow[]; height: number }
    | { kind: "row"; key: string; section: GitSectionKey; row: GitChangeRow; height: number }

export function gitChangeVirtualItems(
    model: GitChangeModel,
    options: {
        openSections?: Readonly<Record<GitSectionKey, boolean>>
        rowMatches?: (row: GitChangeRow) => boolean
    } = {}
): GitChangeVirtualItem[] {
    const items: GitChangeVirtualItem[] = []
    for (const section of GIT_SECTION_ORDER) {
        const rows = options.rowMatches
            ? model.buckets[section].filter(options.rowMatches)
            : model.buckets[section]
        if (rows.length === 0) continue
        items.push({
            kind: "section",
            key: `section:${section}`,
            section,
            rows,
            height: GIT_CHANGE_HEADER_HEIGHT
        })
        if (options.openSections && !options.openSections[section]) continue
        for (const row of rows) {
            items.push({
                kind: "row",
                key: gitChangeId(row),
                section,
                row,
                height: GIT_CHANGE_ROW_HEIGHT
            })
        }
    }
    return items
}

export interface VirtualizedGitChangeListHandle {
    scrollToIndex: (index: number, focusId?: string) => void
    scrollToKey: (key: string, focusId?: string) => void
    viewport: () => HTMLDivElement | null
}

function offsetsFor(items: readonly GitChangeVirtualItem[]): number[] {
    const offsets = new Array<number>(items.length + 1)
    offsets[0] = 0
    for (let index = 0; index < items.length; index += 1) {
        offsets[index + 1] = offsets[index] + items[index].height
    }
    return offsets
}

function firstItemAfter(offsets: readonly number[], value: number): number {
    let low = 0
    let high = offsets.length - 1
    while (low < high) {
        const mid = Math.floor((low + high) / 2)
        if (offsets[mid] < value) low = mid + 1
        else high = mid
    }
    return low
}

function itemAtOffset(offsets: readonly number[], value: number): number {
    return Math.max(0, Math.min(offsets.length - 2, firstItemAfter(offsets, value + 1) - 1))
}

function deriveWindow(
    items: readonly GitChangeVirtualItem[],
    offsets: readonly number[],
    scrollTop: number,
    viewportHeight: number
) {
    if (items.length === 0) return { start: 0, end: 0 }
    const height = viewportHeight || DEFAULT_VIEWPORT_HEIGHT
    const first = itemAtOffset(offsets, Math.max(0, scrollTop))
    const afterLast = Math.min(items.length, firstItemAfter(offsets, scrollTop + height))
    return {
        start: Math.max(0, first - GIT_CHANGE_OVERSCAN),
        end: Math.min(items.length, afterLast + GIT_CHANGE_OVERSCAN)
    }
}

export const VirtualizedGitChangeList = forwardRef<VirtualizedGitChangeListHandle, {
    items: readonly GitChangeVirtualItem[]
    renderItem: (item: GitChangeVirtualItem, index: number, style: CSSProperties) => ReactNode
    className?: string
    viewportClassName?: string
    testId?: string
    spacerTestId?: string
    contentRole?: string
    contentAriaLabel?: string
    activeDescendant?: string
    pinnedKey?: string
    onKeyDown?: (event: React.KeyboardEvent<HTMLDivElement>) => void
    resetKey?: string
}>(function VirtualizedGitChangeList({
    items,
    renderItem,
    className,
    viewportClassName,
    testId,
    spacerTestId,
    contentRole,
    contentAriaLabel,
    activeDescendant,
    pinnedKey,
    onKeyDown,
    resetKey
}, ref) {
    const viewportRef = useRef<HTMLDivElement | null>(null)
    const rafRef = useRef<number | null>(null)
    const pendingFocusRef = useRef<string | null>(null)
    const previousRef = useRef<{ items: readonly GitChangeVirtualItem[]; offsets: number[] } | null>(null)
    const previousResetKeyRef = useRef(resetKey)
    const offsets = useMemo(() => offsetsFor(items), [items])
    const geometry = useMemo(() => ({
        items,
        offsets,
        indexByKey: new Map(items.map((item, index) => [item.key, index]))
    }), [items, offsets])
    const geometryRef = useRef(geometry)
    geometryRef.current = geometry
    const totalHeight = offsets[offsets.length - 1] ?? 0
    const [window, setWindow] = useState({ start: 0, end: 0 })

    const commitWindow = useCallback((scrollTop: number, viewportHeight: number) => {
        const live = geometryRef.current
        const next = deriveWindow(live.items, live.offsets, scrollTop, viewportHeight)
        setWindow((current) => current.start === next.start && current.end === next.end ? current : next)
    }, [])

    const measureNow = useCallback(() => {
        const viewport = viewportRef.current
        if (!viewport) return
        commitWindow(viewport.scrollTop, viewport.clientHeight)
    }, [commitWindow])

    const scheduleMeasure = useCallback(() => {
        if (rafRef.current != null) return
        rafRef.current = requestAnimationFrame(() => {
            rafRef.current = null
            measureNow()
        })
    }, [measureNow])

    useImperativeHandle(ref, () => {
        const scrollToIndex = (index: number, focusId?: string) => {
            const viewport = viewportRef.current
            const live = geometryRef.current
            if (!viewport || index < 0 || index >= live.items.length) return
            const top = live.offsets[index]
            const bottom = live.offsets[index + 1]
            const viewportHeight = viewport.clientHeight || DEFAULT_VIEWPORT_HEIGHT
            if (top < viewport.scrollTop) viewport.scrollTop = top
            else if (bottom > viewport.scrollTop + viewportHeight) {
                viewport.scrollTop = Math.max(0, bottom - viewportHeight)
            }
            if (focusId) {
                const mounted = document.getElementById(focusId)
                if (mounted) mounted.focus()
                else pendingFocusRef.current = focusId
            }
            commitWindow(viewport.scrollTop, viewportHeight)
        }
        return {
            scrollToIndex,
            scrollToKey: (key, focusId) => {
                const index = geometryRef.current.indexByKey.get(key)
                if (index != null) scrollToIndex(index, focusId)
            },
            viewport: () => viewportRef.current
        }
    }, [commitWindow])

    useEffect(() => {
        const viewport = viewportRef.current
        if (!viewport) return
        measureNow()
        const observer = new ResizeObserver(scheduleMeasure)
        observer.observe(viewport)
        return () => {
            observer.disconnect()
            if (rafRef.current != null) cancelAnimationFrame(rafRef.current)
            rafRef.current = null
        }
    }, [measureNow, scheduleMeasure])

    useLayoutEffect(() => {
        const viewport = viewportRef.current
        const previous = previousRef.current
        const resetChanged = previousResetKeyRef.current !== resetKey
        previousResetKeyRef.current = resetKey
        if (viewport && resetChanged) {
            viewport.scrollTop = 0
        } else if (viewport && previous && previous.items !== items && previous.items.length && items.length) {
            const oldIndex = itemAtOffset(previous.offsets, viewport.scrollTop)
            const oldItem = previous.items[oldIndex]
            const withinItem = viewport.scrollTop - previous.offsets[oldIndex]
            let newIndex = oldItem ? geometry.indexByKey.get(oldItem.key) ?? -1 : -1
            if (newIndex < 0) {
                for (let distance = 1; distance < previous.items.length && newIndex < 0; distance += 1) {
                    const before = previous.items[oldIndex - distance]
                    const after = previous.items[oldIndex + distance]
                    if (before) newIndex = geometry.indexByKey.get(before.key) ?? -1
                    if (newIndex < 0 && after) newIndex = geometry.indexByKey.get(after.key) ?? -1
                }
            }
            if (newIndex >= 0) {
                viewport.scrollTop = Math.max(
                    0,
                    offsets[newIndex] + Math.min(withinItem, items[newIndex].height - 1)
                )
            }
        }
        previousRef.current = { items, offsets }
        if (viewport) commitWindow(viewport.scrollTop, viewport.clientHeight)
    }, [commitWindow, geometry, items, offsets, resetKey])

    useLayoutEffect(() => {
        const focusId = pendingFocusRef.current
        if (!focusId) return
        const target = document.getElementById(focusId)
        if (target) {
            pendingFocusRef.current = null
            target.focus()
        }
    }, [items, window.start, window.end])

    function onScroll(event: UIEvent<HTMLDivElement>) {
        scheduleMeasure()
        if (event.currentTarget !== viewportRef.current) return
    }

    const start = Math.min(window.start, items.length)
    const end = Math.max(start, Math.min(window.end || Math.min(items.length, 32), items.length))
    const renderedIndexes = items.slice(start, end).map((_, sliceIndex) => start + sliceIndex)
    const pinnedIndex = pinnedKey ? geometry.indexByKey.get(pinnedKey) ?? -1 : -1
    if (pinnedIndex >= 0 && (pinnedIndex < start || pinnedIndex >= end)) renderedIndexes.push(pinnedIndex)

    return (
        <ScrollArea
            data-testid={testId}
            className={className}
            viewportRef={viewportRef}
            viewportClassName={viewportClassName}
            viewportProps={{ onScroll, onKeyDown }}
        >
            <div
                role={contentRole}
                aria-label={contentAriaLabel}
                aria-activedescendant={activeDescendant}
                className="relative w-full"
                data-testid={spacerTestId}
                data-virtual-total-height={totalHeight}
                style={{ height: totalHeight, minHeight: totalHeight }}
            >
                {renderedIndexes.map((index) => {
                    const item = items[index]
                    return renderItem(item, index, {
                        position: "absolute",
                        top: offsets[index],
                        left: 0,
                        right: 0,
                        height: item.height
                    })
                })}
            </div>
        </ScrollArea>
    )
})
