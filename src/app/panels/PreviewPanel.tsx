import { useEffect, useRef, useState } from "react"
import { parseRemoteFilePath } from "@/lib/runtimeIdentity"
import { isTauri } from "@/lib/platform"
import {
  ArrowLeft,
  ArrowRight,
  ExternalLink,
  Monitor,
  MonitorPlay,
  RotateCw,
  Smartphone,
} from "lucide-react"
import { useTranslation } from "react-i18next"

import { EmptyState } from "@/app/workbench/EmptyState"
import { contextMenuHandler } from "@/state/contextMenuStore"
import { showActionError } from "@/lib/actionFeedback"
import i18n from "@/lib/i18n"
import {
  previewClose,
  previewOpenUrl,
  previewNavigationState,
  previewSetBounds,
  previewSetVisible,
} from "@/lib/ipc"
import { cn } from "@/lib/utils"
import { BrowserFrame } from "@/preview/BrowserFrame"
import { useRemotePreviewUrl } from "@/preview/useRemotePreviewUrl"
import {
  enqueueNativePreviewOperation,
  goBackPreview,
  goForwardPreview,
  openPreviewExternally,
  previewTargetCanGoBack,
  previewTargetCanGoForward,
  previewTargetHasUrl,
  reloadPreview,
  type PreviewCommandTarget,
} from "@/preview/previewCommands"
import { useAnyOverlayOpen } from "@/state/overlayStore"
import {
  isLocalPreviewUrl,
  type PreviewNavState,
  usePreviewStore,
} from "@/state/previewStore"
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

