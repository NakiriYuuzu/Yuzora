import { useEffect, useRef, useState } from "react"
import { isTauri } from "@/lib/platform"
import {
  AlertTriangle,
  ArrowLeft,
  ArrowRight,
  ExternalLink,
  FileText,
  Monitor,
  MonitorPlay,
  Play,
  RotateCw,
  Smartphone,
  Square,
} from "lucide-react"
import { useTranslation } from "react-i18next"

import { EmptyState } from "@/app/workbench/EmptyState"
import { loadPreviewSettings } from "@/app/workbench/SettingsDialog"
import { contextMenuHandler } from "@/state/contextMenuStore"
import { showActionError } from "@/lib/actionFeedback"
import i18n from "@/lib/i18n"
import {
  devServerDetect,
  devServerStart,
  devServerStop,
  previewClose,
  previewOpenUrl,
  previewSetBounds,
  previewSetVisible,
} from "@/lib/ipc"
import type { DevServerCandidate, DevServerInfo, DevServerStatus } from "@/lib/types"
import { cn } from "@/lib/utils"
import { PreviewFrame } from "@/preview/PreviewFrame"
import {
  enqueueNativePreviewOperation,
  goBackPreview,
  goForwardPreview,
  openPreviewExternally,
  previewTargetCanGoBack,
  previewTargetCanGoForward,
  previewTargetHasUrl,
  reloadPreview,
  stopPreviewDevServer,
  type PreviewCommandTarget,
} from "@/preview/previewCommands"
import { useAnyOverlayOpen } from "@/state/overlayStore"
import { isLocalPreviewUrl, type PreviewNavState, usePreviewStore } from "@/state/previewStore"
import { useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

const EMPTY_NAV: PreviewNavState = {
  url: null,
  backStack: [],
  forwardStack: [],
  reloadNonce: 0,
  frame: "full",
}

function requestNativePreviewClose(
  workspacePath: string | null,
  onError?: (error: unknown) => Promise<void>
): void {
  const token = usePreviewStore.getState().beginNativeCloseRequest(workspacePath)
  void enqueueNativePreviewOperation(async () => {
    if (!usePreviewStore.getState().nativeRequestIsCurrent(token)) return
    try {
      await previewClose()
    } catch (error) {
      await onError?.(error)
    } finally {
      if (usePreviewStore.getState().nativeRequestIsCurrent(token)) {
        usePreviewStore.getState().closeNativeSession()
        usePreviewStore.getState().settleNativeRequest(token)
      }
    }
  })
}

type FlowState = "idle" | "detecting" | "occupied" | "no-candidates"

// Non-null statuses map to fixed short labels; the null (no-server) label is
// localized at the call site with the component's `t` (see the status badge).
function statusLabel(status: DevServerStatus): string {
  if (status.status === "starting") return i18n.t("previewPanel.status.starting", { ns: "panels" })
  if (status.status === "running") return i18n.t("previewPanel.status.running", { ns: "panels" })
  if (status.status === "exited") return i18n.t("previewPanel.status.exited", { ns: "panels" })
  return i18n.t("previewPanel.status.failed", { ns: "panels" })
}

function statusTone(status: DevServerStatus | null): string {
  if (status?.status === "running") return "border-(--line-1) bg-[#e7f8ed] text-[#17753a]"
  if (status?.status === "starting") return "border-(--line-1) bg-[#eef5ff] text-[#2666b8]"
  if (status?.status === "failed") return "border-[#f0c4c4] bg-[#fff1f1] text-[#b4232a]"
  return "border-(--line-1) bg-(--yz-sunk) text-(--ink-3)"
}

function portUrl(port: number): string {
  return `http://localhost:${port}`
}

function localhostPort(rawUrl: string | null): number | null {
  if (!rawUrl) return null
  try {
    const url = new URL(rawUrl)
    if (url.protocol !== "http:" || !["localhost", "127.0.0.1"].includes(url.hostname)) return null
    return Number.parseInt(url.port, 10)
  } catch {
    return null
  }
}

function firstCandidatePort(candidate: DevServerCandidate | null): number | null {
  return candidate?.likelyPort ?? null
}

function parsePortOverride(value: string): { port: number | null; hint: string | null } {
  const trimmed = value.trim()
  if (!trimmed) return { port: null, hint: null }
  const invalidHint = i18n.t("previewPanel.portOverrideInvalid", { ns: "panels", value: trimmed })
  if (!/^\d+$/.test(trimmed)) {
    return { port: null, hint: invalidHint }
  }

  const port = Number(trimmed)
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    return { port: null, hint: invalidHint }
  }
  return { port, hint: null }
}

