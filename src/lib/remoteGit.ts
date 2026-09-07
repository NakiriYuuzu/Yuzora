import { requestWorkspace } from "./remoteTrust"
import { runtimeWorkspaceService } from "./remoteFiles"
import { parseRemoteFilePath, remoteFilePath, sameConnection } from "./runtimeIdentity"
import type { GitEnvironment, GitBootstrapResult } from "./types"

const repositories = new Map<string, ReturnType<typeof runtimeWorkspaceService>>()
const reads = new Set(["git_status_cmd", "git_branches", "git_diff_content", "git_log_page", "git_commit_detail", "git_log_authors", "git_file_at_rev", "git_remote_probe"])

function environment(value: GitEnvironment, service: ReturnType<typeof runtimeWorkspaceService>): GitEnvironment {
  if (value.status !== "ready") {
    for (const [key, bound] of repositories) if (bound.uri === service.uri) repositories.delete(key)
    return value
  }
  const root = remoteFilePath(service.owner.hostId, value.root, service.root)
  if (repositories.size >= 128 && !repositories.has(root)) repositories.delete(repositories.keys().next().value!)
  repositories.set(root, service)
  return { ...value, root }
}

export async function invokeRemoteGit<T>(command: string, args: Record<string, unknown>): Promise<T> {
  const discovering = command === "git_detect" || command === "git_bootstrap"
  const uri = String(discovering ? args.path : args.repositoryRoot)
  const remembered = repositories.get(uri)
  const service = runtimeWorkspaceService(remembered?.uri ?? uri)
  const root = parseRemoteFilePath(uri)!
  if (root.hostId !== service.owner.hostId) throw new Error("Git repository belongs to a different host")
  if (!discovering && (!remembered || !sameConnection(remembered.owner, service.owner) || remembered.capabilityId !== service.capabilityId)) {
    if (!reads.has(command)) throw new Error("Refresh Git after reconnecting before modifying the repository")
    const detected = await requestWorkspace<GitEnvironment>(service, { method: "git", params: { workspace: service.capabilityId, repository_root: null, call: { command: "git_detect" } } })
    service.assertCurrent()
    if (detected.status !== "ready" || detected.root !== root.path) throw new Error("Git repository changed after reconnecting")
    environment(detected, service)
  }
  const routed = { ...args }
  delete routed.repositoryRoot
  if (discovering) delete routed.path
  delete routed.background // Remote credentials are owned by that host.
  service.assertCurrent()
  const value = await requestWorkspace<unknown>(service, { method: "git", params: {
    workspace: service.capabilityId, repository_root: discovering ? null : root.path,
    call: { command, ...(Object.keys(routed).length ? { args: routed } : {}) }
  } })
  service.assertCurrent()
  if (command === "git_detect") return environment(value as GitEnvironment, service) as T
  if (command === "git_bootstrap") {
    const result = value as GitBootstrapResult
    return { ...result, environment: environment(result.environment, service) } as T
  }
  return value as T
}
