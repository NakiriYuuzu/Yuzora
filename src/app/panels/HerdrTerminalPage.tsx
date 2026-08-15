import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode
} from "react"
import { FitAddon } from "@xterm/addon-fit"
import { Terminal } from "@xterm/xterm"
import { useTranslation } from "react-i18next"

import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup
} from "@/components/ui/resizable"
import { herdrAttachmentKey, herdrPagePath } from "@/lib/herdrPages"
import {
  herdrLayoutExport,
  herdrLayoutSetSplitRatio,
  herdrPaneFocus
} from "@/lib/herdrIpc"
import type {
  HerdrLayoutDescription,
  HerdrLayoutNode,
  HerdrTerminalMode,
  HerdrTerminalRole
} from "@/lib/herdrTypes"
import { useHerdrStore } from "@/state/herdrStore"
import { useTerminalSettingsStore } from "@/state/terminalSettingsStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { contextMenuHandler } from "@/state/contextMenuStore"
import { installTerminalImeHandling } from "@/terminal/terminalImeHandling"
import {
  TerminalOutputQueue,
  registerTerminalOutputQueue,
  unregisterTerminalOutputQueue
} from "@/terminal/terminalOutputQueue"
import {
  createHerdrTerminalTransport,
  normalizeTerminalWheelRows,
  type TerminalTransportEvent
} from "@/terminal/terminalTransport"
import { buildXtermTheme } from "@/terminal/xtermTheme"
import {
  installTerminalTargetOpen,
  resolveHerdrTerminalBaseCwd
} from "@/terminal/terminalTarget"

export interface HerdrTerminalPageProps {
  herdrSessionId: string
  terminalId: string
  paneId?: string | null
  herdrTabId?: string | null
  title?: string
  pagePath?: string
  active: boolean
  visible?: boolean
}

type TerminalMode = "light" | "dark"

const defaultCols = 80
const defaultRows = 24
const RATIO_EPSILON = 0.01
const RATIO_DEBOUNCE_MS = 120

function currentMode(): TerminalMode {
  return document.documentElement.classList.contains("dark") ? "dark" : "light"
}

function safeFit(fitAddon: FitAddon): void {
  try {
    fitAddon.fit()
  } catch {
    // jsdom / hidden panes
  }
}

function safeFocus(term: Terminal): void {
  try {
    term.focus()
  } catch {
    // jsdom
  }
}

function terminalSize(term: Terminal): { cols: number; rows: number } {
  return {
    cols: term.cols || defaultCols,
    rows: term.rows || defaultRows
  }
}

/** Resolve legacy `live` to the concrete default named session. */
function resolveSessionName(
  sessions: Array<{ name: string; default: boolean }>,
  herdrSessionId: string
): string | null {
  if (herdrSessionId !== "live") return herdrSessionId
  return (sessions.find((s) => s.default) ?? sessions[0])?.name ?? null
}

/** Resolve named session running flag; `live` maps to the default session entry. */
function resolveSessionRunning(
  sessions: Array<{ name: string; default: boolean; running: boolean }>,
  herdrSessionId: string
): boolean | null {
  if (sessions.length === 0) return null
  const resolvedName = resolveSessionName(sessions, herdrSessionId)
  const match = sessions.find((s) => s.name === resolvedName)
  return match?.running ?? null
}

function collectPaneIds(node: HerdrLayoutNode, out: string[] = []): string[] {
  if (node.type === "pane") {
    if (node.paneId) out.push(node.paneId)
    return out
  }
  collectPaneIds(node.first, out)
  collectPaneIds(node.second, out)
  return out
}

function pathKey(path: boolean[]): string {
  return path.map((bit) => (bit ? "1" : "0")).join("")
}

/**
 * Herdr tab surface — one Yuzora page per Herdr tab.
 * Default attachment is control+takeover. Unmount/close releases connectors only.
 */
