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
  const [selectionFeedback, setSelectionFeedback] = useState<string | null>(null)
  const previewVisibleRef = useRef(previewVisible)
  useEffect(() => { previewVisibleRef.current = previewVisible }, [previewVisible])

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
          if (selectingRef.current && isElementContext(interaction.selection)) {
            const selectedUrl = canonicalFilePreviewUrl(remotePreviewSourceUrl(workspace, interaction.selection.url))
            if (selectedUrl === snapshot.url) {
              const source = browserTarget(selectedUrl)
              try {
                await writeText(formatElementContext(interaction.selection, source.kind === "file" ? workspacePathForDisplay(source.path) : selectedUrl))
                if (!disposed) setSelectionFeedback(t("elementCopied"))
              } catch {
                if (!disposed) setSelectionFeedback(t("elementCopyFailed"))
              }
            }
          }
          selectingRef.current = interaction.selecting === true
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
    selectingRef.current = false
    const timer = setTimeout(() => { setSelecting(false); setSelectionFeedback(null) }, 0)
    return () => clearTimeout(timer)
  }, [url, nativeSessionId])

  const setElementSelection = useCallback(async (active: boolean) => {
    if (!nativeSessionId) return
    try {
      await enqueueNativePreviewOperation(async () => {
        if (usePreviewStore.getState().nativeSession?.sessionId !== nativeSessionId || !previewVisibleRef.current) return
        await previewSelectElement(nativeSessionId, active)
        selectingRef.current = active
        setSelecting(active)
        setSelectionFeedback(null)
      })
    } catch (error) { await showActionError(t("selectElement"), error) }
  }, [nativeSessionId, t])

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
    void enqueueNativePreviewOperation(async () => {
      if (previewVisibleRef.current || !selectingRef.current
        || usePreviewStore.getState().nativeSession?.sessionId !== nativeSessionId) return
      await previewSelectElement(nativeSessionId, false)
      selectingRef.current = false
      setSelecting(false)
    }).catch(error => showActionError(t("selectElement"), error))
  }, [external, nativeSessionId, previewVisible, workspace, url, t])

  const toggleElementSelection = () => setElementSelection(!selectingRef.current)
  return { selecting, selectionFeedback, toggleElementSelection }
}
