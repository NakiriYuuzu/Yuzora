"use client"

import { useRef, type Ref } from "react"
import * as ResizablePrimitive from "react-resizable-panels"

import { cn } from "@/lib/utils"

function assignRef<T>(ref: Ref<T> | undefined, value: T) {
  if (typeof ref === "function") ref(value)
  else if (ref) ref.current = value
}

function ResizablePanelGroup({
  className,
  ...props
}: ResizablePrimitive.GroupProps) {
  return (
    <ResizablePrimitive.Group
      data-slot="resizable-panel-group"
      className={cn(
        "flex h-full w-full aria-[orientation=vertical]:flex-col",
        className
      )}
      {...props}
    />
  )
}

function ResizablePanel({
  onCollapse,
  onExpand,
  onResize,
  panelRef,
  elementRef,
  ...props
}: ResizablePrimitive.PanelProps & {
  onCollapse?: () => void
  onExpand?: () => void
}) {
  const collapsedRef = useRef<boolean | null>(null)
  const hasMeasuredExpandedRef = useRef(false)
  const apiRef = useRef<ResizablePrimitive.PanelImperativeHandle | null>(null)

  return (
    <ResizablePrimitive.Panel
      data-slot="resizable-panel"
      panelRef={(api) => {
        const wrapped = api
          ? {
              getSize: () => api.getSize(),
              isCollapsed: () => api.isCollapsed() || collapsedRef.current === true,
              resize: (size: number | string) => api.resize(size),
              collapse: () => {
                api.collapse()
                if (collapsedRef.current === true) return
                collapsedRef.current = true
                onCollapse?.()
              },
              expand: () => {
                api.expand()
                if (collapsedRef.current === false) return
                collapsedRef.current = false
                onExpand?.()
              }
            }
          : api
        apiRef.current = wrapped
        assignRef(panelRef, wrapped)
      }}
      elementRef={(node) => {
        if (node) {
          Object.defineProperty(node, "__yzPanel", {
            configurable: true,
            get: () => apiRef.current
          })
        }
        assignRef(elementRef, node)
      }}
      onResize={(size, id, prev) => {
        onResize?.(size, id, prev)
        // Percentage is the panel's authoritative layout size. During a fresh
        // Dialog mount the Group can briefly measure 0px even while this panel
        // still owns its non-zero default percentage; treating that transient
        // pixel projection as collapsed hides an otherwise expanded panel.
        const percentage = size.asPercentage
        const collapsed = Number.isFinite(percentage) && percentage <= 0
        // A Dialog can emit several 0% (or NaN when the Group is 0px)
        // measurements before its parent has a usable width. None are user
        // collapses: wait until this mounted panel has actually owned a finite,
        // non-zero percentage before reporting resize-driven transitions.
        // Imperative collapse() still reports immediately through the wrapped
        // API above.
        if (!hasMeasuredExpandedRef.current) {
          if (!Number.isFinite(percentage) || percentage <= 0) return
          const was = collapsedRef.current
          collapsedRef.current = false
          hasMeasuredExpandedRef.current = true
          if (was === true) onExpand?.()
          return
        }
        if (collapsedRef.current === collapsed) return
        collapsedRef.current = collapsed
        if (collapsed) onCollapse?.()
        else onExpand?.()
      }}
      {...props}
    />
  )
}

function ResizableHandle({
  withHandle,
  className,
  ...props
}: ResizablePrimitive.SeparatorProps & {
  withHandle?: boolean
}) {
  return (
    <ResizablePrimitive.Separator
      data-slot="resizable-handle"
      className={cn(
        "relative flex w-px items-center justify-center bg-border ring-offset-background after:absolute after:inset-y-0 after:left-1/2 after:w-1 after:-translate-x-1/2 focus-visible:ring-1 focus-visible:ring-ring focus-visible:outline-hidden aria-[orientation=horizontal]:h-px aria-[orientation=horizontal]:w-full aria-[orientation=horizontal]:after:left-0 aria-[orientation=horizontal]:after:h-1 aria-[orientation=horizontal]:after:w-full aria-[orientation=horizontal]:after:translate-x-0 aria-[orientation=horizontal]:after:-translate-y-1/2 [&[aria-orientation=horizontal]>div]:rotate-90",
        className
      )}
      {...props}
    >
      {withHandle && (
        <div className="z-10 flex h-6 w-1 shrink-0 rounded-lg bg-border" />
      )}
    </ResizablePrimitive.Separator>
  )
}

export { ResizableHandle, ResizablePanel, ResizablePanelGroup }
