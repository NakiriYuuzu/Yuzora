import { HerdrScrollbar } from "@/terminal/HerdrScrollbar"
import { terminalFontStack } from "@/terminal/terminalFonts"
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useId,
  type CSSProperties,
  type ReactNode
} from "react"
import { FitAddon } from "@xterm/addon-fit"
import { Terminal } from "@xterm/xterm"
import { useTranslation } from "react-i18next"
import { useShallow } from "zustand/react/shallow"
import { SquareTerminal } from "lucide-react"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"

import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup
} from "@/components/ui/resizable"
import { herdrAttachmentKey, herdrPagePath } from "@/lib/herdrPages"
import { herdrScrollStrategy, supportsHerdrPaneScroll } from "@/lib/herdrCapabilities"
import { findRuntimeSession, sessionScope } from "@/lib/herdrProvider"
import {
  herdrLayoutExport,
  herdrLayoutSetSplitRatio,
  herdrPaneFocus
} from "@/lib/herdrIpc"
import type {
  HerdrLayoutDescription,
  HerdrLayoutNode,
  HerdrNamedSession,
  HerdrTerminalMode,
  HerdrTerminalRole
} from "@/lib/herdrTypes"
import { useHerdrStore } from "@/state/herdrStore"
import { useTextInputDialogStore } from "@/state/textInputDialogStore"
import { useTerminalSettingsStore } from "@/state/terminalSettingsStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { contextMenuHandler } from "@/state/contextMenuStore"
import {
  installTerminalClipboardHandling,
  type TerminalClipboardController
} from "@/terminal/terminalClipboard"
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
const LAYOUT_RETRY_DELAYS = [400, 1000]

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

function terminalModalOpen(): boolean {
  // The request exists before React mounts the dialog portal.
  return useTextInputDialogStore.getState().pending !== null
    || document.querySelector('[aria-modal="true"]:not([data-state="closed"]), dialog[open]') !== null
}

function safeFocus(term: Terminal): void {
  if (terminalModalOpen()) return
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
  sessions: HerdrNamedSession[],
  herdrSessionId: string
): string | null {
  if (herdrSessionId !== "live") return herdrSessionId
  return sessionScope(findRuntimeSession(sessions, herdrSessionId))
}

