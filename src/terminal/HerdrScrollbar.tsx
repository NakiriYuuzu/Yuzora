import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react"
import { useTranslation } from "react-i18next"
import { ScrollArea } from "@/components/ui/scroll-area"
import { createPaneScrollController, offsetFromProxyScroll, proxyScrollTop, scrollProxyContentHeight, type PaneScrollInfo } from "./herdrScrollController"
import { readPaneScroll, setPaneScroll } from "./herdrScrollIpc"

/** A shadcn coordinate proxy for HERDR's server-owned viewport. The spacer is
 * sized from official row counts; xterm itself never accumulates fake history. */
export function HerdrScrollbar({ sessionName, paneId, enabled, canScroll, refreshRef, viewportId }: {
  sessionName: string
  paneId: string
  enabled: boolean
  canScroll: () => boolean
  refreshRef: RefObject<(() => void) | null>
  viewportId: string
}) {
  const { t } = useTranslation("terminalScroll")
  const viewport = useRef<HTMLDivElement>(null)
  const controller = useRef<ReturnType<typeof createPaneScrollController> | null>(null)
  const permission = useRef(canScroll)
  const synchronizedTop = useRef(0)
  const [state, setState] = useState<PaneScrollInfo | null>(null)
  const [height, setHeight] = useState(0)
  const [writable, setWritable] = useState(false)
  useEffect(() => { permission.current = canScroll }, [canScroll])
  useLayoutEffect(() => {
    const element = viewport.current
    if (!element) return
    const resize = () => setHeight(element.clientHeight)
    resize()
    const observer = new ResizeObserver(resize)
    observer.observe(element)
    return () => observer.disconnect()
  }, [])
  useEffect(() => {
    if (!enabled) return
    const active = createPaneScrollController({
      read: () => readPaneScroll(sessionName, paneId),
      write: (offset) => setPaneScroll(sessionName, paneId, offset),
      allowed: () => permission.current(),
      change: setState,
    })
    controller.current = active
    const refresh = () => { const writable = permission.current(); setWritable(writable); if (writable) void active.refresh() }
    refreshRef.current = refresh
    refresh()
    const timer = window.setInterval(refresh, 1000)
    return () => {
      window.clearInterval(timer)
      active.dispose()
      controller.current = null
      refreshRef.current = null
    }
  }, [enabled, sessionName, paneId, refreshRef])
  const current = enabled && writable ? state : null
  const disabled = !current || current.maxOffsetFromBottom === 0
  useLayoutEffect(() => {
    const element = viewport.current
    if (!element) return
    synchronizedTop.current = current ? proxyScrollTop(current, element.scrollHeight - element.clientHeight) : 0
    element.scrollTop = synchronizedTop.current
    synchronizedTop.current = element.scrollTop // Accept browser clamping/rounding without echoing a write.
  }, [current, height])
  return <ScrollArea
    className="h-full w-2.5 shrink-0 [&_[data-slot=scroll-area-scrollbar]]:w-2.5 data-[disabled=true]:[&_[data-slot=scroll-area-scrollbar]]:pointer-events-none"
    orientation="vertical" type="auto" viewportRef={viewport}
    role={current ? "scrollbar" : "note"} aria-label={t(current ? "label" : "unavailable")}
    aria-controls={viewportId} aria-orientation={current ? "vertical" : undefined}
    aria-valuemin={current ? 0 : undefined} aria-valuemax={current?.maxOffsetFromBottom}
    aria-valuenow={current ? current.maxOffsetFromBottom - current.offsetFromBottom : undefined}
    aria-disabled={disabled} data-disabled={disabled} tabIndex={disabled ? -1 : 0}
    title={t(current ? "hint" : "unavailable")}
    viewportProps={{
      "data-testid": "herdr-scroll-proxy",
      onScroll: (event) => {
        const element = event.currentTarget
        // Hydration is a server read, never another write back to that server.
        if (Math.abs(element.scrollTop - synchronizedTop.current) < 0.5) return
        if (!current || disabled || !permission.current()) {
          element.scrollTop = synchronizedTop.current
          return
        }
        synchronizedTop.current = element.scrollTop
        controller.current?.move(offsetFromProxyScroll(current, element.scrollTop, element.scrollHeight - element.clientHeight))
      },
    }}
    onKeyDown={(event) => {
      if (disabled || !current) return
      const delta = event.key === "ArrowUp" ? 1 : event.key === "ArrowDown" ? -1 : event.key === "PageUp" ? current.viewportRows : event.key === "PageDown" ? -current.viewportRows : 0
      const offset = event.key === "Home" ? current.maxOffsetFromBottom : event.key === "End" ? 0 : delta ? current.offsetFromBottom + delta : null
      if (offset === null) return
      event.preventDefault(); event.stopPropagation(); controller.current?.move(offset)
    }}>
    <div aria-hidden="true" data-herdr-scroll-extent style={{ height: current ? scrollProxyContentHeight(current, height) : height, width: 1 }} />
  </ScrollArea>
}