export function HerdrTerminalPage({
  herdrSessionId,
  terminalId,
  paneId = null,
  herdrTabId = null,
  title,
  pagePath: pagePathProp,
  active,
  visible = true
}: HerdrTerminalPageProps) {
  const { t } = useTranslation("workbench")
  const pagePath = useMemo(
    () => pagePathProp ?? herdrPagePath(herdrSessionId, terminalId),
    [pagePathProp, herdrSessionId, terminalId]
  )
  const sessions = useHerdrStore((s) => s.sessions)
  const selectedSessionName = useHerdrStore((s) => s.selectedSessionName)
  const selectedSnapshot = useHerdrStore((s) => s.snapshot)
  const selectedCapabilities = useHerdrStore((s) => s.capabilities)
  const runtimesBySession = useHerdrStore((s) => s.runtimesBySession)
  const topologyRevision = useHerdrStore((s) => s.topologyRevision)
  const attachments = useHerdrStore((s) => s.attachments)
  const targetSessionName = useMemo(
    () => resolveSessionName(sessions, herdrSessionId),
    [sessions, herdrSessionId]
  )
  const targetRuntime = targetSessionName
    ? runtimesBySession[targetSessionName]
    : undefined
  const snapshot =
    targetRuntime?.snapshot ??
    (targetSessionName === selectedSessionName ? selectedSnapshot : null)
  const targetCapabilities =
    targetRuntime?.capabilities ??
    (targetSessionName === selectedSessionName ? selectedCapabilities : null)

  const sessionRunning = useMemo(
    () => resolveSessionRunning(sessions, herdrSessionId),
    [sessions, herdrSessionId]
  )
  const sessionCanConnect = sessionRunning === true
  const sessionIsStopped = sessionRunning === false
  const [hasConnectedSession, setHasConnectedSession] = useState(sessionCanConnect)

  const resolvedTabId = useMemo(() => {
    if (herdrTabId) return herdrTabId
    const fromTerminal = snapshot?.terminals.find(
      (item) =>
        item.terminalId === terminalId ||
        (paneId && item.paneId === paneId)
    )
    if (fromTerminal?.tabId) return fromTerminal.tabId
    const fromAgent = snapshot?.agents.find(
      (item) =>
        item.terminalId === terminalId ||
        (paneId && item.paneId === paneId)
    )
    return fromAgent?.tabId ?? null
  }, [herdrTabId, snapshot, terminalId, paneId])

  const [layout, setLayout] = useState<HerdrLayoutDescription | null>(null)
  const [layoutError, setLayoutError] = useState<string | null>(null)
  const [layoutReady, setLayoutReady] = useState(!sessionCanConnect)
  const [surfaceSessionRunning, setSurfaceSessionRunning] = useState(sessionRunning)
  const suppressRatioWriteRef = useRef(true)
  const ratioTimersRef = useRef<Map<string, number>>(new Map())
  const lastWrittenRatioRef = useRef<Map<string, number>>(new Map())
  const layoutLoadGenerationRef = useRef(0)
  const updateHerdrPageTabId = useWorkspaceStore((s) => s.updateHerdrPageTabId)

  const sessionNameArg = herdrSessionId === "live" ? null : herdrSessionId

  useEffect(() => {
    if (resolvedTabId) updateHerdrPageTabId(pagePath, resolvedTabId)
  }, [pagePath, resolvedTabId, updateHerdrPageTabId])

  const reloadLayout = useCallback(async () => {
    const generation = ++layoutLoadGenerationRef.current
    suppressRatioWriteRef.current = true
    for (const timer of ratioTimersRef.current.values()) {
      window.clearTimeout(timer)
    }
    ratioTimersRef.current.clear()
    lastWrittenRatioRef.current.clear()
    if (sessionIsStopped || (!sessionCanConnect && !hasConnectedSession)) {
      setLayout(null)
      setLayoutReady(true)
      return
    }
    if (!sessionCanConnect) {
      // Inventory refresh is transient, not topology teardown. Preserve the
      // last authoritative BSP tree and its mounted connector leaves.
      setLayoutReady(true)
      return
    }
    setLayoutReady(false)
    try {
      const next = await herdrLayoutExport({
        sessionName: sessionNameArg,
        tabId: resolvedTabId,
        paneId: resolvedTabId ? null : paneId
      })
      if (generation !== layoutLoadGenerationRef.current) return
      setLayout(next)
      setLayoutError(null)
      setHasConnectedSession(true)
      setSurfaceSessionRunning(true)
      // Allow ratio writes only after this hydration settles.
      window.setTimeout(() => {
        if (generation === layoutLoadGenerationRef.current) {
          suppressRatioWriteRef.current = false
        }
      }, 0)
    } catch (error) {
      if (generation !== layoutLoadGenerationRef.current) return
      // Legacy / single-pane fallback when layout.export is unavailable.
      setLayout(null)
      setLayoutError(error instanceof Error ? error.message : String(error))
      setHasConnectedSession(true)
      setSurfaceSessionRunning(true)
    } finally {
      if (generation === layoutLoadGenerationRef.current) setLayoutReady(true)
    }
  }, [
    paneId,
    resolvedTabId,
    sessionCanConnect,
    sessionIsStopped,
    sessionNameArg,
    hasConnectedSession
  ])

  useEffect(() => {
    let cancelled = false
    queueMicrotask(() => {
      if (!cancelled) void reloadLayout()
    })
    return () => {
      cancelled = true
    }
  }, [reloadLayout, topologyRevision])

  useEffect(() => {
    const ratioTimers = ratioTimersRef.current
    return () => {
      layoutLoadGenerationRef.current += 1
      for (const timer of ratioTimers.values()) {
        window.clearTimeout(timer)
      }
      ratioTimers.clear()
    }
  }, [])

  const paneToTerminal = useMemo(() => {
    const map = new Map<string, string>()
    for (const term of snapshot?.terminals ?? []) {
      if (term.paneId) map.set(term.paneId, term.terminalId)
    }
    for (const agent of snapshot?.agents ?? []) {
      if (agent.paneId && agent.terminalId) map.set(agent.paneId, agent.terminalId)
    }
    return map
  }, [snapshot])

  const onSplitRatioChanged = useCallback(
    (splitPath: boolean[], ratio: number) => {
      if (suppressRatioWriteRef.current) return
      if (!sessionCanConnect) return
      if (!(ratio >= 0 && ratio <= 1)) return
      const key = pathKey(splitPath)
      const previous = lastWrittenRatioRef.current.get(key)
      if (previous !== undefined && Math.abs(previous - ratio) < RATIO_EPSILON) return
      const existing = ratioTimersRef.current.get(key)
      if (existing !== undefined) window.clearTimeout(existing)
      const timer = window.setTimeout(() => {
        ratioTimersRef.current.delete(key)
        lastWrittenRatioRef.current.set(key, ratio)
        const generation = layoutLoadGenerationRef.current
        void herdrLayoutSetSplitRatio({
          sessionName: sessionNameArg,
          tabId: layout?.tabId ?? resolvedTabId,
          paneId: layout?.tabId || resolvedTabId ? null : paneId,
          path: splitPath,
          ratio
        })
          .then((next) => {
            if (generation === layoutLoadGenerationRef.current) setLayout(next)
          })
          .catch(() => {
            if (lastWrittenRatioRef.current.get(key) === ratio) {
              lastWrittenRatioRef.current.delete(key)
            }
          })
      }, RATIO_DEBOUNCE_MS)
      ratioTimersRef.current.set(key, timer)
    },
    [sessionCanConnect, sessionNameArg, layout?.tabId, resolvedTabId, paneId]
  )

  const sessionGuidance = sessionIsStopped
    ? t("herdrTerminal.sessionStopped", {
        name: herdrSessionId === "live" ? "default" : herdrSessionId
      })
    : sessionRunning === null
      ? t("herdrNav.connecting")
      : null

  const layoutPaneIds = useMemo(
    () => (layout ? collectPaneIds(layout.root) : []),
    [layout]
  )
  const focusedPaneId = layout?.focusedPaneId ?? layoutPaneIds[0] ?? null
  const expectedAttachmentCount = layout ? Math.max(1, layoutPaneIds.length) : 1
  const pageAttachments = Array.from(attachments.values()).filter(
    (record) => record.pagePath === pagePath
  )
  const showControllerBadge =
    sessionCanConnect &&
    pageAttachments.length >= expectedAttachmentCount &&
    pageAttachments.every(
      (record) => record.mode === "control" && record.role === "controller"
    )
  const canFocusPane = Boolean(
    sessionCanConnect &&
      targetCapabilities?.server.running &&
      targetCapabilities.api.paneFocus
  )
  const onActivatePane = useCallback(
    (nextPaneId: string) => {
      if (!canFocusPane || nextPaneId === focusedPaneId) return
      void herdrPaneFocus({
        sessionName: sessionNameArg,
        paneId: nextPaneId
      })
        .then(() => {
          setLayout((current) =>
            current ? { ...current, focusedPaneId: nextPaneId } : current
          )
        })
        .catch(() => undefined)
    },
    [canFocusPane, focusedPaneId, sessionNameArg]
  )
  const tabMenuSession = targetSessionName ?? herdrSessionId

  const headerContextMenu = contextMenuHandler({
    kind: "herdrTab",
    sessionName: tabMenuSession ?? herdrSessionId,
    tabId: layout?.tabId ?? resolvedTabId ?? "",
    workspaceId: layout?.workspaceId ?? null,
    label: title ?? null,
    pagePath
  })

  const renderNode = (
    node: HerdrLayoutNode,
    path: boolean[],
    leafActive: boolean
  ): ReactNode => {
    if (node.type === "pane") {
      const leafPaneId = node.paneId ?? paneId ?? null
      const leafTerminalId =
        (leafPaneId ? paneToTerminal.get(leafPaneId) : null) ??
        (path.length === 0 ? terminalId : null)
      if (!leafTerminalId) {
        return (
          <div
            className="flex h-full items-center justify-center text-[12px] text-(--ink-3)"
            data-testid="herdr-leaf-missing-terminal"
          >
            {t("herdrTerminal.missingTerminal")}
          </div>
        )
      }
      return (
        <HerdrTerminalLeaf
          key={`${pagePath}:${leafPaneId ?? leafTerminalId}`}
          pagePath={pagePath}
          herdrSessionId={herdrSessionId}
          sessionRunningOverride={surfaceSessionRunning}
          terminalId={leafTerminalId}
          paneId={leafPaneId}
          label={node.label ?? null}
          title={node.label ?? title}
          active={leafActive && (!focusedPaneId || leafPaneId === focusedPaneId)}
          visible={visible}
          focusedPaneId={focusedPaneId}
          tabId={layout?.tabId ?? resolvedTabId}
          workspaceId={layout?.workspaceId ?? null}
          contextSessionName={tabMenuSession}
          onActivatePane={onActivatePane}
        />
      )
    }

    const orientation = node.direction === "down" ? "vertical" : "horizontal"
    const firstPct = Math.max(5, Math.min(95, Math.round(node.ratio * 100)))
    const secondPct = 100 - firstPct
    const groupId = `herdr-split-${pathKey(path) || "root"}`
    return (
      <ResizablePanelGroup
        id={groupId}
        orientation={orientation}
        className="h-full w-full"
        data-testid={`herdr-split-${pathKey(path) || "root"}`}
        data-direction={node.direction}
        onLayoutChanged={(nextLayout, meta) => {
          if (!meta.isUserInteraction) return
          const firstId = `${groupId}-first`
          const secondId = `${groupId}-second`
          const first = nextLayout[firstId]
          const second = nextLayout[secondId]
          if (typeof first !== "number" || typeof second !== "number") return
          const total = first + second
          if (total <= 0) return
          onSplitRatioChanged(path, first / total)
        }}
      >
        <ResizablePanel id={`${groupId}-first`} defaultSize={firstPct} minSize={10}>
          {renderNode(node.first, [...path, false], leafActive)}
        </ResizablePanel>
        <ResizableHandle withHandle />
        <ResizablePanel id={`${groupId}-second`} defaultSize={secondPct} minSize={10}>
          {renderNode(node.second, [...path, true], leafActive)}
        </ResizablePanel>
      </ResizablePanelGroup>
    )
  }

  const body = sessionIsStopped || (!sessionCanConnect && !hasConnectedSession) ? (
    <HerdrTerminalLeaf
      pagePath={pagePath}
      herdrSessionId={herdrSessionId}
      sessionRunningOverride={surfaceSessionRunning}
      terminalId={terminalId}
      paneId={paneId}
      label={null}
      title={title}
      active={active}
      visible={visible}
      focusedPaneId={null}
      tabId={resolvedTabId}
      workspaceId={null}
      contextSessionName={tabMenuSession}
      forceDisconnected={sessionIsStopped}
    />
  ) : !layoutReady && !layout ? (
    <div
      className="flex h-full items-center justify-center text-[12px] text-(--ink-3)"
      data-testid="herdr-layout-loading"
    >
      {t("herdrNav.connecting")}
    </div>
  ) : layout ? (
    renderNode(layout.root, [], active)
  ) : (
    <HerdrTerminalLeaf
      pagePath={pagePath}
      herdrSessionId={herdrSessionId}
      sessionRunningOverride={surfaceSessionRunning}
      terminalId={terminalId}
      paneId={paneId}
      label={null}
      title={title}
      active={active}
      visible={visible}
      focusedPaneId={null}
      tabId={resolvedTabId}
      workspaceId={null}
      contextSessionName={tabMenuSession}
    />
  )

  return (
    <div
      className="relative flex h-full min-h-0 w-full flex-col overflow-hidden bg-(--term-bg) text-(--term-fg)"
      data-testid={`herdr-terminal-page-${terminalId}`}
      data-herdr-session={herdrSessionId}
      data-terminal-id={terminalId}
      data-tab-id={layout?.tabId ?? resolvedTabId ?? ""}
      data-visible={String(visible)}
      data-session-stopped={String(sessionIsStopped)}
      data-layout={layout ? "bsp" : "legacy"}
      data-pane-count={layout ? String(collectPaneIds(layout.root).length) : "1"}
    >
      <div
        className="flex h-[32px] shrink-0 items-center gap-[8px] border-b border-(--term-line) bg-(--term-bar) px-[10px] text-[11px] text-(--term-fg2)"
        onContextMenu={
          (layout?.tabId ?? resolvedTabId)
            ? headerContextMenu
            : undefined
        }
        data-testid="herdr-tab-header"
      >
        <span className="truncate font-medium text-(--term-fg)">
          {title ?? t("herdrTerminal.defaultTitle")}
        </span>
        {showControllerBadge && (
          <span className="rounded-[6px] bg-(--yz-active) px-[6px] py-[1px] font-mono text-[10px] uppercase tracking-[0.04em]">
            {t("herdrTerminal.controller")}
          </span>
        )}
        {sessionIsStopped && (
          <span
            data-testid="herdr-terminal-stopped"
            className="ml-auto truncate text-[11px] text-(--ink-3)"
          >
            {t("herdrTerminal.sessionStopped", {
              name: herdrSessionId === "live" ? "default" : herdrSessionId
            })}
          </span>
        )}
      </div>
      <div className="min-h-0 flex-1">{body}</div>
      {sessionGuidance && !sessionIsStopped && (
        <div
          role="status"
          className="pointer-events-none absolute bottom-2 right-2 max-w-[70%] truncate rounded-[4px] border border-(--term-line) bg-(--term-bar) px-[8px] py-[4px] text-[12px] text-(--term-fg2)"
        >
          {sessionGuidance}
        </div>
      )}
      {layoutError && sessionCanConnect && !layout && (
        <div
          role="status"
          data-testid="herdr-layout-fallback"
          className="pointer-events-none absolute bottom-2 left-2 max-w-[70%] truncate rounded-[4px] border border-(--term-line) bg-(--term-bar) px-[8px] py-[4px] text-[11px] text-(--term-fg2)"
        >
          {t("herdrTerminal.legacyLayout")}
        </div>
      )}
    </div>
  )
}

