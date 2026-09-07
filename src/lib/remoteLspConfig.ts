import { runtimeWorkspaceService } from "./remoteFiles"
import { requestWorkspace } from "./remoteTrust"
import { parseRemoteFilePath, remoteFilePath } from "./runtimeIdentity"
import type { HostOperation } from "./hostIpc"
import type { LspConfig, LspServerInfo } from "./types"

type Call = Extract<HostOperation, { method: "lspConfig" }>["params"]["call"]
async function call<T>(context: string, action: Call): Promise<T> {
  const service = runtimeWorkspaceService(context)
  return requestWorkspace(service, { method: "lspConfig", params: { workspace: service.capabilityId, call: action } })
}
function project(config: LspConfig, context: string): LspConfig {
  const hostId = parseRemoteFilePath(context)!.hostId
  return { ...config, workspaces: Object.fromEntries(Object.entries(config.workspaces).map(([root, settings]) => [remoteFilePath(hostId, root), settings])) }
}
export async function remoteLspConfigGet(context: string): Promise<LspConfig> {
  return project(await call(context, { action: "get" }), context)
}
export async function remoteLspConfigStale(context: string): Promise<string[]> {
  const hostId = parseRemoteFilePath(context)!.hostId
  return (await call<string[]>(context, { action: "stale" })).map((path) => remoteFilePath(hostId, path))
}
export async function remoteLspConfigClear(context: string, workspace: string): Promise<LspConfig> {
  const path = parseRemoteFilePath(workspace)
  if (!path || path.hostId !== parseRemoteFilePath(context)!.hostId) throw new Error("LSP settings belong to another host")
  return project(await call(context, { action: "clearStale", path: path.path }), context)
}
export async function remoteLspConfigSet(context: string, workspace: string | null, language: string, serverId: string): Promise<LspConfig> {
  if (workspace && workspace !== context) throw new Error("LSP settings belong to another workspace")
  const service = runtimeWorkspaceService(context)
  const config = await call<LspConfig>(context, { action: "set", language, serverId, global: workspace === null })
  const remote = await import("./remoteLsp")
  service.assertCurrent()
  await remote.restartConfiguredRemoteLsp(context, workspace === null, language)
  return project(config, context)
}
export async function remoteLspConfigDetect(context: string, language: string, global: boolean): Promise<LspServerInfo> {
  const info = await call<LspServerInfo>(context, { action: "detect", language, global })
  return { ...info, workspace: global ? "" : context }
}
