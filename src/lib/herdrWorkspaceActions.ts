import { useHerdrStore } from '@/state/herdrStore'
import { herdrErrorKind } from './herdrErrors'
import { herdrWorkspaceMove, herdrWorkspaceMoveBlock } from './herdrIpc'
import { runtimeOwner } from './herdrProvider'
import { herdrReorderMembers, planHerdrWorkspaceReorder } from './herdrWorkspaceReorder'

const movingSessions = new Set<string>()

export function canMoveHerdrWorkspace(sessionName: string, workspaceId: string): boolean {
  const runtime = useHerdrStore.getState().runtimesBySession[sessionName]
  const caps = runtime?.capabilities
  const members = herdrReorderMembers(runtime?.snapshot?.spaces ?? [], workspaceId)
  return !movingSessions.has(sessionName) && runtime?.connectionState === 'ready' &&
    (!runtime.errorMessage || herdrErrorKind(runtime.errorMessage) === 'busy') &&
    caps?.server.compatible !== false && !!caps?.server.running && !!members &&
    (!!caps.api.workspaceMoveBlock || (members.length === 1 && !!caps.api.workspaceMove))
}

/** Shared mutation path for drag and context-menu moves; ambiguous failures only reconcile. */
export async function moveHerdrWorkspace(sessionName: string, workspaceId: string, targetId: string, after: boolean): Promise<boolean> {
  if (!canMoveHerdrWorkspace(sessionName, workspaceId) || !canMoveHerdrWorkspace(sessionName, targetId)) return false
  const state = useHerdrStore.getState()
  const runtime = state.runtimesBySession[sessionName]
  const plan = planHerdrWorkspaceReorder(runtime.snapshot?.spaces ?? [], workspaceId, targetId, after)
  if (!plan) return false
  const caps = runtime.capabilities!.api
  const owner = JSON.stringify(runtimeOwner(sessionName))
  movingSessions.add(sessionName)
  state.setWorkspaceReordering(sessionName, true)
  state.applyWorkspaceOrder(sessionName, plan.expectedOrder)
  try {
    let result
    if (caps.workspaceMoveBlock) {
      try {
        result = await herdrWorkspaceMoveBlock({ sessionName, workspaceIds: plan.sourceWorkspaceIds, beforeWorkspaceId: plan.beforeWorkspaceId })
      } catch (error) {
        if (herdrErrorKind(error) !== 'unsupported' || plan.sourceWorkspaceIds.length !== 1 || !caps.workspaceMove || owner !== JSON.stringify(runtimeOwner(sessionName))) throw error
        result = await herdrWorkspaceMove({ sessionName, workspaceId, insertIndex: plan.legacyInsertIndex })
      }
    } else result = await herdrWorkspaceMove({ sessionName, workspaceId, insertIndex: plan.legacyInsertIndex })
    if (!result || !Array.isArray(result.workspaceIds)) throw new Error('Invalid HERDR workspace order response')
    if (owner === JSON.stringify(runtimeOwner(sessionName))) useHerdrStore.getState().applyWorkspaceOrder(sessionName, result.workspaceIds)
    return true
  } finally {
    useHerdrStore.getState().setWorkspaceReordering(sessionName, false)
    try {
      await useHerdrStore.getState().refreshSnapshot(sessionName)
    } finally {
      movingSessions.delete(sessionName)
    }
  }
}
