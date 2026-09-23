import type { HerdrFeatureRequest, HerdrFeatureResult } from "./herdrFeatures"
import { herdrPageMatchesSnapshotSession } from "./workbenchTabReorder"
import { localDefaultScope } from "./herdrProvider"
import { useHerdrStore } from "@/state/herdrStore"
import { useWorkspaceStore, type TabInfo } from "@/state/workspaceStore"

/** Unmount herdr-terminal pages of `scope` selected by `match`, releasing their streams and layout retries. */
export function closeHerdrTerminalPages(scope: string, match: (tab: TabInfo) => boolean) {
  const defaultScope = localDefaultScope(useHerdrStore.getState().sessions)
  const paths = useWorkspaceStore.getState().groups.flatMap(group => group.tabs)
    .filter(tab => tab.kind === "herdr-terminal" && herdrPageMatchesSnapshotSession(tab.herdrSessionId, scope, defaultScope) && match(tab))
    .map(tab => tab.path)
  if (paths.length) useWorkspaceStore.getState().closeTabsByPath(paths, defaultScope ?? undefined)
}

interface MoveResult { changed?: boolean; closed_tab_id?: string; pane?: { pane_id?: string; tab_id?: string; workspace_id?: string } }

export function closeRemovedHerdrPages(scope: string, request: HerdrFeatureRequest, value: HerdrFeatureResult) {
  const move = value.move_result as MoveResult | undefined
  const workspaceId = request.method === "worktree.remove" ? request.params.workspace_id : null
  const sessionRemoved = request.method === "session.stop" || request.method === "session.delete"
  const closedTab = request.method === "pane.move" ? move?.closed_tab_id : null
  if (!workspaceId && !sessionRemoved && !closedTab) return
  const workspaceTabs = new Set(useHerdrStore.getState().runtimesBySession[scope]?.snapshot?.tabs.filter(tab => tab.workspaceId === workspaceId).map(tab => tab.id))
  closeHerdrTerminalPages(scope, tab => Boolean(sessionRemoved || (closedTab && tab.herdrTabId === closedTab)
    || (workspaceId && (tab.herdrWorkspaceId === workspaceId || (tab.herdrTabId && workspaceTabs.has(tab.herdrTabId))))))
}

export async function activateHerdrFeatureResult(scope: string, request: HerdrFeatureRequest, value: HerdrFeatureResult) {
  let tabId: string | undefined
  if (request.method === "pane.move") tabId = (value.move_result as MoveResult | undefined)?.pane?.tab_id
  else if (request.method === "worktree.create" || request.method === "worktree.open") tabId = (value.tab as { tab_id?: string } | undefined)?.tab_id
  if (!tabId) return
  const store = useHerdrStore.getState()
  const tab = store.runtimesBySession[scope]?.snapshot?.tabs.find(item => item.id === tabId)
  if (!tab) throw new Error("The operation completed; the destination tab has not appeared in the snapshot yet")
  const result = await store.activateTab({ ...tab, sessionName: scope })
  if (!result.ok && result.error) throw new Error(result.error)
}
