import { writeText } from "@tauri-apps/plugin-clipboard-manager"
import { openUrl } from "@tauri-apps/plugin-opener"

import type { ContextMenuCommandOutcome } from "@/app/workbench/contextMenuModel"
import {
  devServerStop,
  previewBack as nativePreviewBack,
  previewForward as nativePreviewForward,
  previewReload as nativePreviewReload,
} from "@/lib/ipc"
import type { DevServerInfo } from "@/lib/types"
import { isLocalPreviewUrl, usePreviewStore } from "@/state/previewStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

export interface PreviewCommandTarget {
  workspacePath: string
  url: string | null
  serverAttempt: number
}

const completed = (): ContextMenuCommandOutcome => "completed"
const cancelled = (): ContextMenuCommandOutcome => "cancelled"

let nativePreviewQueue: Promise<void> = Promise.resolve()

export function enqueueNativePreviewOperation<T>(operation: () => Promise<T>): Promise<T> {
  const queued = nativePreviewQueue.then(operation, operation)
  nativePreviewQueue = queued.then(() => undefined, () => undefined)
  return queued
}

export function previewTargetIsCurrent(target: PreviewCommandTarget): boolean {
  if (useWorkspaceStore.getState().workspacePath !== target.workspacePath) return false
  return usePreviewStore.getState().navForWorkspace(target.workspacePath).url === target.url
}

export function previewTargetHasUrl(target: PreviewCommandTarget): boolean {
  return previewTargetIsCurrent(target) && target.url !== null
}

export function previewTargetCanGoBack(target: PreviewCommandTarget): boolean {
  if (!previewTargetHasUrl(target)) return false
  return usePreviewStore.getState().navForWorkspace(target.workspacePath).backStack.length > 0
}

export function previewTargetCanGoForward(target: PreviewCommandTarget): boolean {
  if (!previewTargetHasUrl(target)) return false
  return usePreviewStore.getState().navForWorkspace(target.workspacePath).forwardStack.length > 0
}

export function previewTargetHasRunningServer(target: PreviewCommandTarget): boolean {
  if (!previewTargetIsCurrent(target)) return false
  const state = usePreviewStore.getState()
  return state.attemptForWorkspace(target.workspacePath) === target.serverAttempt
    && state.devServerForWorkspace(target.workspacePath)?.status.status === "running"
}

export async function goBackPreview(
  target: PreviewCommandTarget
): Promise<ContextMenuCommandOutcome> {
  if (!previewTargetCanGoBack(target) || !target.url) return cancelled()
  if (isLocalPreviewUrl(target.url)) {
    usePreviewStore.getState().goBack(target.workspacePath)
    return completed()
  }

  return enqueueNativePreviewOperation(async () => {
    if (!previewTargetCanGoBack(target) || !target.url) return cancelled()
    const state = usePreviewStore.getState()
    const nav = state.navForWorkspace(target.workspacePath)
    const adjacentUrl = nav.backStack.at(-1)
    if (!adjacentUrl) return cancelled()
    const nativeSession = state.nativeSession
    const hasNativeContinuity = state.nativeRequest === null
      && nativeSession?.workspacePath === target.workspacePath
      && nativeSession.currentUrl === target.url
      && nativeSession.backStack.at(-1) === adjacentUrl
    if (isLocalPreviewUrl(adjacentUrl) || !hasNativeContinuity) {
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
  if (isLocalPreviewUrl(target.url)) {
    usePreviewStore.getState().goForward(target.workspacePath)
    return completed()
  }

  return enqueueNativePreviewOperation(async () => {
    if (!previewTargetCanGoForward(target) || !target.url) return cancelled()
    const state = usePreviewStore.getState()
    const nav = state.navForWorkspace(target.workspacePath)
    const adjacentUrl = nav.forwardStack[0]
    if (!adjacentUrl) return cancelled()
    const nativeSession = state.nativeSession
    const hasNativeContinuity = state.nativeRequest === null
      && nativeSession?.workspacePath === target.workspacePath
      && nativeSession.currentUrl === target.url
      && nativeSession.forwardStack[0] === adjacentUrl
    if (isLocalPreviewUrl(adjacentUrl) || !hasNativeContinuity) {
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
  if (isLocalPreviewUrl(target.url)) {
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
  await openUrl(target.url)
  return completed()
}

function sameRunningServer(left: DevServerInfo | null, right: DevServerInfo | null): boolean {
  if (left?.status.status !== "running" || right?.status.status !== "running") return false
  return left.workspace === right.workspace
    && left.command === right.command
    && left.port === right.port
    && left.status.port === right.status.port
}

export async function stopPreviewDevServer(
  target: PreviewCommandTarget
): Promise<ContextMenuCommandOutcome> {
  if (!previewTargetHasRunningServer(target)) return cancelled()

  const state = usePreviewStore.getState()
  const server = state.devServerForWorkspace(target.workspacePath)
  if (!server) return cancelled()
  const stopAttempt = state.beginAttempt(target.workspacePath)

  try {
    await devServerStop(target.workspacePath)
  } catch (error) {
    usePreviewStore.getState().restoreAttempt(
      target.workspacePath,
      stopAttempt,
      target.serverAttempt
    )
    throw error
  }

  const latest = usePreviewStore.getState()
  if (
    useWorkspaceStore.getState().workspacePath === target.workspacePath
    && latest.attemptForWorkspace(target.workspacePath) === stopAttempt
    && sameRunningServer(server, latest.devServerForWorkspace(target.workspacePath))
  ) {
    latest.setDevServer({
      workspace: target.workspacePath,
      command: server.command,
      port: server.port,
      status: { status: "exited", code: null },
    })
  }
  return completed()
}