/** Resolve named session running flag; `live` maps to the default session entry. */
function resolveSessionRunning(
  sessions: HerdrNamedSession[],
  herdrSessionId: string
): boolean | null {
  if (sessions.length === 0) return null
  const match = findRuntimeSession(sessions, herdrSessionId)
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
  const topologyRevision = useHerdrStore((s) => s.topologyRevision)
  const pageAttachments = useHerdrStore(useShallow((s) =>
    Array.from(s.attachments.values()).filter((record) => record.pagePath === pagePath)
  ))
  const targetSessionName = useMemo(
    () => resolveSessionName(sessions, herdrSessionId),
    [sessions, herdrSessionId]
  )
  const { terminals, agents, resolvedTabId } = useHerdrStore(useShallow((s) => {
    const snapshot = (targetSessionName ? s.runtimesBySession[targetSessionName]?.snapshot : null)
      ?? (targetSessionName === s.selectedSessionName ? s.snapshot : null)
    // Focus updates replace the snapshot and tab flags. A mounted page only
    // needs its owning tab identity and terminal topology, not those flags.
    const knownTab = herdrTabId && (snapshot?.tabs.some((tab) => tab.id === herdrTabId)
      || snapshot?.terminals.some((term) => term.tabId === herdrTabId))
    const fromTerminal = snapshot?.terminals.find((item) =>
      item.terminalId === terminalId || (paneId && item.paneId === paneId))
    const fromAgent = snapshot?.agents.find((item) =>
      item.terminalId === terminalId || (paneId && item.paneId === paneId))
    return {
      terminals: snapshot?.terminals,
      agents: snapshot?.agents,
      resolvedTabId: knownTab ? herdrTabId : fromTerminal?.tabId ?? fromAgent?.tabId ?? herdrTabId
    }
  }))
  const targetCapabilities = useHerdrStore((s) => (targetSessionName ? s.runtimesBySession[targetSessionName]?.capabilities : null)
    ?? (targetSessionName === s.selectedSessionName ? s.capabilities : null))

  const sessionRunning = useMemo(
    () => resolveSessionRunning(sessions, herdrSessionId),
    [sessions, herdrSessionId]
  )
  const sessionCanConnect = sessionRunning === true
  const sessionIsStopped = sessionRunning === false
  const canSetSplitRatio = Boolean(
    sessionCanConnect &&
      targetCapabilities?.server.running &&
      targetCapabilities.api.layoutSetSplitRatio
  )
  const terminalConnectorCapabilitiesAllowControl = Boolean(
    targetCapabilities?.server.running &&
      targetCapabilities.terminal.control &&
      targetCapabilities.terminal.takeover &&
      targetCapabilities.terminal.input &&
      targetCapabilities.terminal.resize &&
      targetCapabilities.terminal.release
  )
  const [hasConnectedSession, setHasConnectedSession] = useState(sessionCanConnect)
  const canOpenTerminalConnector = Boolean(
    !sessionIsStopped &&
      (sessionCanConnect || hasConnectedSession) &&
      terminalConnectorCapabilitiesAllowControl
  )

  const [layout, setLayout] = useState<HerdrLayoutDescription | null>(null)
  const [layoutError, setLayoutError] = useState<string | null>(null)
  const [layoutRetrying, setLayoutRetrying] = useState(false)
  const [layoutReady, setLayoutReady] = useState(!sessionCanConnect)
  const [surfaceSessionRunning, setSurfaceSessionRunning] = useState(sessionRunning)
  const suppressRatioWriteRef = useRef(true)
  const ratioTimersRef = useRef<Map<string, number>>(new Map())
  const lastWrittenRatioRef = useRef<Map<string, number>>(new Map())
  const canSetSplitRatioRef = useRef(canSetSplitRatio)
  const layoutLoadGenerationRef = useRef(0)
  const layoutRetryTimerRef = useRef<number | null>(null)
  const layoutTargetRef = useRef<string | null>(null)
  const updateHerdrPageTabId = useWorkspaceStore((s) => s.updateHerdrPageTabId)

  const sessionNameArg = herdrSessionId === "live" ? null : herdrSessionId

  useEffect(() => {
    if (resolvedTabId) updateHerdrPageTabId(pagePath, resolvedTabId)
  }, [pagePath, resolvedTabId, updateHerdrPageTabId])

  const reloadLayout = useCallback(async function loadLayout(attempt = 0): Promise<void> {
    const generation = ++layoutLoadGenerationRef.current
    if (layoutRetryTimerRef.current !== null) window.clearTimeout(layoutRetryTimerRef.current)
    layoutRetryTimerRef.current = null
    setLayoutRetrying(true)
    const target = JSON.stringify([targetSessionName, resolvedTabId, resolvedTabId ? null : paneId])
    if (layoutTargetRef.current !== target) {
      layoutTargetRef.current = target
      setLayout(null)
      setLayoutError(null)
      setLayoutReady(false)
    }
    suppressRatioWriteRef.current = true
    for (const timer of ratioTimersRef.current.values()) {
      window.clearTimeout(timer)
    }
    ratioTimersRef.current.clear()
    lastWrittenRatioRef.current.clear()
    if (sessionIsStopped || (!sessionCanConnect && !hasConnectedSession)) {
      setLayoutRetrying(false)
      setLayout(null)
      setLayoutReady(true)
      return
    }
    if (!sessionCanConnect) {
      setLayoutRetrying(false)
      // Inventory refresh is transient, not topology teardown. Preserve the
      // last authoritative BSP tree and its mounted connector leaves.
      setLayoutReady(true)
      return
    }
    // Keep the existing terminal mounted while refreshing topology, including
    // the single-pane fallback. Replacing it with a loader releases its owner.
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
      // Allow ratio writes only after hydration when the server advertises it.
      window.setTimeout(() => {
        if (generation === layoutLoadGenerationRef.current && canSetSplitRatio) {
          suppressRatioWriteRef.current = false
        }
      }, 0)
    } catch (error) {
      if (generation !== layoutLoadGenerationRef.current) return
      // A busy WSL helper is not evidence that BSP is unsupported. Keep the
      // last authoritative tree and its live connectors while recovering.
      setLayoutError(error instanceof Error ? error.message : String(error))
      setHasConnectedSession(true)
      setSurfaceSessionRunning(true)
      if (attempt < LAYOUT_RETRY_DELAYS.length && targetCapabilities?.api.layoutExport !== false) {
        setLayoutRetrying(true)
        layoutRetryTimerRef.current = window.setTimeout(() => {
          if (generation === layoutLoadGenerationRef.current) void loadLayout(attempt + 1)
        }, LAYOUT_RETRY_DELAYS[attempt])
      }
    } finally {
      if (generation === layoutLoadGenerationRef.current && layoutRetryTimerRef.current === null) {
        setLayoutRetrying(false)
        setLayoutReady(true)
      }
    }
  }, [
    paneId,
    resolvedTabId,
    sessionCanConnect,
    sessionIsStopped,
    sessionNameArg,
    targetSessionName,
    hasConnectedSession,
    canSetSplitRatio,
    targetCapabilities?.api.layoutExport
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
      if (layoutRetryTimerRef.current !== null) window.clearTimeout(layoutRetryTimerRef.current)
      for (const timer of ratioTimers.values()) {
        window.clearTimeout(timer)
      }
      ratioTimers.clear()
    }
  }, [])

  useEffect(() => {
    canSetSplitRatioRef.current = canSetSplitRatio
    if (canSetSplitRatio) {
      suppressRatioWriteRef.current = false
      return
    }
    suppressRatioWriteRef.current = true
    for (const timer of ratioTimersRef.current.values()) window.clearTimeout(timer)
    ratioTimersRef.current.clear()
    lastWrittenRatioRef.current.clear()
  }, [canSetSplitRatio])

  const paneToTerminal = useMemo(() => {
    const map = new Map<string, string>()
    for (const term of terminals ?? []) {
      if (term.paneId) map.set(term.paneId, term.terminalId)
    }
    for (const agent of agents ?? []) {
      if (agent.paneId && agent.terminalId) map.set(agent.paneId, agent.terminalId)
    }
    return map
  }, [terminals, agents])

  const onSplitRatioChanged = useCallback(
    (splitPath: boolean[], ratio: number) => {
      if (suppressRatioWriteRef.current || !canSetSplitRatio) return
      if (!sessionCanConnect) return
      if (!(ratio >= 0 && ratio <= 1)) return
      const key = pathKey(splitPath)
      const previous = lastWrittenRatioRef.current.get(key)
      if (previous !== undefined && Math.abs(previous - ratio) < RATIO_EPSILON) return
      const existing = ratioTimersRef.current.get(key)
      if (existing !== undefined) window.clearTimeout(existing)
      const timer = window.setTimeout(() => {
        ratioTimersRef.current.delete(key)
        if (!canSetSplitRatioRef.current) return
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
    [canSetSplitRatio, sessionCanConnect, sessionNameArg, layout?.tabId, resolvedTabId, paneId]
  )

  const sessionGuidance = sessionIsStopped
    ? t("herdrTerminal.sessionStopped", {
        name: herdrSessionId === "live" ? "default" : herdrSessionId
      })
    : sessionRunning === null
      ? t("herdrNav.connecting")
      : sessionCanConnect && !canOpenTerminalConnector
        ? targetCapabilities?.terminal.reason ?? t("herdrTerminal.connectorUnavailable")
        : null

  const layoutPaneIds = useMemo(
    () => (layout ? collectPaneIds(layout.root) : []),
    [layout]
  )
  const focusedPaneId = layout?.focusedPaneId ?? layoutPaneIds[0] ?? null
  const splitResizeUnavailable = Boolean(layout && layoutPaneIds.length > 1 && !canSetSplitRatio)
  const expectedAttachmentCount = layout ? Math.max(1, layoutPaneIds.length) : 1
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

  const zoomedPaneId = layout?.zoomed && focusedPaneId && layoutPaneIds.includes(focusedPaneId)
    ? focusedPaneId
    : null
  const containsZoomedPane = (node: HerdrLayoutNode): boolean => node.type === "pane"
    ? node.paneId === zoomedPaneId
    : containsZoomedPane(node.first) || containsZoomedPane(node.second)
  const zoomPanelStyle = (node: HerdrLayoutNode): CSSProperties | undefined => !zoomedPaneId
    ? undefined
    : containsZoomedPane(node)
      ? { position: "absolute", inset: 0, width: "100%", height: "100%" }
      : { display: "none" }

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
          connectorEnabledOverride={canOpenTerminalConnector}
          terminalId={leafTerminalId}
          paneId={leafPaneId}
          label={node.label ?? null}
          title={node.label ?? title}
          active={leafActive && (!focusedPaneId || leafPaneId === focusedPaneId)}
          visible={visible && (!zoomedPaneId || leafPaneId === zoomedPaneId)}
          focusedPaneId={focusedPaneId}
          showFocusHeader={layoutPaneIds.length > 1}
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
        className="relative h-full w-full"
        data-testid={`herdr-split-${pathKey(path) || "root"}`}
        data-direction={node.direction}
        onLayoutChanged={(nextLayout, meta) => {
          if (zoomedPaneId || !canSetSplitRatio || !meta.isUserInteraction) return
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
        <ResizablePanel id={`${groupId}-first`} defaultSize={firstPct} minSize={10} style={zoomPanelStyle(node.first)}>
          {renderNode(node.first, [...path, false], leafActive)}
        </ResizablePanel>
        <ResizableHandle
          withHandle
          style={zoomedPaneId ? { display: "none" } : undefined}
          disabled={Boolean(zoomedPaneId) || !canSetSplitRatio}
          aria-disabled={!canSetSplitRatio}
          id={`herdr-split-handle-${pathKey(path) || "root"}`}
          className={!canSetSplitRatio ? "pointer-events-none cursor-default opacity-50" : undefined}
        />
        <ResizablePanel id={`${groupId}-second`} defaultSize={secondPct} minSize={10} style={zoomPanelStyle(node.second)}>
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
      connectorEnabledOverride={canOpenTerminalConnector}
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
      connectorEnabledOverride={canOpenTerminalConnector}
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
      {splitResizeUnavailable && (
        <div
          role="status"
          data-testid="herdr-split-resize-unavailable"
          className="pointer-events-none absolute bottom-2 left-2 max-w-[70%] truncate rounded-[4px] border border-(--term-line) bg-(--term-bar) px-[8px] py-[4px] text-[11px] text-(--term-fg2)"
        >
          {t("herdrTerminal.splitResizeUnavailable")}
        </div>
      )}
      {layoutError && sessionCanConnect && (layout || !layoutRetrying) && (
        <div
          role="status"
          data-testid={layout ? "herdr-layout-refresh-error" : "herdr-layout-fallback"}
          title={layoutError}
          className="absolute bottom-2 left-2 flex max-w-[70%] items-center gap-2 rounded-[4px] border border-(--term-line) bg-(--term-bar) px-[8px] py-[4px] text-[11px] text-(--term-fg2)"
        >
          <span className="truncate">{t(layout ? "herdrTerminal.layoutRefreshFailed" : "herdrTerminal.legacyLayout")}</span>
          <Button variant="ghost" size="xs" disabled={layoutRetrying} onClick={() => void reloadLayout()}>
            {t(layoutRetrying ? "herdrTerminal.layoutRetrying" : "herdrTerminal.retryLayout")}
          </Button>
        </div>
      )}
    </div>
  )
}

interface HerdrTerminalLeafProps {
  pagePath: string
  herdrSessionId: string
  sessionRunningOverride?: boolean | null
  connectorEnabledOverride?: boolean
  terminalId: string
  paneId?: string | null
  label?: string | null
  title?: string
  active: boolean
  visible: boolean
  focusedPaneId?: string | null
  showFocusHeader?: boolean
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
  connectorEnabledOverride,
  terminalId,
  paneId = null,
  label = null,
  title,
  active,
  visible,
  focusedPaneId = null,
  showFocusHeader = false,
  tabId = null,
  workspaceId = null,
  contextSessionName,
  onActivatePane,
  forceDisconnected = false
}: HerdrTerminalLeafProps) {
  const { t } = useTranslation("workbench")
  const fontSize = useTerminalSettingsStore((state) => state.fontSize)
  const fontFamily = useTerminalSettingsStore((state) => state.fontFamily)
  const paneKey = paneId ?? terminalId
  const attachmentKey = herdrAttachmentKey(pagePath, paneKey)
  const sessions = useHerdrStore((s) => s.sessions)
  const inventorySessionRunning = useMemo(
    () => resolveSessionRunning(sessions, herdrSessionId),
    [sessions, herdrSessionId]
  )
  const targetSessionName = useMemo(
    () => resolveSessionName(sessions, herdrSessionId),
    [sessions, herdrSessionId]
  )
  const baseCwd = useHerdrStore((s) => resolveHerdrTerminalBaseCwd({
    snapshot: targetSessionName ? s.runtimesBySession[targetSessionName]?.snapshot ?? null : null,
    terminalId,
    paneId,
    workspaceId
  }))
  const sessionRunning = sessionRunningOverride ?? inventorySessionRunning
  const sessionCanConnect = !forceDisconnected && sessionRunning === true
  const sessionIsStopped = forceDisconnected || sessionRunning === false
  const connectorEnabled = connectorEnabledOverride ?? sessionCanConnect
  const updatePaneId = useWorkspaceStore((s) => s.updateHerdrPagePaneId)
  const registerAttachment = useHerdrStore((s) => s.registerAttachment)
  const updateAttachmentPaneId = useHerdrStore((s) => s.updateAttachmentPaneId)
  const updateAttachmentMode = useHerdrStore((s) => s.updateAttachmentMode)
  const releaseAttachment = useHerdrStore((s) => s.releaseAttachment)

  const terminalViewportId = useId()
  const scrollbarRefreshRef = useRef<(() => void) | null>(null)
  const supportsScrollInfo = useHerdrStore((state) => {
    // A selected runtime is projected to the global capabilities field while
    // its scoped record is being reconciled. Keep the scrollbar capability
    // gate consistent with the connector gate so WSL pages do not lose their
    // scrollbar during that projection window.
    const capabilities = (targetSessionName ? state.runtimesBySession[targetSessionName]?.capabilities : null)
      ?? (targetSessionName === state.selectedSessionName ? state.capabilities : null)
    // The proxy scrollbar polls the separate pane API. Older runtimes use the
    // connector wheel command and must not mount a pane proxy that will keep
    // retrying unsupported methods.
    return supportsHerdrPaneScroll(capabilities)
  })
  const supportsScrollInfoRef = useRef(supportsScrollInfo)
  useEffect(() => {
    supportsScrollInfoRef.current = supportsScrollInfo
  }, [supportsScrollInfo])
  const containerRef = useRef<HTMLDivElement | null>(null)
  const termRef = useRef<Terminal | null>(null)
  const fitRef = useRef<FitAddon | null>(null)
  const observerRef = useRef<ResizeObserver | null>(null)
  const themeObserverRef = useRef<MutationObserver | null>(null)
  const dataDisposableRef = useRef<{ dispose: () => void } | null>(null)
  const clipboardRef = useRef<TerminalClipboardController | null>(null)
  const outputQueueRef = useRef<TerminalOutputQueue | null>(null)
  const recoverOutputRef = useRef<(() => void) | null>(null)
  const repaintAfterWriteRef = useRef(false)
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

  useEffect(() => {
    let cancelled = false
    void document.fonts?.load(`${fontSize}px ${terminalFontStack(fontFamily)}`).then(() => {
      const fit = fitRef.current
      if (!cancelled && !disposedRef.current && visibleRef.current && fit) safeFit(fit)
    }).catch(() => undefined)
    return () => { cancelled = true }
  }, [fontFamily, fontSize])

  useLayoutEffect(() => {
    activeRef.current = active
    visibleRef.current = visible
    tabIdRef.current = tabId
    cwdRef.current = baseCwd
    const queue = outputQueueRef.current
    if (visible && !previousVisibleRef.current) repaintAfterWriteRef.current = true
    if (visible && queue?.needsResync) {
      recoverOutputRef.current?.()
    } else {
      queue?.setVisible(visible)
      if (visible) queue?.flushNow()
    }
  }, [active, baseCwd, tabId, visible])

  useEffect(() => {
    disposedRef.current = false
    const container = containerRef.current
    if (!container) return

    const term = new Terminal({
      convertEol: true,
      cursorBlink: true,
      fontSize,
      fontFamily: terminalFontStack(fontFamily),
      theme: { ...buildXtermTheme(currentMode()) },
      disableStdin: false,
      scrollback: 0,
      scrollOnUserInput: false,
      smoothScrollDuration: 0
    })
    const fitAddon = new FitAddon()
    term.loadAddon(fitAddon)
    term.open(container)
    if (activeRef.current) safeFocus(term)
    clipboardRef.current = installTerminalClipboardHandling(term, {
      pasteText: (text) => { void transportRef.current?.paste(text).catch(() => undefined) },
      copyOnSelect: () => useTerminalSettingsStore.getState().copyOnSelect,
      pasteImage: (image) => {
        const transport = transportRef.current
        const sessionId = transport?.getSessionId?.()
        if (!transport || !sessionId) return
        void import("@/terminal/terminalClipboardImage").then(({ pasteTerminalClipboardImage }) =>
          pasteTerminalClipboardImage(contextSessionName,
            () => !disposedRef.current && visibleRef.current && activeRef.current
              && transportRef.current === transport && transport.canWrite() && transport.getSessionId?.() === sessionId,
            (path) => transport.paste(path), image)
        ).catch((error) => {
          if (!disposedRef.current) setStatusMessage(t("clipboardPasteFailed", { ns: "terminal", message: String(error) }))
        })
      },
      canPaste: () =>
        !disposedRef.current && visibleRef.current && activeRef.current
        && openReadyRef.current && Boolean(transportRef.current?.canWrite())
    })
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
      void transport.scroll(event.deltaY < 0 ? -rows : rows)
        .then(() => scrollbarRefreshRef.current?.())
        .catch((error) => {
          if (!disposedRef.current) setStatusMessage(error instanceof Error ? error.message : String(error))
        })
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
      term.write(data, () => {
        if (!disposedRef.current && visibleRef.current && repaintAfterWriteRef.current) {
          repaintAfterWriteRef.current = false
          term.refresh(0, term.rows - 1)
        }
        onProcessed()
      })
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
        clipboardRef.current?.dispose()
        clipboardRef.current = null
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
      // Use the resolved runtime scope for both connector and pane-scroll
      // fallback. The legacy `live` token has no addressable pane namespace.
      sessionName: contextSessionName,
      paneScrollEnabled: () => supportsScrollInfoRef.current,
      scrollEnabled: () => {
        const state = useHerdrStore.getState()
        const capabilities = (targetSessionName
          ? state.runtimesBySession[targetSessionName]?.capabilities
          : null)
          ?? (targetSessionName === state.selectedSessionName ? state.capabilities : null)
        return herdrScrollStrategy(capabilities) !== "unavailable"
      },
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

    let recovering = false
    const recoverOutput = () => {
      if (recovering || transport.isDisposed?.()) return
      recovering = true
      repaintAfterWriteRef.current = true
      openReadyRef.current = false
      queue.setVisible(false)
      transport.detachSession()
      lastOutputSeqRef.current = null
      void useHerdrStore.getState().releaseAttachment(attachmentKey).then(async () => {
        if (transport.isDisposed?.()) return
        await transport.open({ ...lastSizeRef.current, onEvent: handleEvent })
        if (transport.isDisposed?.()) return
        openReadyRef.current = true
        scrollbarRefreshRef.current?.()
        clipboardRef.current?.flushPendingPaste()
      }).catch((error) => {
        if (!transport.isDisposed?.()) setStatusMessage(String(error))
      }).finally(() => { recovering = false })
    }
    recoverOutputRef.current = recoverOutput

    dataDisposableRef.current = installTerminalImeHandling(
      term,
      (data) => {
        if (disposedRef.current || terminalModalOpen()) return
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
          queue.setVisible(visibleRef.current)
          if (visibleRef.current) queue.flushNow()
          setStatusMessage(null)
          return
        }
        outputQueueRef.current?.push(event.data)
        if (queue.needsResync && visibleRef.current) recoverOutput()
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
        recoverOutput()
        return
      }
      if (event.type === "error") {
        const message = event.message === "terminal-input-limit" || event.message === "terminal-input-failed"
          ? t("herdrTerminal.inputPaused") : event.message
        setStatusMessage(message)
        outputQueueRef.current?.push(`\r\n[Herdr: ${message}]\r\n`)
        return
      }
      if (event.type === "control") {
        setControlMode(event.mode)
        setRole(event.role)
        updateAttachmentMode(attachmentKey, event.mode, event.role)
        term.options.disableStdin = event.mode !== "control"
        if (event.mode === "control" && openReadyRef.current) {
          clipboardRef.current?.flushPendingPaste()
        }
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
        scrollbarRefreshRef.current?.()
        clipboardRef.current?.flushPendingPaste()
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
      clipboardRef.current?.dispose()
      clipboardRef.current = null
      parsedDisposable?.dispose()
      container.removeEventListener("mouseleave", resetTargetHover)
      window.removeEventListener("blur", resetTargetHover)
      targetOpenRef.current?.dispose()
      targetOpenRef.current = null
      outputQueueRef.current?.dispose()
      unregisterTerminalOutputQueue(attachmentKey)
      transport.detach()
      recoverOutputRef.current = null
      transportRef.current = null
      void useHerdrStore.getState().releaseAttachment(attachmentKey).catch(() => undefined)
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
    transport.detach()
    transportRef.current = null
    void releaseAttachment(attachmentKey).catch(() => undefined)
  }, [sessionIsStopped, attachmentKey, releaseAttachment])

  useEffect(() => {
    const term = termRef.current
    const fitAddon = fitRef.current
    if (!term || !fitAddon || disposedRef.current) return
    if (term.options.fontSize === fontSize && term.options.fontFamily === terminalFontStack(fontFamily)) return
    term.options.fontSize = fontSize
    term.options.fontFamily = terminalFontStack(fontFamily)
    if (!visibleRef.current) return
    safeFit(fitAddon)
  }, [fontSize, fontFamily])

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
      term.refresh(0, term.rows - 1)
    }
    if (active) safeFocus(term)
  }, [active, visible])

  const onTakeControl = useCallback(async () => {
    if (takingControl || controlMode === "control" || !connectorEnabled) return
    if (transportRef.current?.isDisposed?.()) return
    setTakingControl(true)
    try {
      const transport = transportRef.current
      if (!transport) return
      transport.detachSession()
      await releaseAttachment(attachmentKey)
      if (
        disposedRef.current ||
        transportRef.current !== transport ||
        transport.isDisposed?.()
      ) return
      await transport.takeControl?.()
      if (disposedRef.current || transport.isDisposed?.()) return
      const nextMode = transport.getControlMode?.() ?? "control"
      const nextRole = transport.getRole?.() ?? "controller"
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
  }, [
    attachmentKey,
    connectorEnabled,
    controlMode,
    releaseAttachment,
    takingControl,
    updateAttachmentMode
  ])

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

  const paneActive = active && visible
  const paneTitle = label || title || t("herdrTerminal.defaultTitle")
  const takeControlButton = displayMode !== "control" && sessionCanConnect ? (
    <Button
      type="button"
      variant="outline"
      size="xs"
      data-testid="herdr-take-control"
      disabled={takingControl}
      onClick={() => void onTakeControl()}
    >
      {t("herdrTerminal.takeControl")}
    </Button>
  ) : null

  return (
    <div
      className="relative flex h-full min-h-0 w-full flex-col overflow-hidden"
      data-testid={`herdr-terminal-leaf-${terminalId}`}
      data-pane-id={paneId ?? ""}
      data-terminal-id={terminalId}
      data-control-mode={displayMode}
      data-role={displayRole}
      data-attachment-key={attachmentKey}
      onContextMenu={(event) => {
        if (targetOpenRef.current?.handleContextMenu(event)) return
        if (paneId) leafContextMenu(event)
      }}
      onPointerDown={(event) => {
        if (event.button === 0 && paneId) onActivatePane?.(paneId)
      }}
    >
      {showFocusHeader && (
        <div
          data-active={paneActive}
          className="flex h-7 shrink-0 items-center gap-2 border-b border-(--term-line) bg-(--term-bar) pr-2 data-[active=true]:bg-[color-mix(in_srgb,var(--yz-accent)_18%,var(--term-bar))]"
        >
          <Button
            type="button"
            variant="ghost"
            size="xs"
            className="h-full min-w-0 flex-1 justify-start"
            aria-label={t("herdrTerminal.focusPane", { name: paneTitle })}
            aria-pressed={paneActive}
            onPointerDown={(event) => event.stopPropagation()}
            onClick={() => {
              if (paneActive && termRef.current) safeFocus(termRef.current)
              else if (paneId) onActivatePane?.(paneId)
            }}
          >
            <SquareTerminal data-icon="inline-start" aria-hidden="true" />
            <span className="min-w-0 truncate" title={paneTitle}>{paneTitle}</span>
            {paneActive && <Badge variant="secondary" className="ml-auto">{t("herdrTerminal.focused")}</Badge>}
          </Button>
          {takeControlButton}
        </div>
      )}
      {!showFocusHeader && takeControlButton && (
        <div className="absolute right-2 top-2 z-10">
          {takeControlButton}
        </div>
      )}
      <div className="flex min-h-0 flex-1">
        <div id={terminalViewportId} ref={containerRef} className="min-h-0 min-w-0 flex-1" />
        <HerdrScrollbar sessionName={contextSessionName} paneId={paneId ?? ""}
          enabled={active && visible && sessionCanConnect && !!paneId && supportsScrollInfo}
          viewportId={terminalViewportId} refreshRef={scrollbarRefreshRef}
          canScroll={() => !disposedRef.current && document.visibilityState !== "hidden" && visibleRef.current && activeRef.current && openReadyRef.current
            && termRef.current?.buffer.active.type === "normal" && !!transportRef.current?.canWrite()} />
      </div>
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
