import { useCallback, useEffect, useMemo, useRef, useState } from "react"

import {
  KEYBOARD_RESIZE_LARGE_STEP_PX,
  KEYBOARD_RESIZE_STEP_PX,
  applyResizeDelta,
  clampPixelSize,
  clearDialogSizePreference,
  defaultDialogSizePreference,
  dialogSizeBounds,
  getViewportSize,
  loadDialogSizePreference,
  preferenceFromSize,
  saveDialogSizePreference,
  sizeFromPreference,
  type DialogMinSize,
  type DialogPixelSize,
  type DialogSizeId,
  type DialogSizePreference,
} from "@/lib/dialogSize"

export type DialogResizeAxis = "x" | "y" | "both"

type DragState = {
  pointerId: number
  axis: DialogResizeAxis
  originX: number
  originY: number
  originSize: DialogPixelSize
  lastSize: DialogPixelSize
  moved: boolean
}

export type ResizableDialogSize = {
  resizeId: DialogSizeId
  size: DialogPixelSize
  preference: DialogSizePreference
  bounds: ReturnType<typeof dialogSizeBounds>
  isResizing: boolean
  style: React.CSSProperties
  beginPointerResize: (axis: DialogResizeAxis, event: React.PointerEvent<HTMLElement>) => void
  onPointerMove: (event: React.PointerEvent<HTMLElement>) => void
  onPointerUp: (event: React.PointerEvent<HTMLElement>) => void
  onPointerCancel: (event: React.PointerEvent<HTMLElement>) => void
  onLostPointerCapture: (event: React.PointerEvent<HTMLElement>) => void
  onKeyboardResize: (axis: DialogResizeAxis, event: React.KeyboardEvent<HTMLElement>) => void
  resetSize: () => void
}

