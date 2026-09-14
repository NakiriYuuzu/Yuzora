import { herdrErrorKind } from "@/lib/herdrErrors"
import { recordHerdrScrollMetric } from "./herdrScrollTelemetry"
import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react"
import { useTranslation } from "react-i18next"
import { ScrollArea } from "@/components/ui/scroll-area"
import { createPaneScrollController, offsetFromProxyScroll, proxyScrollTop, scrollProxyContentHeight, type PaneScrollController, type PaneScrollInfo } from "./herdrScrollController"
import { readPaneScroll, setPaneScroll } from "./herdrScrollIpc"

/** A shadcn coordinate proxy for HERDR's server-owned viewport. The spacer is
 * sized from official row counts; xterm itself never accumulates fake history.
 * Position changes are optimistic and dispatched immediately. */
export function HerdrScrollbar({ sessionName, paneId, enabled, canScroll, refreshRef, controllerRef, onError, viewportId }: {
  sessionName: string
  paneId: string
  enabled: boolean
  canScroll: () => boolean
  refreshRef: RefObject<((state?: PaneScrollInfo | null) => void) | null>
  controllerRef?: RefObject<PaneScrollController | null>
  onError?: (error: unknown) => void
  viewportId: string
}) {
  const { t } = useTranslation("terminalScroll")
  const viewport = useRef<HTMLDivElement>(null)
  const controller = useRef<ReturnType<typeof createPaneScrollController> | null>(null)
  const permission = useRef(canScroll)
  const errorHandler = useRef(onError)
  const synchronizedTop = useRef(0)
  const [state, setState] = useState<PaneScrollInfo | null>(null)
  const [height, setHeight] = useState(0)
  const [unavailableReason, setUnavailableReason] = useState<string | null>(null)
  const [writable, setWritable] = useState(false)
  useEffect(() => { permission.current = canScroll }, [canScroll])
  useEffect(() => { errorHandler.current = onError }, [onError])
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
      read: (signal) => readPaneScroll(sessionName, paneId, signal),
      write: (offset, signal) => setPaneScroll(sessionName, paneId, offset, signal),
      allowed: () => permission.current(),
      change: (next) => { setState(next); if (next) setUnavailableReason(null) },
      metric: (metric) => recordHerdrScrollMetric({ ...metric, session: sessionName, pane: paneId }),
      error: (error) => { setUnavailableReason(herdrErrorKind(error)); errorHandler.current?.(error) },
    })
    controller.current = active
    if (controllerRef) controllerRef.current = active
    const refresh = (next?: PaneScrollInfo | null) => {
      const writable = permission.current()
      setWritable(writable)
      if (!writable) { active.reset(); return }
      if (next !== undefined) active.sync(next)
      else void active.refresh()
    }
    refreshRef.current = refresh
    refresh()
    const timer = window.setInterval(refresh, 1000)
    return () => {
      window.clearInterval(timer)
      active.dispose()
      controller.current = null
      if (controllerRef?.current === active) controllerRef.current = null
      refreshRef.current = null
    }
  }, [enabled, sessionName, paneId, refreshRef, controllerRef])
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
    title={current ? t("hint") : t(unavailableReason ? `reason.${unavailableReason}` : "unavailable")}
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
