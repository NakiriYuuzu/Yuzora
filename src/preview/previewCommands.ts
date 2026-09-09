import { writeText } from "@tauri-apps/plugin-clipboard-manager"
import { openUrl } from "@tauri-apps/plugin-opener"

import type { ContextMenuCommandOutcome } from "@/app/workbench/contextMenuModel"
import {
  previewBack as nativePreviewBack,
  previewForward as nativePreviewForward,
  previewReload as nativePreviewReload,
} from "@/lib/ipc"
import { enqueueNativePreviewOperation } from "@/preview/nativePreviewQueue"
import { isLocalPreviewUrl, usePreviewStore } from "@/state/previewStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

export { enqueueNativePreviewOperation } from "@/preview/nativePreviewQueue"

export interface PreviewCommandTarget {
  workspacePath: string
  url: string | null
}

const completed = (): ContextMenuCommandOutcome => "completed"
const cancelled = (): ContextMenuCommandOutcome => "cancelled"

export function previewTargetIsCurrent(target: PreviewCommandTarget): boolean {
  if (useWorkspaceStore.getState().workspacePath !== target.workspacePath) return false
  return usePreviewStore.getState().navForWorkspace(target.workspacePath).url === target.url
}

export function previewTargetHasUrl(target: PreviewCommandTarget): boolean {
  return previewTargetIsCurrent(target) && target.url !== null
}

export function previewTargetCanGoBack(target: PreviewCommandTarget): boolean {
  if (!previewTargetHasUrl(target)) return false
  const state = usePreviewStore.getState()
  if (state.nativeSession?.workspacePath === target.workspacePath && state.nativeSession.canGoBack !== undefined) {
    return state.nativeRequest === null && state.nativeSession.currentUrl === target.url
      && (state.nativeSession.canGoBack || (state.nativeSession.outerBackStack?.length ?? 0) > 0)
  }
  return state.navForWorkspace(target.workspacePath).backStack.length > 0
}

export function previewTargetCanGoForward(target: PreviewCommandTarget): boolean {
  if (!previewTargetHasUrl(target)) return false
  const state = usePreviewStore.getState()
  if (state.nativeSession?.workspacePath === target.workspacePath && state.nativeSession.canGoForward !== undefined) {
    return state.nativeRequest === null && state.nativeSession.currentUrl === target.url
      && (state.nativeSession.canGoForward || (state.nativeSession.outerForwardStack?.length ?? 0) > 0)
  }
  return state.navForWorkspace(target.workspacePath).forwardStack.length > 0
}

function traverseOuterHistory(workspace: string, direction: "back" | "forward"): ContextMenuCommandOutcome {
  const state = usePreviewStore.getState()
  const session = state.nativeSession
  const nav = state.navForWorkspace(workspace)
  if (!session || session.workspacePath !== workspace) return cancelled()
  const backStack = session.outerBackStack ?? []
  const forwardStack = session.outerForwardStack ?? []
  if ((direction === "back" ? backStack : forwardStack).length === 0) return cancelled()
  // A prior surface belongs to a different native history. Reopen under a new
  // owner instead of appending a synthetic Back to the current browser history.
  state.closeNativeSession(workspace)
  usePreviewStore.setState({ nav: { ...state.nav, [workspace]: { ...nav, backStack, forwardStack } } })
  if (direction === "back") state.goBack(workspace)
  else state.goForward(workspace)
  return completed()
}

export async function goBackPreview(
  target: PreviewCommandTarget
): Promise<ContextMenuCommandOutcome> {
  if (!previewTargetCanGoBack(target) || !target.url) return cancelled()
  if (isLocalPreviewUrl(target.url)
    && usePreviewStore.getState().nativeSession?.workspacePath !== target.workspacePath) {
    usePreviewStore.getState().goBack(target.workspacePath)
    return completed()
  }

  return enqueueNativePreviewOperation(async () => {
    if (!previewTargetCanGoBack(target) || !target.url) return cancelled()
    const state = usePreviewStore.getState()
    if (state.nativeSession?.workspacePath === target.workspacePath
      && state.nativeSession.canGoBack !== undefined) {
      if (state.nativeRequest !== null || state.nativeSession.currentUrl !== target.url) return cancelled()
      if (!state.nativeSession.canGoBack) return traverseOuterHistory(target.workspacePath, "back")
      await nativePreviewBack(state.nativeSession.sessionId)
      // The next native snapshot acknowledges the actual URL and capabilities.
      return completed()
    }
    const nav = state.navForWorkspace(target.workspacePath)
    const adjacentUrl = nav.backStack.at(-1)
    if (!adjacentUrl) return cancelled()
    const nativeSession = state.nativeSession
    const hasNativeContinuity = state.nativeRequest === null
      && nativeSession?.workspacePath === target.workspacePath
      && nativeSession.currentUrl === target.url
      && nativeSession.backStack.at(-1) === adjacentUrl
    if (!hasNativeContinuity) {
      state.goBack(target.workspacePath)
      return completed()
    }

    const requestGeneration = state.nativeRequestToken
    await nativePreviewBack()
    const latest = usePreviewStore.getState()
    const latestNav = latest.navForWorkspace(target.workspacePath)
    if (
      !previewTargetIsCurrent(target)
      || latest.nativeRequest !== null
      || latest.nativeRequestToken !== requestGeneration
      || latestNav.backStack.at(-1) !== adjacentUrl
    ) {
      latest.closeNativeSession(target.workspacePath)
      return cancelled()
    }
    if (!latest.syncNativeBack(target.workspacePath)) {
      latest.closeNativeSession(target.workspacePath)
      return cancelled()
    }
    return completed()
  })
}