export function PreviewPanel() {
  const { t } = useTranslation("preview")
  const { t: tp } = useTranslation("panels")
  const workspace = useWorkspaceStore((s) => s.workspacePath)
  const devServer = usePreviewStore((s) =>
    workspace ? s.devServerForWorkspace(workspace) : null
  )
  const navMap = usePreviewStore((s) => s.nav)
  const attempts = usePreviewStore((s) => s.attempts)
  const nativeNavigationSyncs = usePreviewStore((s) => s.nativeNavigationSyncs)
  const nav = workspace ? navMap[workspace] ?? EMPTY_NAV : EMPTY_NAV
  const navigate = usePreviewStore((s) => s.navigate)
  const openSettings = useUiStore((s) => s.openSettings)
  // EditorPanel (and this panel) stays mounted but CSS-hidden when the mode is not
  // "files"; a native webview isn't affected by `display:none`, so gate its
  // visibility on the mode too — otherwise it floats over the Git/DB/SSH panels.
  const mode = useUiStore((s) => s.mode)
  const [flowState, setFlowState] = useState<FlowState>("idle")
  const [candidates, setCandidates] = useState<DevServerCandidate[]>([])
  const [runningPorts, setRunningPorts] = useState<number[]>([])
  const [portOverride, setPortOverride] = useState("")
  const [portOverrideHint, setPortOverrideHint] = useState<string | null>(null)
  const [localError, setLocalError] = useState<string | null>(null)

  const overlayOpen = useAnyOverlayOpen()
  const [urlDraft, setUrlDraft] = useState<string | null>(null)
  const webviewHostRef = useRef<HTMLDivElement | null>(null)
  const consumedNativeNavigationRef = useRef<{ url: string; token: number } | null>(null)

  const status = devServer?.status ?? null
  const primaryCandidate = candidates[0] ?? null
  // External https renders in a native child webview; localhost/127.0.0.1 stays
  // in the iframe. Native Back/Forward is used only while previewStore can prove
  // that the current child-webview session owns the adjacent external URL.
  const external = !!nav.url && !isLocalPreviewUrl(nav.url)
  const nativeNavigationSync = workspace ? nativeNavigationSyncs[workspace] ?? null : null
  const previewTarget: PreviewCommandTarget | null = workspace ? {
    workspacePath: workspace,
    url: nav.url,
    serverAttempt: attempts[workspace] ?? 0,
  } : null
  const previewRequest = previewTarget ? { kind: "preview" as const, ...previewTarget } : null
  const previewChromeContextMenu = previewRequest ? contextMenuHandler(previewRequest) : undefined
  const canGoBack = previewTarget ? previewTargetCanGoBack(previewTarget) : false
  const canGoForward = previewTarget ? previewTargetCanGoForward(previewTarget) : false
  const canReload = previewTarget ? previewTargetHasUrl(previewTarget) : false
  const frameWidth = nav.frame === "mobile" ? 390 : "100%"

  const isAttemptLive = (attemptWorkspace: string, attempt: number) =>
    attemptWorkspace === useWorkspaceStore.getState().workspacePath
    && attempt === usePreviewStore.getState().attemptForWorkspace(attemptWorkspace)

  useEffect(() => {
    if (!workspace || !devServer || devServer.status.status !== "running") return
    const port = devServer.status.port ?? devServer.port
    if (port === null) return
    if (localhostPort(nav.url) === port) return
    navigate(workspace, portUrl(port))
  }, [devServer, navigate, nav.url, workspace])

  // --- external-URL child webview (P3) ---
  // Open/navigate the native webview to the external URL, positioned over the
  // placeholder <div>. previewOpenUrl reuses an existing webview (just navigates),
  // so re-running on url change doesn't recreate it.
  useEffect(() => {
    if (!isTauri() || !external || !nav.url || !workspace) return
    if (nativeNavigationSync?.url === nav.url) {
      consumedNativeNavigationRef.current = nativeNavigationSync
      usePreviewStore.getState().consumeNativeNavigationSync(
        workspace,
        nativeNavigationSync.token
      )
      return
    }
    const consumed = consumedNativeNavigationRef.current
    if (consumed) {
      consumedNativeNavigationRef.current = null
      if (consumed.url === nav.url) return
    }
    const host = webviewHostRef.current
    if (!host) return
    const rect = host.getBoundingClientRect()
    const targetUrl = nav.url
    const requestToken = usePreviewStore.getState().beginNativeOpenRequest(workspace, targetUrl)
    let cancelled = false
    void enqueueNativePreviewOperation(async () => {
      if (!usePreviewStore.getState().nativeRequestIsCurrent(requestToken)) return
      try {
        await previewOpenUrl(targetUrl, rect.left, rect.top, rect.width, rect.height)
      } catch (error) {
        if (usePreviewStore.getState().nativeRequestIsCurrent(requestToken)) {
          usePreviewStore.getState().closeNativeSession()
          let reportedError = error
          try {
            await previewClose()
          } catch (cleanupError) {
            reportedError = new Error(`${String(error)}; preview cleanup failed: ${String(cleanupError)}`)
          }
          usePreviewStore.getState().settleNativeRequest(requestToken)
          if (!cancelled) await showActionError(tp("previewPanel.reload"), reportedError)
        }
        return
      }
      if (!usePreviewStore.getState().nativeRequestIsCurrent(requestToken)) return
      if (
        cancelled
        || useWorkspaceStore.getState().workspacePath !== workspace
        || usePreviewStore.getState().navForWorkspace(workspace).url !== targetUrl
      ) {
        usePreviewStore.getState().closeNativeSession()
        try {
          await previewClose()
        } finally {
          usePreviewStore.getState().settleNativeRequest(requestToken)
        }
        return
      }
      usePreviewStore.getState().recordNativeOpen(workspace, targetUrl)
      usePreviewStore.getState().settleNativeRequest(requestToken)
    })
    return () => {
      cancelled = true
    }
  }, [external, nativeNavigationSync, nav.url, workspace])

  // Close the webview when the preview is no longer showing an external URL, and
  // on unmount (the panel unmounts when another tab becomes active — a stray
  // native layer would otherwise float over the editor).
  useEffect(() => {
    if (!isTauri()) return
    if (!external) {
      // The Rust child webview is a singleton. Closing it invalidates whichever
      // workspace owned the proof ledger, including an external -> other-workspace
      // local transition where `workspace` is no longer the previous owner.
      requestNativePreviewClose(
        workspace,
        (error) => showActionError(tp("previewPanel.reload"), error)
      )
    }
  }, [external, workspace])
  useEffect(() => {
    return () => {
      if (isTauri()) requestNativePreviewClose(null)
    }
  }, [])

  // Track the placeholder's bounds so the native layer stays glued to it as the
  // panel resizes (nav width, terminal drawer, responsive-frame toggle, window).
  useEffect(() => {
    if (!isTauri() || !external) return
    const host = webviewHostRef.current
    if (!host) return
    const update = () => {
      const rect = host.getBoundingClientRect()
      void previewSetBounds(rect.left, rect.top, rect.width, rect.height)
    }
    const observer = new ResizeObserver(update)
    observer.observe(host)
    window.addEventListener("resize", update)
    return () => {
      observer.disconnect()
      window.removeEventListener("resize", update)
    }
  }, [external])

  // Visibility gate: show the webview only when the preview is the visible
  // foreground — Files mode, no overlay open (the webview paints above every DOM
  // overlay). Recompute bounds on show so it doesn't flash at a stale position.
  const previewVisible = mode === "files" && !overlayOpen
  useEffect(() => {
    if (!isTauri() || !external) return
    if (previewVisible) {
      const host = webviewHostRef.current
      if (host) {
        const rect = host.getBoundingClientRect()
        void previewSetBounds(rect.left, rect.top, rect.width, rect.height)
      }
      void previewSetVisible(true)
    } else {
      void previewSetVisible(false)
    }
  }, [external, previewVisible])

  const submitUrl = () => {
    if (!workspace || urlDraft === null) return
    const raw = urlDraft.trim()
    if (!raw) {
      setUrlDraft(null)
      return
    }
    const scheme = /^(localhost|127\.0\.0\.1)(:|\/|$)/.test(raw) ? "http" : "https"
    const normalized = /^[a-z][a-z0-9+.-]*:\/\//i.test(raw) ? raw : `${scheme}://${raw}`
    if (usePreviewStore.getState().navigate(workspace, normalized)) setUrlDraft(null)
  }

  const runToolbarCommand = async (
    actionLabel: string,
    command: () => Promise<unknown>
  ) => {
    try {
      await command()
    } catch (error) {
      await showActionError(actionLabel, error)
    }
  }

  const startCandidate = async (
    candidate: DevServerCandidate,
    port: number | null,
    attempt?: number
  ) => {
    const attemptWorkspace = workspace
    if (!attemptWorkspace) return
    const attemptToken = attempt ?? usePreviewStore.getState().beginAttempt(attemptWorkspace)
    setLocalError(null)
    const startingInfo: DevServerInfo = {
      workspace: attemptWorkspace,
      command: candidate.command,
      port,
      status: { status: "starting" },
    }
    usePreviewStore.getState().setDevServer(startingInfo)
    try {
      const info = await devServerStart(attemptWorkspace, candidate.command, port, () => {})
      if (!isAttemptLive(attemptWorkspace, attemptToken)) {
        if (useWorkspaceStore.getState().workspacePath !== attemptWorkspace) {
          try {
            await devServerStop(attemptWorkspace)
          } catch (error) {
            await showActionError(tp("previewPanel.stop"), error)
          }
        }
        return
      }
      usePreviewStore.getState().setDevServer(info)
      setFlowState("idle")
      const runningPort =
        info.status.status === "running" ? info.status.port ?? info.port : info.port
      if (runningPort !== null)
        usePreviewStore.getState().navigate(attemptWorkspace, portUrl(runningPort))
    } catch (error) {
      if (!isAttemptLive(attemptWorkspace, attemptToken)) return
      const reason = error instanceof Error ? error.message : String(error)
      usePreviewStore.getState().setDevServer({
        workspace: attemptWorkspace,
        command: candidate.command,
        port,
        status: { status: "failed", reason },
      })
    }
  }

  const detectAndStart = async () => {
    const attemptWorkspace = workspace
    if (!attemptWorkspace || flowState === "detecting") return
    const attempt = usePreviewStore.getState().beginAttempt(attemptWorkspace)
    const settings = loadPreviewSettings()
    const commandOverride = settings.command.trim()
    const configuredPort = parsePortOverride(settings.port)
    setFlowState("detecting")
    setLocalError(null)
    setPortOverrideHint(configuredPort.hint)
    try {
      const extraPorts = configuredPort.port === null ? undefined : [configuredPort.port]
      const result = extraPorts
        ? await devServerDetect(attemptWorkspace, extraPorts)
        : await devServerDetect(attemptWorkspace)
      if (!isAttemptLive(attemptWorkspace, attempt)) return
      const detectedCandidate = result.candidates[0] ?? null
      const candidate =
        commandOverride.length > 0
          ? {
              scriptName: "settings",
              command: commandOverride,
              likelyPort: configuredPort.port ?? firstCandidatePort(detectedCandidate),
          }
          : detectedCandidate
      setCandidates(candidate ? [candidate, ...result.candidates.filter((c) => c.command !== candidate.command)] : [])
      const candidatePort = configuredPort.port ?? firstCandidatePort(candidate)
      const conflictingPort =
        candidatePort !== null && result.runningPorts.includes(candidatePort) ? candidatePort : null
      setRunningPorts(conflictingPort === null ? [] : [conflictingPort])
      const suggestedPort = candidatePort ?? result.runningPorts[0] ?? 5173
      setPortOverride(String(suggestedPort + (result.runningPorts.includes(suggestedPort) ? 1 : 0)))

      if (conflictingPort !== null) {
        setFlowState("occupied")
        return
      }
      if (!candidate) {
        setFlowState("no-candidates")
        return
      }
      await startCandidate(candidate, candidatePort, attempt)
      if (!isAttemptLive(attemptWorkspace, attempt)) return
    } catch (error) {
      if (!isAttemptLive(attemptWorkspace, attempt)) return
      setFlowState("idle")
      setLocalError(error instanceof Error ? error.message : String(error))
    }
  }

  const connectExisting = (port: number) => {
    if (!workspace) return
    usePreviewStore.getState().navigate(workspace, portUrl(port))
    setFlowState("idle")
  }

  const startOnOverridePort = () => {
    const nextPort = Number.parseInt(portOverride, 10)
    if (!primaryCandidate || !Number.isFinite(nextPort)) return
    void startCandidate(primaryCandidate, nextPort)
  }

  const body = (() => {
    if (nav.url && status?.status !== "exited" && status?.status !== "failed") {
      return (
        <div className="flex min-h-0 flex-1 justify-center bg-(--paper-1) p-[10px]">
          <div
            data-testid="preview-frame-shell"
            className="flex min-h-0 max-w-full flex-1 overflow-hidden rounded-[8px] border border-(--line-1) bg-(--paper-0)"
            style={{ width: frameWidth, flex: nav.frame === "mobile" ? "0 1 auto" : "1 1 auto" }}
          >
            {external ? (
              // Placeholder the native child webview is positioned over; its
              // bounds are tracked by the ResizeObserver effect above.
              <div
                ref={webviewHostRef}
                data-testid="preview-webview-host"
                className="min-h-0 flex-1 bg-white"
              />
            ) : (
              <PreviewFrame url={nav.url} reloadNonce={nav.reloadNonce} />
            )}
          </div>
        </div>
      )
    }

    if (status?.status === "failed") {
      return (
        <div
          data-testid="preview-error-chrome"
          onContextMenu={previewChromeContextMenu}
          className="flex min-h-0 flex-1 items-center justify-center p-[18px]"
        >
          <div className="flex max-w-[360px] flex-col items-center gap-[10px] text-center">
            <AlertTriangle className="size-[28px] text-[#b4232a]" aria-hidden="true" />
            <p className="text-[13px] font-medium text-(--ink-1)">{t("failedTitle")}</p>
            <p className="text-[12.5px] text-(--ink-3)">{status.reason}</p>
            <div className="flex flex-wrap items-center justify-center gap-[8px]">
              <button
                type="button"
                onClick={() => void detectAndStart()}
                className="h-[26px] rounded-[7px] bg-(--ink-1) px-[10px] text-[12px] text-(--paper-0)"
              >
                {t("retryStart")}
              </button>
              <button
                type="button"
                onClick={() => openSettings("logs", { source: "dev_server" })}
                className="flex h-[26px] items-center gap-[5px] rounded-[7px] border border-(--line-1) px-[10px] text-[12px] text-(--ink-2) hover:bg-(--paper-2)"
              >
                <FileText className="size-[12px]" aria-hidden="true" />
                {tp("previewPanel.viewLogs")}
              </button>
            </div>
          </div>
        </div>
      )
    }

    if (flowState === "occupied") {
      return (
        <div
          data-testid="preview-empty-chrome"
          onContextMenu={previewChromeContextMenu}
          className="flex min-h-0 flex-1 items-center justify-center p-[18px]"
        >
          <div className="flex max-w-[420px] flex-col items-center gap-[10px] text-center">
            <Monitor className="size-[28px] text-(--ink-3)" aria-hidden="true" />
            <p className="text-[13px] font-medium text-(--ink-1)">{t("portOccupied")}</p>
            <p className="text-[12.5px] text-(--ink-3)">
              {tp("previewPanel.portOccupiedDescription", { ports: runningPorts.join(", ") })}
            </p>
            <div className="flex flex-wrap items-center justify-center gap-[8px]">
              {runningPorts.map((port) => (
                <button
                  key={port}
                  type="button"
                  onClick={() => connectExisting(port)}
                  className="h-[26px] rounded-[7px] bg-(--ink-1) px-[10px] text-[12px] text-(--paper-0)"
                >
                  {t("connectExisting")} {port}
                </button>
              ))}
              <label className="flex items-center gap-[5px] text-[12px] text-(--ink-3)">
                {t("alternatePort")}
                <input
                  type="number"
                  aria-label={t("alternatePort")}
                  value={portOverride}
                  onChange={(event) => setPortOverride(event.currentTarget.value)}
                  className="h-[26px] w-[78px] rounded-[7px] border border-(--line-1) bg-(--yz-sunk) px-[7px] font-mono text-[12px] text-(--ink-2)"
                />
              </label>
              <button
                type="button"
                onClick={startOnOverridePort}
                className="h-[26px] rounded-[7px] border border-(--line-1) px-[10px] text-[12px] text-(--ink-2)"
              >
                {t("startChangedPort")}
              </button>
              <button
                type="button"
                onClick={() => void detectAndStart()}
                className="h-[26px] rounded-[7px] border border-(--line-1) px-[10px] text-[12px] text-(--ink-2)"
              >
                {t("retryDetect")}
              </button>
            </div>
          </div>
        </div>
      )
    }

    if (flowState === "no-candidates") {
      return (
        <div
          data-testid="preview-empty-chrome"
          onContextMenu={previewChromeContextMenu}
          className="flex min-h-0 flex-1 items-center justify-center p-[18px]"
        >
          <div className="flex max-w-[360px] flex-col items-center gap-[10px] text-center">
            <MonitorPlay className="size-[28px] text-(--ink-3)" aria-hidden="true" />
            <p className="text-[13px] font-medium text-(--ink-1)">{t("noCandidates")}</p>
            <p className="text-[12.5px] text-(--ink-3)">{t("settingsHint")}</p>
            <button
              type="button"
              onClick={() => void detectAndStart()}
              className="h-[26px] rounded-[7px] border border-(--line-1) px-[10px] text-[12px] text-(--ink-2)"
            >
              {t("retryDetect")}
            </button>
          </div>
        </div>
      )
    }

    if (status?.status === "exited") {
      return (
        <div
          data-testid="preview-empty-chrome"
          onContextMenu={previewChromeContextMenu}
          className="flex min-h-0 flex-1 items-center justify-center"
        >
          <div className="flex flex-col items-center gap-[12px]">
            <EmptyState
              icon={MonitorPlay}
              title={t("exitedTitle")}
              description={t("emptyDescription")}
            />
            <button
              type="button"
              disabled={!workspace || flowState === "detecting"}
              onClick={() => void detectAndStart()}
              className="flex h-[28px] items-center gap-[6px] rounded-[7px] bg-(--ink-1) px-[10px] text-[12px] text-(--paper-0)"
            >
              <Play className="size-[13px]" aria-hidden="true" />
              {t("start")}
            </button>
          </div>
        </div>
      )
    }

    return (
      <div
        data-testid="preview-empty-chrome"
        onContextMenu={previewChromeContextMenu}
        className="flex min-h-0 flex-1 items-center justify-center"
      >
        <div className="flex flex-col items-center gap-[12px]">
          <EmptyState
            icon={MonitorPlay}
            title={t("emptyTitle")}
            description={localError ?? t("emptyDescription")}
          />
          <button
            type="button"
            disabled={!workspace || flowState === "detecting"}
            onClick={() => void detectAndStart()}
            className={cn(
              "flex h-[28px] items-center gap-[6px] rounded-[7px] px-[10px] text-[12px]",
              workspace && flowState !== "detecting"
                ? "bg-(--ink-1) text-(--paper-0)"
                : "cursor-not-allowed bg-(--yz-sunk) text-(--ink-4)"
            )}
          >
            <Play className="size-[13px]" aria-hidden="true" />
            {flowState === "detecting" ? t("detecting") : t("start")}
          </button>
        </div>
      </div>
    )
  })()

  return (
    <div data-testid="preview-panel" className="flex min-h-0 flex-1 flex-col">
      <div
        data-testid="preview-toolbar"
        onContextMenu={previewChromeContextMenu}
        className="flex h-[38px] shrink-0 items-center gap-[6px] border-b border-(--line-1) px-[8px]"
      >
        <button
          type="button"
          disabled={!canGoBack}
          aria-label={tp("previewPanel.back")}
          onClick={() => previewTarget && void runToolbarCommand(
            tp("previewPanel.back"),
            () => goBackPreview(previewTarget)
          )}
          className={cn(
            "flex size-[24px] shrink-0 items-center justify-center rounded-[7px]",
            canGoBack ? "text-(--ink-2) hover:bg-(--paper-2)" : "cursor-not-allowed text-(--ink-4)"
          )}
        >
          <ArrowLeft className="size-[14px]" aria-hidden="true" />
        </button>
        <button
          type="button"
          disabled={!canGoForward}
          aria-label={tp("previewPanel.forward")}
          onClick={() => previewTarget && void runToolbarCommand(
            tp("previewPanel.forward"),
            () => goForwardPreview(previewTarget)
          )}
          className={cn(
            "flex size-[24px] shrink-0 items-center justify-center rounded-[7px]",
            canGoForward ? "text-(--ink-2) hover:bg-(--paper-2)" : "cursor-not-allowed text-(--ink-4)"
          )}
        >
          <ArrowRight className="size-[14px]" aria-hidden="true" />
        </button>
        <button
          type="button"
          disabled={!canReload}
          aria-label={tp("previewPanel.reload")}
          onClick={() => previewTarget && void runToolbarCommand(
            tp("previewPanel.reload"),
            () => reloadPreview(previewTarget)
          )}
          className={cn(
            "flex size-[24px] shrink-0 items-center justify-center rounded-[7px]",
            canReload ? "text-(--ink-2) hover:bg-(--paper-2)" : "cursor-not-allowed text-(--ink-4)"
          )}
        >
          <RotateCw className="size-[13px]" aria-hidden="true" />
        </button>

        <input
          aria-label={tp("previewPanel.urlLabel")}
          value={urlDraft ?? nav.url ?? ""}
          placeholder={tp("previewPanel.urlPlaceholder")}
          disabled={!workspace}
          spellCheck={false}
          autoComplete="off"
          onChange={(event) => setUrlDraft(event.currentTarget.value)}
          onFocus={(event) => event.currentTarget.select()}
          onBlur={() => setUrlDraft(null)}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault()
              submitUrl()
            } else if (event.key === "Escape") {
              setUrlDraft(null)
              event.currentTarget.blur()
            }
          }}
          className="h-[24px] min-w-0 flex-1 rounded-[7px] border border-(--line-1) bg-(--yz-sunk) px-[8px] font-mono text-[11px] text-(--ink-2)"
        />

        <button
          type="button"
          disabled={!workspace}
          aria-label={tp("previewPanel.toggleResponsiveFrame")}
          onClick={() =>
            workspace &&
            usePreviewStore.getState().setFrame(workspace, nav.frame === "full" ? "mobile" : "full")
          }
          className={cn(
            "flex size-[24px] shrink-0 items-center justify-center rounded-[7px]",
            workspace ? "text-(--ink-2) hover:bg-(--paper-2)" : "cursor-not-allowed text-(--ink-4)"
          )}
        >
          {nav.frame === "mobile" ? (
            <Monitor className="size-[13px]" aria-hidden="true" />
          ) : (
            <Smartphone className="size-[13px]" aria-hidden="true" />
          )}
        </button>

        {nav.url && (
          <button
            type="button"
            aria-label={tp("previewPanel.openExternally")}
            title={tp("previewPanel.openExternally")}
            onClick={() => previewTarget && void runToolbarCommand(
              tp("previewPanel.openExternally"),
              () => openPreviewExternally(previewTarget)
            )}
            className="flex size-[24px] shrink-0 items-center justify-center rounded-[7px] text-(--ink-2) hover:bg-(--paper-2)"
          >
            <ExternalLink className="size-[13px]" aria-hidden="true" />
          </button>
        )}

        {status?.status === "running" && (
          <button
            type="button"
            onClick={() => previewTarget && void runToolbarCommand(
              tp("previewPanel.stop"),
              () => stopPreviewDevServer(previewTarget)
            )}
            className="flex h-[24px] shrink-0 items-center gap-[5px] rounded-[7px] border border-(--line-1) px-[8px] text-[11px] text-(--ink-2) hover:bg-(--paper-2)"
          >
            <Square className="size-[11px]" aria-hidden="true" />
            {tp("previewPanel.stop")}
          </button>
        )}

        <span
          className={cn(
            "shrink-0 rounded-[6px] border px-[7px] py-[3px] text-[10.5px]",
            statusTone(status)
          )}
        >
          {status ? statusLabel(status) : t("noDevServer")}
        </span>
      </div>

      {portOverrideHint ? (
        <div
          role="status"
          className="shrink-0 border-b border-[#ecd7a8] bg-[#fff7df] px-[10px] py-[5px] text-[11px] text-[#7a4b00]"
        >
          {portOverrideHint}
        </div>
      ) : null}

      {body}
    </div>
  )
}