function normalizePreviewUrl(rawUrl: string): string {
  const schemeLessHostWithPort = /^(?:[^:/?#\s]+|\[[0-9a-f:.]+\]):\d+(?:[/?#]|$)/i
    .test(rawUrl)
  if (!schemeLessHostWithPort && /^[a-z][a-z0-9+.-]*:/i.test(rawUrl)) return rawUrl
  const scheme = /^(?:localhost|127\.0\.0\.1)(?=[:/]|$)/i.test(rawUrl) ? "http" : "https"
  return `${scheme}://${rawUrl}`
}

function reportPreviewReloadError(error: unknown): Promise<void> {
  return showActionError(i18n.t("previewPanel.reload", { ns: "panels" }), error)
}

export function PreviewPanel() {
  const { t } = useTranslation("preview")
  const { t: tp } = useTranslation("panels")
  const workspace = useWorkspaceStore((s) => s.workspacePath)
  const navMap = usePreviewStore((s) => s.nav)
  const nativeSessionId = usePreviewStore((s) => s.nativeSession?.sessionId)
  const nativeNavigationSyncs = usePreviewStore((s) => s.nativeNavigationSyncs)
  const nav = workspace ? navMap[workspace] ?? EMPTY_NAV : EMPTY_NAV
  // EditorPanel (and this panel) stays mounted but CSS-hidden when the mode is not
  // "files"; a native webview isn't affected by `display:none`, so gate its
  // visibility on the mode too — otherwise it floats over the Git/DB/SSH panels.
  const mode = useUiStore((s) => s.mode)

  const overlayOpen = useAnyOverlayOpen()
  const previewVisible = (mode === "files" || mode === "ade") && !overlayOpen
  const [urlDraft, setUrlDraft] = useState<string | null>(null)
  const [urlError, setUrlError] = useState<string | null>(null)
  const webviewHostRef = useRef<HTMLDivElement | null>(null)
  const previewVisibleRef = useRef(previewVisible)
  const consumedNativeNavigationRef = useRef<{ url: string; token: number } | null>(null)

  const external = !!nav.url && (!isLocalPreviewUrl(nav.url)
    || (isTauri() && !!workspace && !parseRemoteFilePath(workspace)))
  const renderedPreview = useRemotePreviewUrl(workspace, nav.url, nav.reloadNonce)
  const nativeNavigationSync = workspace ? nativeNavigationSyncs[workspace] ?? null : null
  const previewTarget: PreviewCommandTarget | null = workspace ? {
    workspacePath: workspace,
    url: nav.url,
  } : null
  const previewRequest = previewTarget ? { kind: "preview" as const, ...previewTarget } : null
  const previewChromeContextMenu = previewRequest ? contextMenuHandler(previewRequest) : undefined
  const canGoBack = previewTarget ? previewTargetCanGoBack(previewTarget) : false
  const canGoForward = previewTarget ? previewTargetCanGoForward(previewTarget) : false
  const canReload = previewTarget ? previewTargetHasUrl(previewTarget) : false
  const frameWidth = nav.frame === "mobile" ? 390 : "100%"

  useEffect(() => {
    setUrlDraft(null)
    setUrlError(null)
  }, [workspace])


  useEffect(() => {
    if (!isTauri() || !external || !previewVisible || !workspace || !nativeSessionId) return
    let disposed = false
    let timer: ReturnType<typeof setTimeout> | undefined
    const poll = async () => {
      try {
        await enqueueNativePreviewOperation(async () => {
          const state = usePreviewStore.getState()
          if (disposed || state.nativeRequest !== null || state.nativeSession?.sessionId !== nativeSessionId
            || useWorkspaceStore.getState().workspacePath !== workspace) return
          const generation = state.nativeRequestToken
          const snapshot = await previewNavigationState(nativeSessionId)
          const latest = usePreviewStore.getState()
          if (!disposed && latest.nativeRequestToken === generation) latest.receiveNativeNavigation(snapshot)
        })
      } catch {
        // Closing/replacing the native owner can race an in-flight snapshot.
      } finally {
        if (!disposed) timer = setTimeout(() => void poll(), 250)
      }
    }
    void poll()
    return () => { disposed = true; clearTimeout(timer) }
  }, [external, nativeSessionId, previewVisible, workspace])

  // --- native child webview ---
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
    const targetUrl = nav.url
    const nativeSession = usePreviewStore.getState().nativeSession
    const sessionId = (nativeSession?.workspacePath === workspace ? nativeSession.sessionId : null)
      ?? crypto.randomUUID()
    const requestToken = usePreviewStore.getState().beginNativeOpenRequest(workspace, targetUrl)
    let cancelled = false
    void enqueueNativePreviewOperation(async () => {
      if (!usePreviewStore.getState().nativeRequestIsCurrent(requestToken)) return
      try {
        const currentHost = webviewHostRef.current
        if (!currentHost) {
          usePreviewStore.getState().closeNativeSession()
          usePreviewStore.getState().settleNativeRequest(requestToken)
          return
        }
        const rect = currentHost.getBoundingClientRect()
        await previewOpenUrl(targetUrl, rect.left, rect.top, rect.width, rect.height, sessionId)
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
          if (!cancelled) await reportPreviewReloadError(reportedError)
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
      usePreviewStore.getState().recordNativeOpen(workspace, targetUrl, sessionId)
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
      requestNativePreviewClose(workspace, reportPreviewReloadError)
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
    if (!isTauri() || !external || !workspace || !nav.url) return
    const host = webviewHostRef.current
    if (!host) return
    const targetWorkspace = workspace
    const targetUrl = nav.url
    const update = () => {
      void enqueueNativePreviewOperation(async () => {
        if (
          useWorkspaceStore.getState().workspacePath !== targetWorkspace
          || usePreviewStore.getState().navForWorkspace(targetWorkspace).url !== targetUrl
        ) return
        const currentHost = webviewHostRef.current
        if (!currentHost) return
        const rect = currentHost.getBoundingClientRect()
        await previewSetBounds(rect.left, rect.top, rect.width, rect.height)
      })
    }
    const observer = new ResizeObserver(update)
    observer.observe(host)
    window.addEventListener("resize", update)
    return () => {
      observer.disconnect()
      window.removeEventListener("resize", update)
    }
  }, [external, nav.url, workspace])

  // Visibility gate: show the webview only when the preview is the visible
  // foreground — Files mode, no overlay open (the webview paints above every DOM
  // overlay). Recompute bounds on show so it doesn't flash at a stale position.
  // Browser preview tab is hosted on the shared ADE/Files editor surface.
  useEffect(() => {
    previewVisibleRef.current = previewVisible
  }, [previewVisible])
  useEffect(() => {
    if (!isTauri() || !external || !workspace || !nav.url) return
    const targetWorkspace = workspace
    const targetUrl = nav.url
    void enqueueNativePreviewOperation(async () => {
      if (
        useWorkspaceStore.getState().workspacePath !== targetWorkspace
        || usePreviewStore.getState().navForWorkspace(targetWorkspace).url !== targetUrl
      ) return
      const shouldShow = previewVisibleRef.current
      if (shouldShow) {
        const host = webviewHostRef.current
        if (host) {
          const rect = host.getBoundingClientRect()
          await previewSetBounds(rect.left, rect.top, rect.width, rect.height)
        }
      }
      await previewSetVisible(shouldShow)
    })
  }, [external, nav.url, previewVisible, workspace])

  const submitUrl = () => {
    if (!workspace || urlDraft === null) return
    const raw = urlDraft.trim()
    if (!raw) {
      setUrlDraft(null)
      setUrlError(null)
      return
    }
    const normalized = normalizePreviewUrl(raw)
    if (usePreviewStore.getState().navigate(workspace, normalized)) {
      setUrlDraft(null)
      setUrlError(null)
      return
    }
    setUrlError(tp("previewPanel.urlSchemeError"))
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

  const body = nav.url ? (
    <div className="flex min-h-0 flex-1 justify-center bg-(--paper-1) p-[10px]">
      <div data-testid="preview-frame-shell"
        className="flex min-h-0 max-w-full flex-1 overflow-hidden rounded-[8px] border border-(--line-1) bg-(--paper-0)"
        style={{ width: frameWidth, flex: nav.frame === "mobile" ? "0 1 auto" : "1 1 auto" }}>
        {external ? (
          <div ref={webviewHostRef} data-testid="preview-webview-host" className="min-h-0 flex-1 bg-white" />
        ) : renderedPreview.error ? (
          <p role="alert" className="p-4 text-sm text-destructive">{renderedPreview.error}</p>
        ) : <BrowserFrame url={renderedPreview.url} reloadNonce={nav.reloadNonce} />}
      </div>
    </div>
  ) : (
    <div data-testid="preview-empty-chrome" onContextMenu={previewChromeContextMenu}
      className="flex min-h-0 flex-1 items-center justify-center">
      <EmptyState icon={MonitorPlay} title={t("emptyTitle")} description={t("emptyDescription")} />
    </div>
  )

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
          aria-invalid={urlError ? true : undefined}
          aria-describedby={urlError ? "preview-url-error" : undefined}
          value={urlDraft ?? nav.url ?? ""}
          placeholder={tp("previewPanel.urlPlaceholder")}
          disabled={!workspace}
          spellCheck={false}
          autoComplete="off"
          onChange={(event) => {
            setUrlDraft(event.currentTarget.value)
            setUrlError(null)
          }}
          onFocus={(event) => event.currentTarget.select()}
          onBlur={() => {
            if (!urlError) setUrlDraft(null)
          }}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault()
              submitUrl()
            } else if (event.key === "Escape") {
              setUrlDraft(null)
              setUrlError(null)
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

      </div>

      {urlError ? (
        <div
          id="preview-url-error"
          role="alert"
          className="shrink-0 border-b border-[#f0c4c4] bg-[#fff1f1] px-[10px] py-[5px] text-[11px] text-[#b4232a]"
        >
          {urlError}
        </div>
      ) : null}


      {body}
    </div>
  )
}