export async function goForwardPreview(
  target: PreviewCommandTarget
): Promise<ContextMenuCommandOutcome> {
  if (!previewTargetCanGoForward(target) || !target.url) return cancelled()
  if (isLocalPreviewUrl(target.url)
    && usePreviewStore.getState().nativeSession?.workspacePath !== target.workspacePath) {
    usePreviewStore.getState().goForward(target.workspacePath)
    return completed()
  }

  return enqueueNativePreviewOperation(async () => {
    if (!previewTargetCanGoForward(target) || !target.url) return cancelled()
    const state = usePreviewStore.getState()
    if (state.nativeSession?.workspacePath === target.workspacePath
      && state.nativeSession.canGoForward !== undefined) {
      if (state.nativeRequest !== null || state.nativeSession.currentUrl !== target.url) return cancelled()
      if (!state.nativeSession.canGoForward) return traverseOuterHistory(target.workspacePath, "forward")
      await nativePreviewForward(state.nativeSession.sessionId)
      // The next native snapshot acknowledges the actual URL and capabilities.
      return completed()
    }
    const nav = state.navForWorkspace(target.workspacePath)
    const adjacentUrl = nav.forwardStack[0]
    if (!adjacentUrl) return cancelled()
    const nativeSession = state.nativeSession
    const hasNativeContinuity = state.nativeRequest === null
      && nativeSession?.workspacePath === target.workspacePath
      && nativeSession.currentUrl === target.url
      && nativeSession.forwardStack[0] === adjacentUrl
    if (!hasNativeContinuity) {
      state.goForward(target.workspacePath)
      return completed()
    }

    const requestGeneration = state.nativeRequestToken
    await nativePreviewForward()
    const latest = usePreviewStore.getState()
    const latestNav = latest.navForWorkspace(target.workspacePath)
    if (
      !previewTargetIsCurrent(target)
      || latest.nativeRequest !== null
      || latest.nativeRequestToken !== requestGeneration
      || latestNav.forwardStack[0] !== adjacentUrl
    ) {
      latest.closeNativeSession(target.workspacePath)
      return cancelled()
    }
    if (!latest.syncNativeForward(target.workspacePath)) {
      latest.closeNativeSession(target.workspacePath)
      return cancelled()
    }
    return completed()
  })
}

export async function reloadPreview(
  target: PreviewCommandTarget
): Promise<ContextMenuCommandOutcome> {
  if (!previewTargetHasUrl(target) || !target.url) return cancelled()
  if (isLocalPreviewUrl(target.url)
    && usePreviewStore.getState().nativeSession?.workspacePath !== target.workspacePath) {
    usePreviewStore.getState().reload(target.workspacePath)
    return completed()
  }
  return enqueueNativePreviewOperation(async () => {
    if (!previewTargetHasUrl(target) || !target.url) return cancelled()
    const state = usePreviewStore.getState()
    const nativeSession = state.nativeSession
    if (
      state.nativeRequest !== null
      || nativeSession?.workspacePath !== target.workspacePath
      || nativeSession.currentUrl !== target.url
    ) return cancelled()

    const requestGeneration = state.nativeRequestToken
    await nativePreviewReload()
    const latest = usePreviewStore.getState()
    if (
      !previewTargetIsCurrent(target)
      || latest.nativeRequest !== null
      || latest.nativeRequestToken !== requestGeneration
      || latest.nativeSession?.workspacePath !== target.workspacePath
      || latest.nativeSession.currentUrl !== target.url
    ) {
      latest.closeNativeSession(target.workspacePath)
      return cancelled()
    }
    return completed()
  })
}

export async function copyPreviewUrl(
  target: PreviewCommandTarget
): Promise<ContextMenuCommandOutcome> {
  if (!previewTargetHasUrl(target) || !target.url) return cancelled()
  await writeText(target.url)
  return completed()
}

export async function openPreviewExternally(
  target: PreviewCommandTarget
): Promise<ContextMenuCommandOutcome> {
  if (!previewTargetHasUrl(target) || !target.url) return cancelled()
  const url = (await import("./remotePreviewUrl")).remotePreviewDisplayUrl(target.workspacePath, target.url)
  if (!previewTargetHasUrl(target)) return cancelled()
  await openUrl(url)
  return completed()
}
