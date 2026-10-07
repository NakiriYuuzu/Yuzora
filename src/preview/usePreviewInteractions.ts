import { useCallback, useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { writeText } from "@tauri-apps/plugin-clipboard-manager"
import { showActionError } from "@/lib/actionFeedback"
import { previewInteractions, previewNavigationState, previewSelectElement } from "@/lib/ipc"
import { workspacePathForDisplay } from "@/lib/paths"
import { isTauri } from "@/lib/platform"
import { navigateWorkbenchTabs } from "@/lib/workbenchTabNavigation"
import { tabShortcutBindings } from "@/state/keyboardSettingsStore"
import { usePreviewStore } from "@/state/previewStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { formatElementContext, isElementContext } from "./elementContext"
import { assertFilePreviewCurrent, browserTarget, canonicalFilePreviewUrl } from "./filePreview"
import { enqueueNativePreviewOperation } from "./nativePreviewQueue"
import { remotePreviewSourceUrl } from "./remotePreviewUrl"

interface PreviewInteractionTarget {
  workspace: string | null
  url: string | null
  nativeSessionId: string | undefined
  external: boolean
  previewVisible: boolean
}

/** Own native interaction polling, selection and clipboard for one Browser owner. */
export function usePreviewInteractions({ workspace, url, nativeSessionId, external, previewVisible }: PreviewInteractionTarget) {
  const { t } = useTranslation("preview")
  const [selecting, setSelecting] = useState(false)
  const selectingRef = useRef(false)
  const grantRef = useRef<{ generation: number; navigation: number; url: string } | null>(null)
  const selectionEpoch = useRef(0)
  const [selectionFeedback, setSelectionFeedback] = useState<string | null>(null)
  const previewVisibleRef = useRef(previewVisible)
  useEffect(() => { previewVisibleRef.current = previewVisible }, [previewVisible])

  useEffect(() => {
    const revokeStale = () => {
      const state = usePreviewStore.getState()
      const grant = grantRef.current
      if (state.nativeSession?.sessionId !== nativeSessionId
        || useWorkspaceStore.getState().workspacePath !== workspace
        || (grant && (state.nativeRequestToken !== grant.generation
          || state.nativeNavigationSyncToken !== grant.navigation
          || state.nativeSession?.currentUrl !== grant.url
          || (workspace && state.nav[workspace]?.url !== grant.url)))) {
        selectionEpoch.current++
        grantRef.current = null
        selectingRef.current = false
        setSelecting(false)
      }
    }
    const preview = usePreviewStore.subscribe(revokeStale)
    const workspaces = useWorkspaceStore.subscribe(revokeStale)
    return () => { preview(); workspaces(); selectionEpoch.current++; grantRef.current = null; selectingRef.current = false }
  }, [nativeSessionId, workspace])

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
          snapshot.url = canonicalFilePreviewUrl(remotePreviewSourceUrl(workspace, snapshot.url))
          assertFilePreviewCurrent(snapshot.url)
          const latest = usePreviewStore.getState()
          // A close/replacement queued during navigation must not wait for
          // another page evaluation whose result already has no current owner.
          if (latest.nativeRequest?.kind === "close" || latest.nativeSession?.sessionId !== nativeSessionId
            || useWorkspaceStore.getState().workspacePath !== workspace) return
          if (!disposed && latest.nativeRequestToken === generation) latest.receiveNativeNavigation(snapshot)
          const interaction = await previewInteractions(nativeSessionId, tabShortcutBindings())
          if (disposed || usePreviewStore.getState().nativeSession?.sessionId !== nativeSessionId
            || usePreviewStore.getState().nativeRequestToken !== generation
            || useWorkspaceStore.getState().workspacePath !== workspace || !interaction) return
          assertFilePreviewCurrent(snapshot.url)
          for (const command of (Array.isArray(interaction.commands) ? interaction.commands : []).slice(0, 8)) {
            if (command === "nextTab" || command === "previousTab") void navigateWorkbenchTabs({ direction: command === "nextTab" ? 1 : -1 })
            else if (/^tab[1-9]$/.test(command)) void navigateWorkbenchTabs({ index: Number(command.slice(3)) - 1 })
          }
          const grant = grantRef.current
          if (selectingRef.current && previewVisibleRef.current && grant
            && grant.generation === generation && grant.url === snapshot.url
            && grant.navigation === usePreviewStore.getState().nativeNavigationSyncToken
            && isElementContext(interaction.selection)) {
            const selectedUrl = canonicalFilePreviewUrl(remotePreviewSourceUrl(workspace, interaction.selection.url))
            if (selectedUrl === snapshot.url) {
              // One trusted toolbar action authorizes at most one write, including
              // failed writes. Page-world selecting state can never create a grant.
              grantRef.current = null
              selectingRef.current = false
              const source = browserTarget(selectedUrl)
              try {
                await writeText(formatElementContext(interaction.selection, source.kind === "file" ? workspacePathForDisplay(source.path) : selectedUrl))
                if (!disposed) setSelectionFeedback(t("elementCopied"))
              } catch {
                if (!disposed) setSelectionFeedback(t("elementCopyFailed"))
              }
            }
          }
          if (interaction.selecting !== true) {
            grantRef.current = null
            selectingRef.current = false
          }
          setSelecting(selectingRef.current)
        })
      } catch {
        // Closing/replacing the native owner can race an in-flight snapshot.
      } finally {
        if (!disposed) timer = setTimeout(() => void poll(), 100)
      }
    }
    void poll()
    return () => { disposed = true; clearTimeout(timer) }
  }, [external, nativeSessionId, previewVisible, workspace, t])

  useEffect(() => {
    selectionEpoch.current++
    grantRef.current = null
    selectingRef.current = false
    const timer = setTimeout(() => { setSelecting(false); setSelectionFeedback(null) }, 0)
    return () => clearTimeout(timer)
  }, [url, nativeSessionId, workspace])

  const setElementSelection = useCallback(async (active: boolean) => {
    if (!nativeSessionId) return
    const epoch = ++selectionEpoch.current
    if (!active) {
      grantRef.current = null
      selectingRef.current = false
      setSelecting(false)
    }
    try {
      await enqueueNativePreviewOperation(async () => {
        const state = usePreviewStore.getState()
        if (epoch !== selectionEpoch.current || state.nativeSession?.sessionId !== nativeSessionId
          || !previewVisibleRef.current || useWorkspaceStore.getState().workspacePath !== workspace) return
        const generation = state.nativeRequestToken
        const navigation = state.nativeNavigationSyncToken
        const currentUrl = state.nativeSession.currentUrl
        if (active) grantRef.current = { generation, navigation, url: currentUrl }
        await previewSelectElement(nativeSessionId, active)
        const latest = usePreviewStore.getState()
        if (epoch !== selectionEpoch.current || !previewVisibleRef.current
          || latest.nativeSession?.sessionId !== nativeSessionId
          || latest.nativeRequestToken !== generation || latest.nativeNavigationSyncToken !== navigation
          || latest.nativeSession.currentUrl !== currentUrl
          || (workspace && latest.nav[workspace]?.url !== currentUrl)
          || useWorkspaceStore.getState().workspacePath !== workspace) return
        grantRef.current = active ? { generation, navigation, url: currentUrl } : null
        selectingRef.current = active
        setSelecting(active)
        setSelectionFeedback(null)
      })
    } catch (error) {
      if (epoch === selectionEpoch.current) { grantRef.current = null; selectingRef.current = false; setSelecting(false) }
      await showActionError(t("selectElement"), error)
    }
  }, [nativeSessionId, workspace, t])

  useEffect(() => {
    const cancel = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.isComposing || !selectingRef.current) return
      event.preventDefault()
      event.stopPropagation()
      void setElementSelection(false)
    }
    // Clicking the toolbar leaves keyboard focus in the main webview. The
    // injected page listener handles Escape after focus moves into the child.
    document.addEventListener("keydown", cancel, true)
    return () => document.removeEventListener("keydown", cancel, true)
  }, [setElementSelection])

  // This queues before the panel hides the child webview on an overlay/mode change.
  useEffect(() => {
    if (!isTauri() || !external || previewVisible || !workspace || !url || !nativeSessionId) return
    selectionEpoch.current++
    grantRef.current = null
    selectingRef.current = false
    void enqueueNativePreviewOperation(async () => {
      if (previewVisibleRef.current
        || usePreviewStore.getState().nativeSession?.sessionId !== nativeSessionId) return
      await previewSelectElement(nativeSessionId, false)
      selectingRef.current = false
      setSelecting(false)
    }).catch(error => showActionError(t("selectElement"), error))
  }, [external, nativeSessionId, previewVisible, workspace, url, t])

  const toggleElementSelection = () => setElementSelection(!selectingRef.current)
  return { selecting, selectionFeedback, toggleElementSelection }
}
