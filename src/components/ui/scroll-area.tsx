import * as React from "react"
import { ScrollArea as ScrollAreaPrimitive } from "radix-ui"

import { cn } from "@/lib/utils"

type ScrollAreaOrientation = "vertical" | "horizontal" | "both"

type ScrollAreaViewportProps = React.ComponentPropsWithoutRef<
  typeof ScrollAreaPrimitive.Viewport
> &
  React.HTMLAttributes<HTMLDivElement> & {
    "data-testid"?: string
  }

type ScrollAreaProps = React.ComponentProps<typeof ScrollAreaPrimitive.Root> & {
  /** Ref to the real scrollport (Radix Viewport). Use for scrollTop / onScroll / ResizeObserver. */
  viewportRef?: React.Ref<HTMLDivElement>
  /** Styles for the scrollport only (padding, size, focus). Do not put flex/gap child layout here — Radix inserts a table wrapper between Viewport and children. */
  viewportClassName?: string
  /** Layout wrapper around children (flex/gap/items). Preferred for content arrangement. */
  contentClassName?: string
  viewportProps?: ScrollAreaViewportProps
  /** Pure-reading surfaces: make the viewport keyboard-focusable. */
  focusable?: boolean
  /** Which custom scrollbars to mount. Default vertical. */
  orientation?: ScrollAreaOrientation
}

function ScrollArea({
  className,
  children,
  viewportRef,
  viewportClassName,
  contentClassName,
  viewportProps,
  focusable = false,
  orientation = "vertical",
  type = "auto",
  ...props
}: ScrollAreaProps) {
  const {
    className: viewportPropsClassName,
    tabIndex: viewportTabIndex,
    ...restViewportProps
  } = viewportProps ?? {}

  return (
    <ScrollAreaPrimitive.Root
      data-slot="scroll-area"
      type={type}
      className={cn("relative overflow-hidden", className)}
      {...props}
    >
      <ScrollAreaPrimitive.Viewport
        data-slot="scroll-area-viewport"
        ref={viewportRef}
        tabIndex={focusable ? (viewportTabIndex ?? 0) : viewportTabIndex}
        className={cn(
          // size-full: flex-bounded roots. max-*-inherit: Root max-h/max-w becomes a real scrollport bound.
          "size-full max-h-[inherit] max-w-[inherit] rounded-[inherit] transition-[color,box-shadow] outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:outline-1",
          viewportClassName,
          viewportPropsClassName
        )}
        {...restViewportProps}
      >
        {contentClassName ? (
          <div data-slot="scroll-area-content" className={contentClassName}>
            {children}
          </div>
        ) : (
          children
        )}
      </ScrollAreaPrimitive.Viewport>
      {(orientation === "vertical" || orientation === "both") && (
        <ScrollBar orientation="vertical" />
      )}
      {(orientation === "horizontal" || orientation === "both") && (
        <ScrollBar orientation="horizontal" />
      )}
      <ScrollAreaPrimitive.Corner />
    </ScrollAreaPrimitive.Root>
  )
}

function ScrollBar({
  className,
  orientation = "vertical",
  ...props
}: React.ComponentProps<typeof ScrollAreaPrimitive.ScrollAreaScrollbar>) {
  return (
    <ScrollAreaPrimitive.ScrollAreaScrollbar
      data-slot="scroll-area-scrollbar"
      data-orientation={orientation}
      orientation={orientation}
      className={cn(
        "flex touch-none p-px transition-colors select-none data-horizontal:h-2.5 data-horizontal:flex-col data-horizontal:border-t data-horizontal:border-t-transparent data-vertical:h-full data-vertical:w-2.5 data-vertical:border-l data-vertical:border-l-transparent",
        className
      )}
      {...props}
    >
      <ScrollAreaPrimitive.ScrollAreaThumb
        data-slot="scroll-area-thumb"
        className="relative flex-1 rounded-full bg-muted-foreground hover:bg-foreground/70 active:bg-foreground/80"
      />
    </ScrollAreaPrimitive.ScrollAreaScrollbar>
  )
}

export { ScrollArea, ScrollBar }