export function useResizableDialogSize(options: {
  resizeId: DialogSizeId
  minSize?: DialogMinSize | null
}): ResizableDialogSize {
  const { resizeId, minSize = null } = options
  const [viewport, setViewport] = useState(() => getViewportSize())
  const [preference, setPreference] = useState<DialogSizePreference>(() =>
    loadDialogSizePreference(resizeId),
  )
  const [transientSize, setTransientSize] = useState<DialogPixelSize | null>(null)
  const [isResizing, setIsResizing] = useState(false)
  const dragRef = useRef<DragState | null>(null)
  const minSizeRef = useRef(minSize)

  // Keep the latest minSize available to window/pointer handlers without writing
  // refs during render (react-hooks/refs).
  useEffect(() => {
    minSizeRef.current = minSize
  }, [minSize])

  useEffect(() => {
    setPreference(loadDialogSizePreference(resizeId))
    setTransientSize(null)
    dragRef.current = null
    setIsResizing(false)
  }, [resizeId])

  useEffect(() => {
    const onResize = () => {
      // Window shrink only clamps the rendered pixels; never rewrite the stored
      // preference so a later enlarge restores the user's ratio.
      const nextViewport = getViewportSize()
      setViewport(nextViewport)
      const drag = dragRef.current
      if (!drag) return
      // Active pointer drags keep a transient pixel size. Clamp it immediately so
      // ARIA/current display never exceed the new max, and so release cannot
      // persist a stale oversized ratio (e.g. 900px into a 500px viewport).
      const clamped = clampPixelSize(drag.lastSize, nextViewport, minSizeRef.current)
      drag.lastSize = clamped
      setTransientSize(clamped)
    }
    window.addEventListener("resize", onResize)
    return () => window.removeEventListener("resize", onResize)
  }, [])

  const committedSize = useMemo(
    () => sizeFromPreference(preference, viewport, minSize),
    [preference, viewport, minSize],
  )
  const size = useMemo(() => {
    if (!transientSize) return committedSize
    // Defensive: even if a resize event races, never render outside bounds.
    return clampPixelSize(transientSize, viewport, minSize)
  }, [transientSize, committedSize, viewport, minSize])
  const bounds = useMemo(() => dialogSizeBounds(viewport, minSize), [viewport, minSize])

  const finishDrag = useCallback(
    (pointerId: number) => {
      const drag = dragRef.current
      if (!drag || drag.pointerId !== pointerId) return
      dragRef.current = null
      setIsResizing(false)
      setTransientSize(null)
      if (!drag.moved) return
      const viewportNow = getViewportSize()
      const clamped = clampPixelSize(drag.lastSize, viewportNow, minSizeRef.current)
      const nextPreference = preferenceFromSize(clamped, viewportNow)
      setPreference(nextPreference)
      saveDialogSizePreference(resizeId, nextPreference)
    },
    [resizeId],
  )

  const beginPointerResize = useCallback(
    (axis: DialogResizeAxis, event: React.PointerEvent<HTMLElement>) => {
      if (event.button !== 0) return
      event.preventDefault()
      event.stopPropagation()
      const originSize = sizeFromPreference(
        preference,
        getViewportSize(),
        minSizeRef.current,
      )
      event.currentTarget.setPointerCapture(event.pointerId)
      dragRef.current = {
        pointerId: event.pointerId,
        axis,
        originX: event.clientX,
        originY: event.clientY,
        originSize,
        lastSize: originSize,
        moved: false,
      }
      setTransientSize(originSize)
      setIsResizing(true)
    },
    [preference],
  )

  const onPointerMove = useCallback((event: React.PointerEvent<HTMLElement>) => {
    const drag = dragRef.current
    if (!drag || drag.pointerId !== event.pointerId) return
    event.preventDefault()
    event.stopPropagation()
    const viewportNow = getViewportSize()
    // Centered dialogs grow equally on both sides, so the dragged edge moves
    // by half the width/height delta. Double the pointer delta to keep the
    // edge under the cursor.
    const deltaX = drag.axis === "y" ? 0 : (event.clientX - drag.originX) * 2
    const deltaY = drag.axis === "x" ? 0 : (event.clientY - drag.originY) * 2
    const next = applyResizeDelta(
      drag.originSize,
      { width: deltaX, height: deltaY },
      viewportNow,
      minSizeRef.current,
    )
    drag.lastSize = next
    drag.moved =
      next.width !== drag.originSize.width || next.height !== drag.originSize.height
    setTransientSize(next)
  }, [])

  const onPointerUp = useCallback(
    (event: React.PointerEvent<HTMLElement>) => {
      event.preventDefault()
      event.stopPropagation()
      finishDrag(event.pointerId)
      if (event.currentTarget.hasPointerCapture(event.pointerId)) {
        event.currentTarget.releasePointerCapture(event.pointerId)
      }
    },
    [finishDrag],
  )

  const onPointerCancel = useCallback(
    (event: React.PointerEvent<HTMLElement>) => {
      event.stopPropagation()
      finishDrag(event.pointerId)
    },
    [finishDrag],
  )

  const onLostPointerCapture = useCallback(
    (event: React.PointerEvent<HTMLElement>) => {
      finishDrag(event.pointerId)
    },
    [finishDrag],
  )

  const onKeyboardResize = useCallback(
    (axis: DialogResizeAxis, event: React.KeyboardEvent<HTMLElement>) => {
      if (dragRef.current) return

      if (event.key === "Home") {
        event.preventDefault()
        event.stopPropagation()
        clearDialogSizePreference(resizeId)
        setPreference(defaultDialogSizePreference())
        setTransientSize(null)
        return
      }

      let deltaWidth = 0
      let deltaHeight = 0
      const step = event.shiftKey ? KEYBOARD_RESIZE_LARGE_STEP_PX : KEYBOARD_RESIZE_STEP_PX

      // Keyboard deltas are exact pixel steps. Pointer edge tracking still
      // doubles deltas for centered dialogs; keyboard must not.
      if (axis === "x" || axis === "both") {
        if (event.key === "ArrowRight") deltaWidth = step
        if (event.key === "ArrowLeft") deltaWidth = -step
      }
      if (axis === "y" || axis === "both") {
        if (event.key === "ArrowDown") deltaHeight = step
        if (event.key === "ArrowUp") deltaHeight = -step
      }
      if (deltaWidth === 0 && deltaHeight === 0) return

      event.preventDefault()
      event.stopPropagation()
      const viewportNow = getViewportSize()
      const current = sizeFromPreference(preference, viewportNow, minSizeRef.current)
      const next = applyResizeDelta(
        current,
        { width: deltaWidth, height: deltaHeight },
        viewportNow,
        minSizeRef.current,
      )
      const nextPreference = preferenceFromSize(next, viewportNow)
      setPreference(nextPreference)
      saveDialogSizePreference(resizeId, nextPreference)
    },
    [preference, resizeId],
  )

  const resetSize = useCallback(() => {
    clearDialogSizePreference(resizeId)
    setPreference(defaultDialogSizePreference())
    setTransientSize(null)
    dragRef.current = null
    setIsResizing(false)
  }, [resizeId])

  return {
    resizeId,
    size,
    preference,
    bounds,
    isResizing,
    style: {
      width: size.width,
      height: size.height,
      maxWidth: bounds.maxWidth,
      maxHeight: bounds.maxHeight,
    },
    beginPointerResize,
    onPointerMove,
    onPointerUp,
    onPointerCancel,
    onLostPointerCapture,
    onKeyboardResize,
    resetSize,
  }
}