interface HerdrTerminalLeafProps {
  pagePath: string
  herdrSessionId: string
  sessionRunningOverride?: boolean | null
  terminalId: string
  paneId?: string | null
  label?: string | null
  title?: string
  active: boolean
  visible: boolean
  focusedPaneId?: string | null
  tabId?: string | null
  workspaceId?: string | null
  contextSessionName: string
  onActivatePane?: (paneId: string) => void
  forceDisconnected?: boolean
}

function HerdrTerminalLeaf({
  pagePath,
  herdrSessionId,
  sessionRunningOverride,
  terminalId,
  paneId = null,
  label = null,
  title,
  active,
  visible,
  focusedPaneId = null,
  tabId = null,
  workspaceId = null,
  contextSessionName,
  onActivatePane,
  forceDisconnected = false
}: HerdrTerminalLeafProps) {
  const { t } = useTranslation("workbench")
  const fontSize = useTerminalSettingsStore((state) => state.fontSize)
  const paneKey = paneId ?? terminalId
  const attachmentKey = herdrAttachmentKey(pagePath, paneKey)
  const sessions = useHerdrStore((s) => s.sessions)
  const runtimesBySession = useHerdrStore((s) => s.runtimesBySession)
  const inventorySessionRunning = useMemo(
    () => resolveSessionRunning(sessions, herdrSessionId),
    [sessions, herdrSessionId]
  )
  const targetSessionName = useMemo(
    () => resolveSessionName(sessions, herdrSessionId),
    [sessions, herdrSessionId]
  )
  const targetSnapshot = targetSessionName
    ? runtimesBySession[targetSessionName]?.snapshot ?? null
    : null
  const baseCwd = resolveHerdrTerminalBaseCwd({
    snapshot: targetSnapshot,
    terminalId,
    paneId,
    workspaceId
  })
  const sessionRunning = sessionRunningOverride ?? inventorySessionRunning
  const sessionCanConnect = !forceDisconnected && sessionRunning === true
  const sessionIsStopped = forceDisconnected || sessionRunning === false
  const connectorEnabled = sessionCanConnect
  const updatePaneId = useWorkspaceStore((s) => s.updateHerdrPagePaneId)
  const registerAttachment = useHerdrStore((s) => s.registerAttachment)
  const updateAttachmentPaneId = useHerdrStore((s) => s.updateAttachmentPaneId)
  const updateAttachmentMode = useHerdrStore((s) => s.updateAttachmentMode)
  const releaseAttachment = useHerdrStore((s) => s.releaseAttachment)

  const containerRef = useRef<HTMLDivElement | null>(null)
  const termRef = useRef<Terminal | null>(null)
  const fitRef = useRef<FitAddon | null>(null)
  const observerRef = useRef<ResizeObserver | null>(null)
  const themeObserverRef = useRef<MutationObserver | null>(null)
  const dataDisposableRef = useRef<{ dispose: () => void } | null>(null)
  const outputQueueRef = useRef<TerminalOutputQueue | null>(null)
  const lastOutputSeqRef = useRef<number | null>(null)
  const disposedRef = useRef(false)
  const openReadyRef = useRef(false)
  const lastSizeRef = useRef({ cols: defaultCols, rows: defaultRows })
  const activeRef = useRef(active)
  const visibleRef = useRef(visible)
  const tabIdRef = useRef(tabId)
  const previousVisibleRef = useRef(visible)
  const transportRef = useRef<ReturnType<typeof createHerdrTerminalTransport> | null>(null)
  const targetOpenRef = useRef<ReturnType<typeof installTerminalTargetOpen> | null>(null)
  const cwdRef = useRef<string | null>(baseCwd)
  const [controlMode, setControlMode] = useState<HerdrTerminalMode>(() =>
    useHerdrStore.getState().attachments.get(attachmentKey)?.mode ?? "control"
  )
  const [role, setRole] = useState<HerdrTerminalRole>(() =>
    useHerdrStore.getState().attachments.get(attachmentKey)?.role ?? "controller"
  )
  const [statusMessage, setStatusMessage] = useState<string | null>(null)
  const [takingControl, setTakingControl] = useState(false)

  const displayMode: HerdrTerminalMode = connectorEnabled ? controlMode : "observe"
  const displayRole: HerdrTerminalRole = connectorEnabled ? role : "observer"

  useLayoutEffect(() => {
    activeRef.current = active
    visibleRef.current = visible
    tabIdRef.current = tabId
    cwdRef.current = baseCwd
    outputQueueRef.current?.setVisible(visible)
    if (visible) outputQueueRef.current?.flushNow()
  }, [active, baseCwd, tabId, visible])

  useEffect(() => {
    disposedRef.current = false
    const container = containerRef.current
    if (!container) return

    const term = new Terminal({
      convertEol: true,
      cursorBlink: true,
      fontSize,
      theme: { ...buildXtermTheme(currentMode()) },
      disableStdin: false
    })
    const fitAddon = new FitAddon()
    term.loadAddon(fitAddon)
    term.open(container)
    term.attachCustomWheelEventHandler((event) => {
      const transport = transportRef.current
      if (
        !activeRef.current
        || !visibleRef.current
        || term.buffer.active.type === "alternate"
        || !transport?.canWrite()
        || !transport.scroll
      ) return true
      const rows = normalizeTerminalWheelRows(event.deltaY, event.deltaMode, terminalSize(term).rows)
      if (rows === 0) return true
      event.preventDefault()
      event.stopPropagation()
      void transport.scroll(event.deltaY < 0 ? -rows : rows).catch(() => undefined)
      return false
    })
    termRef.current = term
    fitRef.current = fitAddon
    targetOpenRef.current = installTerminalTargetOpen(term, {
      getCwd: () => cwdRef.current
    })
    const parsedDisposable = term.onWriteParsed?.(() => targetOpenRef.current?.resetHover()) ?? null
    const resetTargetHover = () => targetOpenRef.current?.resetHover()
    container.addEventListener("mouseleave", resetTargetHover)
    window.addEventListener("blur", resetTargetHover)

    const queue = new TerminalOutputQueue((data, onProcessed) => {
      if (disposedRef.current) {
        onProcessed()
        return
      }
      term.write(data, onProcessed)
    }, visibleRef.current)
    outputQueueRef.current = queue
    registerTerminalOutputQueue(attachmentKey, queue)

    // Inactive Herdr pages stay mounted with `visibility: hidden`, so their
    // container still has an authoritative size. Fit before opening the
    // connector to avoid a default 80×24 frame and first-switch reflow.
    safeFit(fitAddon)
    lastSizeRef.current = terminalSize(term)

    if (!connectorEnabled) {
      term.options.disableStdin = true
      const resizeObserver = new ResizeObserver(() => {
        if (disposedRef.current || !visibleRef.current) return
        safeFit(fitAddon)
      })
      resizeObserver.observe(container)
      observerRef.current = resizeObserver
      const themeObserver = new MutationObserver(() => {
        if (disposedRef.current) return
        term.options.theme = { ...buildXtermTheme(currentMode()) }
      })
      themeObserver.observe(document.documentElement, {
        attributes: true,
        attributeFilter: ["class"]
      })
      themeObserverRef.current = themeObserver
      return () => {
        disposedRef.current = true
        openReadyRef.current = false
        observerRef.current?.disconnect()
        themeObserverRef.current?.disconnect()
        parsedDisposable?.dispose()
        container.removeEventListener("mouseleave", resetTargetHover)
        window.removeEventListener("blur", resetTargetHover)
        targetOpenRef.current?.dispose()
        targetOpenRef.current = null
        outputQueueRef.current?.dispose()
        unregisterTerminalOutputQueue(attachmentKey)
        transportRef.current = null
        term.dispose()
        termRef.current = null
        fitRef.current = null
      }
    }

    const transport = createHerdrTerminalTransport({
      terminalId,
      paneId,
      mode: "control",
      takeover: true,
      sessionName: herdrSessionId === "live" ? null : herdrSessionId,
      onAttachment: ({ sessionId, mode, role: nextRole, takeover, target }) => {
        if (disposedRef.current) return
        registerAttachment(attachmentKey, {
          sessionId,
          pagePath,
          paneKey,
          herdrSessionId,
          terminalId,
          target,
          paneId,
          mode,
          role: nextRole,
          takeover
        })
        setControlMode(mode)
        setRole(nextRole)
        term.options.disableStdin = mode !== "control"
      },
      onPaneId: (nextPaneId) => {
        if (disposedRef.current) return
        updateAttachmentPaneId(attachmentKey, nextPaneId)
        // A tab-backed BSP page has many pane ids; keep singular page metadata
        // only for unresolved legacy pages.
        if (nextPaneId && !tabIdRef.current) updatePaneId(pagePath, nextPaneId)
      }
    })
    transportRef.current = transport

    dataDisposableRef.current = installTerminalImeHandling(
      term,
      (data) => {
        if (disposedRef.current) return
        if (!transport.canWrite()) return
        void transport.write(data).catch(() => undefined)
      },
      { anchorMode: "cursor" }
    )

    const handleEvent = (event: TerminalTransportEvent) => {
      if (disposedRef.current) return
      if (event.type === "output") {
        const previousSeq = lastOutputSeqRef.current
        lastOutputSeqRef.current = event.seq
        const missedEvents = previousSeq === null ? 0 : event.seq - previousSeq - 1
        outputQueueRef.current?.noteBackendLoss(0, Math.max(0, missedEvents))
        if (event.full) {
          lastOutputSeqRef.current = event.seq
          // Herdr full frames already bracket one authoritative screen update
          // with synchronized-output mode and clear the viewport themselves.
          // Resetting xterm before that atomic frame paints an intermediate
          // empty viewport, visible as a one-frame shake on every scroll.
          outputQueueRef.current?.replace(event.data)
          return
        }
        outputQueueRef.current?.push(event.data)
        return
      }
      if (event.type === "exit") {
        outputQueueRef.current?.push("\r\n[Herdr stream closed]\r\n")
        setStatusMessage(t("herdrTerminal.streamClosed"))
        // `exit` removes the pane from Herdr's runtime topology. Refresh both
        // snapshot identities and layout so the BSP split collapses instead of
        // retaining a dead leaf while the event subscription catches up.
        useHerdrStore.getState().bumpTopologyRevision()
        void useHerdrStore.getState().refreshSnapshot(contextSessionName).catch(() => undefined)
        return
      }
      if (event.type === "resync") {
        setStatusMessage(event.message)
        outputQueueRef.current?.push(`\r\n[Herdr resync: ${event.message}]\r\n`)
        void transport.release().then(() => {
          if (disposedRef.current || transport.isDisposed?.()) return
          return transport.open({
            cols: lastSizeRef.current.cols,
            rows: lastSizeRef.current.rows,
            onEvent: handleEvent
          })
        })
        return
      }
      if (event.type === "error") {
        setStatusMessage(event.message)
        outputQueueRef.current?.push(`\r\n[Herdr: ${event.message}]\r\n`)
        return
      }
      if (event.type === "control") {
        setControlMode(event.mode)
        setRole(event.role)
        updateAttachmentMode(attachmentKey, event.mode, event.role)
        term.options.disableStdin = event.mode !== "control"
      }
    }

    void transport
      .open({
        cols: lastSizeRef.current.cols,
        rows: lastSizeRef.current.rows,
        onEvent: handleEvent
      })
      .then(() => {
        if (disposedRef.current) return
        openReadyRef.current = true
        // ResizablePanel can report a tiny provisional width during the first
        // layout pass. Fit and publish the authoritative viewport once the
        // connector is ready and the browser has painted the BSP surface.
        window.requestAnimationFrame(() => {
          if (disposedRef.current || !visibleRef.current) return
          safeFit(fitAddon)
          const next = terminalSize(term)
          lastSizeRef.current = next
          void transport.resize(next.cols, next.rows).catch(() => undefined)
        })
      })
      .catch((error) => {
        if (disposedRef.current) return
        const message = error instanceof Error ? error.message : String(error)
        setStatusMessage(message)
        outputQueueRef.current?.push(`\r\n[Failed to open Herdr terminal: ${message}]\r\n`)
      })

    const resizeObserver = new ResizeObserver(() => {
      if (disposedRef.current || !visibleRef.current) return
      safeFit(fitAddon)
      const next = terminalSize(term)
      if (next.cols === lastSizeRef.current.cols && next.rows === lastSizeRef.current.rows) return
      lastSizeRef.current = next
      void transport.resize(next.cols, next.rows).catch(() => undefined)
    })
    resizeObserver.observe(container)
    observerRef.current = resizeObserver

    const themeObserver = new MutationObserver(() => {
      if (disposedRef.current) return
      term.options.theme = { ...buildXtermTheme(currentMode()) }
    })
    themeObserver.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["class"]
    })
    themeObserverRef.current = themeObserver

    return () => {
      disposedRef.current = true
      openReadyRef.current = false
      observerRef.current?.disconnect()
      themeObserverRef.current?.disconnect()
      dataDisposableRef.current?.dispose()
      parsedDisposable?.dispose()
      container.removeEventListener("mouseleave", resetTargetHover)
      window.removeEventListener("blur", resetTargetHover)
      targetOpenRef.current?.dispose()
      targetOpenRef.current = null
      outputQueueRef.current?.dispose()
      unregisterTerminalOutputQueue(attachmentKey)
      const transportToDispose = transport
      transportRef.current = null
      void useHerdrStore
        .getState()
        .releaseAttachment(attachmentKey)
        .then(() => transportToDispose.dispose?.() ?? transportToDispose.release())
        .catch(() => undefined)
      term.dispose()
      termRef.current = null
      fitRef.current = null
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [herdrSessionId, terminalId, pagePath, attachmentKey, connectorEnabled])

  useEffect(() => {
    if (!sessionIsStopped) return
    const transport = transportRef.current
    if (!transport) return
    openReadyRef.current = false
    if (termRef.current) termRef.current.options.disableStdin = true
    void releaseAttachment(attachmentKey)
      .then(() => transport.dispose?.() ?? transport.release())
      .catch(() => undefined)
    transportRef.current = null
  }, [sessionIsStopped, attachmentKey, releaseAttachment])

  useEffect(() => {
    const term = termRef.current
    const fitAddon = fitRef.current
    if (!term || !fitAddon || disposedRef.current) return
    if (term.options.fontSize === fontSize) return
    term.options.fontSize = fontSize
    if (!visibleRef.current) return
    safeFit(fitAddon)
  }, [fontSize])

  useLayoutEffect(() => {
    const term = termRef.current
    const fitAddon = fitRef.current
    const becameVisible = visible && !previousVisibleRef.current
    previousVisibleRef.current = visible
    if (!visible || !term || !fitAddon) return
    if (becameVisible) {
      safeFit(fitAddon)
      const next = terminalSize(term)
      if (next.cols !== lastSizeRef.current.cols || next.rows !== lastSizeRef.current.rows) {
        lastSizeRef.current = next
        void transportRef.current?.resize(next.cols, next.rows).catch(() => undefined)
      }
    }
    if (active) safeFocus(term)
  }, [active, visible])

  const onTakeControl = useCallback(async () => {
    if (takingControl || controlMode === "control" || !connectorEnabled) return
    if (transportRef.current?.isDisposed?.()) return
    setTakingControl(true)
    try {
      await transportRef.current?.takeControl?.()
      if (disposedRef.current || transportRef.current?.isDisposed?.()) return
      const nextMode = transportRef.current?.getControlMode?.() ?? "control"
      const nextRole = transportRef.current?.getRole?.() ?? "controller"
      setControlMode(nextMode)
      setRole(nextRole)
      updateAttachmentMode(attachmentKey, nextMode, nextRole)
      if (termRef.current) termRef.current.options.disableStdin = nextMode !== "control"
      setStatusMessage(null)
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      setStatusMessage(message)
    } finally {
      setTakingControl(false)
    }
  }, [attachmentKey, connectorEnabled, controlMode, takingControl, updateAttachmentMode])

  const leafContextMenu = contextMenuHandler({
    kind: "herdrPane",
    sessionName: contextSessionName,
    paneId: paneId ?? "",
    terminalId,
    tabId,
    workspaceId,
    label: label ?? title ?? null,
    pagePath,
    focusedPaneId
  })

  const badgeStyle: CSSProperties | undefined =
    focusedPaneId && paneId && focusedPaneId === paneId
      ? { boxShadow: "inset 0 0 0 1px var(--yz-accent)" }
      : undefined

  return (
    <div
      className="relative flex h-full min-h-0 w-full flex-col overflow-hidden"
      data-testid={`herdr-terminal-leaf-${terminalId}`}
      data-pane-id={paneId ?? ""}
      data-terminal-id={terminalId}
      data-control-mode={displayMode}
      data-role={displayRole}
      data-attachment-key={attachmentKey}
      style={badgeStyle}
      onContextMenu={(event) => {
        if (targetOpenRef.current?.handleContextMenu(event)) return
        if (paneId) leafContextMenu(event)
      }}
      onPointerDown={(event) => {
        if (event.button === 0 && paneId) onActivatePane?.(paneId)
      }}
    >
      {displayMode !== "control" && sessionCanConnect && (
        <div className="absolute right-2 top-2 z-10">
          <button
            type="button"
            data-testid="herdr-take-control"
            disabled={takingControl}
            onClick={() => void onTakeControl()}
            className="rounded-[7px] border border-(--line-2) bg-(--term-bar) px-[8px] py-[2px] text-[11px] font-medium text-(--ink-1) transition-colors hover:bg-(--yz-hover) disabled:opacity-50"
          >
            {t("herdrTerminal.takeControl")}
          </button>
        </div>
      )}
      <div ref={containerRef} className="min-h-0 flex-1" />
      {statusMessage && (
        <div
          role="status"
          className="pointer-events-none absolute bottom-2 right-2 max-w-[70%] truncate rounded-[4px] border border-(--term-line) bg-(--term-bar) px-[8px] py-[4px] text-[12px] text-(--term-fg2)"
        >
          {statusMessage}
        </div>
      )}
      {sessionIsStopped && !forceDisconnected && (
        <div
          className="pointer-events-none absolute inset-x-2 bottom-2 truncate rounded-[4px] border border-(--term-line) bg-(--term-bar) px-[8px] py-[4px] text-[11px] text-(--ink-3)"
        >
          {t("herdrTerminal.sessionStopped", {
            name: herdrSessionId === "live" ? "default" : herdrSessionId
          })}
        </div>
      )}
    </div>
  )
}
