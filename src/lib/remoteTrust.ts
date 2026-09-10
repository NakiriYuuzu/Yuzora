import { requestHost, type HostOperation } from "./hostIpc"
import { connectedWorkspaceOwners, runtimeWorkspaceService } from "./remoteFiles"
import { parseRemoteFilePath, remoteFilePath, sameConnection } from "./runtimeIdentity"
import type { WorkspaceTrustStatus, TrustedWorkspace } from "./types"

const challenges = new Map<string, { raw: string; service: ReturnType<typeof runtimeWorkspaceService> }>()
function project<T extends { canonicalPath?: string; challengeId?: string }>(value: T, service: ReturnType<typeof runtimeWorkspaceService>): T {
  const result = { ...value }
  if (result.canonicalPath) result.canonicalPath = remoteFilePath(service.owner.hostId, result.canonicalPath, service.root)
  if (result.challengeId) {
    const raw = result.challengeId
    result.challengeId = JSON.stringify([service.owner.hostId, service.owner.generation, raw])
    // Challenges expire on the host after 60s; bound the renderer registry too.
    if (challenges.size >= 128) challenges.delete(challenges.keys().next().value!)
    challenges.set(result.challengeId, { raw, service })
  }
  return result
}


export async function requestWorkspace<T>(service: ReturnType<typeof runtimeWorkspaceService>, operation: HostOperation): Promise<T> {
  service.assertCurrent()
  try {
    const result = await requestHost<T>(service.owner, operation)
    service.assertCurrent()
    return result
  } catch (error) {
    service.assertCurrent()
    let dto: { error?: string; canonicalPath?: string; challengeId?: string } | undefined
    try { dto = JSON.parse(error instanceof Error ? error.message : String(error)) } catch { /* ordinary transport error */ }
    if (dto && typeof dto.error === "string") throw JSON.stringify(project(dto, service))
    throw error
  }
}

export async function remoteTrustStatus(path: string): Promise<WorkspaceTrustStatus> {
  const service = runtimeWorkspaceService(path)
  const value = await requestWorkspace<WorkspaceTrustStatus>(service, { method: "trust", params: { call: { action: "status", workspace: service.capabilityId } } })
  service.assertCurrent()
  return project(value, service)
}

export async function remoteTrustGrant(id: string): Promise<WorkspaceTrustStatus> {
  const challenge = challenges.get(id)
  if (!challenge) throw new Error("Remote workspace trust challenge expired")
  challenges.delete(id)
  challenge.service.assertCurrent()
  const value = await requestWorkspace<WorkspaceTrustStatus>(challenge.service, { method: "trust", params: { call: { action: "grant", challenge: challenge.raw } } })
  challenge.service.assertCurrent()
  return project(value, challenge.service)
}

export async function remoteTrustList(): Promise<TrustedWorkspace[]> {
  const results = await Promise.allSettled(connectedWorkspaceOwners().map(async (owner) => {
    const rows = await requestHost<TrustedWorkspace[]>(owner, { method: "trust", params: { call: { action: "list" } } })
    if (!connectedWorkspaceOwners().some((current) => sameConnection(owner, current))) return []
    return rows.map((row) => ({ ...row, canonicalPath: remoteFilePath(owner.hostId, row.canonicalPath) }))
  }))
  return results.flatMap((result) => result.status === "fulfilled" ? result.value : [])
}

export async function remoteTrustRevoke(path: string): Promise<void> {
  const resource = parseRemoteFilePath(path)
  const owner = resource && connectedWorkspaceOwners().find((owner) => owner.hostId === resource.hostId)
  if (!owner || !resource) throw new Error("Reconnect the host before changing workspace trust")
  await requestHost(owner, { method: "trust", params: { call: { action: "revoke", path: resource.path } } })
}
