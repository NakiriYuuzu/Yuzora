import type { ResizableDialogSize } from "@/hooks/useResizableDialogSize"
import { cn } from "@/lib/utils"

type DialogResizeHandlesProps = {
  sizing: ResizableDialogSize
  resizeLabel?: string
  resetSizeLabel?: string
}

/**
 * Domain-specific outer-frame handles for app modals. shadcn Resizable is a
 * panel-group primitive and is intentionally not used here.
 *
 * Hit targets stay fully inside the overflow-hidden dialog surface (no
 * translate outside the box) and keep a visible grip affordance.
 */
export function DialogResizeHandles({
  sizing,
  resizeLabel = "Resize dialog",
  resetSizeLabel = "Reset dialog size",
}: DialogResizeHandlesProps) {
  const widthNow = Math.round(sizing.size.width)
  const heightNow = Math.round(sizing.size.height)
  const title = `${resizeLabel}. ${resetSizeLabel}.`

  return (
    <>
      <div
        role="separator"
        tabIndex={0}
        data-slot="dialog-resize-handle"
        data-axis="x"
        data-affordance="edge"
        aria-orientation="vertical"
        aria-label={`${resizeLabel} width`}
        aria-valuemin={sizing.bounds.minWidth}
        aria-valuemax={sizing.bounds.maxWidth}
        aria-valuenow={widthNow}
        aria-valuetext={`${widthNow}px`}
        title={title}
        onPointerDown={(event) => sizing.beginPointerResize("x", event)}
        onPointerMove={sizing.onPointerMove}
        onPointerUp={sizing.onPointerUp}
        onPointerCancel={sizing.onPointerCancel}
        onLostPointerCapture={sizing.onLostPointerCapture}
        onKeyDown={(event) => sizing.onKeyboardResize("x", event)}
        onDoubleClick={(event) => {
          event.preventDefault()
          event.stopPropagation()
          sizing.resetSize()
        }}
        onClick={(event) => event.stopPropagation()}
        className={cn(
          "absolute top-2 right-0 bottom-2 z-[1] flex w-3 cursor-ew-resize touch-none select-none items-center justify-center outline-none",
          "hover:bg-foreground/5 focus-visible:bg-foreground/10",
        )}
      >
        <span
          data-slot="dialog-resize-grip"
          aria-hidden="true"
          className="h-10 w-1 rounded-full bg-foreground/30"
        />
      </div>
      <div
        role="separator"
        tabIndex={0}
        data-slot="dialog-resize-handle"
        data-axis="y"
        data-affordance="edge"
        aria-orientation="horizontal"
        aria-label={`${resizeLabel} height`}
        aria-valuemin={sizing.bounds.minHeight}
        aria-valuemax={sizing.bounds.maxHeight}
        aria-valuenow={heightNow}
        aria-valuetext={`${heightNow}px`}
        title={title}
        onPointerDown={(event) => sizing.beginPointerResize("y", event)}
        onPointerMove={sizing.onPointerMove}
        onPointerUp={sizing.onPointerUp}
        onPointerCancel={sizing.onPointerCancel}
        onLostPointerCapture={sizing.onLostPointerCapture}
        onKeyDown={(event) => sizing.onKeyboardResize("y", event)}
        onDoubleClick={(event) => {
          event.preventDefault()
          event.stopPropagation()
          sizing.resetSize()
        }}
        onClick={(event) => event.stopPropagation()}
        className={cn(
          "absolute right-2 bottom-0 left-2 z-[1] flex h-3 cursor-ns-resize touch-none select-none items-center justify-center outline-none",
          "hover:bg-foreground/5 focus-visible:bg-foreground/10",
        )}
      >
        <span
          data-slot="dialog-resize-grip"
          aria-hidden="true"
          className="h-1 w-10 rounded-full bg-foreground/30"
        />
      </div>
      <div
        role="separator"
        tabIndex={0}
        data-slot="dialog-resize-handle"
        data-axis="both"
        data-affordance="corner"
        aria-orientation="horizontal"
        aria-label={resizeLabel}
        aria-valuemin={sizing.bounds.minWidth}
        aria-valuemax={sizing.bounds.maxWidth}
        aria-valuenow={widthNow}
        aria-valuetext={`${widthNow}×${heightNow}px`}
        title={title}
        onPointerDown={(event) => sizing.beginPointerResize("both", event)}
        onPointerMove={sizing.onPointerMove}
        onPointerUp={sizing.onPointerUp}
        onPointerCancel={sizing.onPointerCancel}
        onLostPointerCapture={sizing.onLostPointerCapture}
        onKeyDown={(event) => sizing.onKeyboardResize("both", event)}
        onDoubleClick={(event) => {
          event.preventDefault()
          event.stopPropagation()
          sizing.resetSize()
        }}
        onClick={(event) => event.stopPropagation()}
        className={cn(
          "absolute right-0 bottom-0 z-[2] flex size-4 cursor-nwse-resize touch-none select-none items-center justify-center outline-none",
          "hover:bg-foreground/10 focus-visible:bg-foreground/15",
        )}
      >
        <span
          data-slot="dialog-resize-grip"
          aria-hidden="true"
          className="size-2.5 rounded-[2px] border border-foreground/40 bg-foreground/20"
        />
      </div>
    </>
  )
}
