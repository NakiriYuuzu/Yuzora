import { invoke } from "./ipc"

interface Workspace { generation?: number }
const workspaces = new Map<string, Workspace>()

export function closeNativeGitWorkspace(path: string): void {
  const workspace = workspaces.get(path)
  workspaces.delete(path)
  if (workspace?.generation !== undefined) {
    void invoke("git_close_workspace", { path, generation: workspace.generation }).catch(() => {})
  }
}

export async function invokeNativeGit<T>(command: string, args: Record<string, unknown>): Promise<T> {
  if (command !== "git_detect" && command !== "git_bootstrap") return invoke<T>(command, args)
  const path = String(args.path)
  const workspace: Workspace = { generation: workspaces.get(path)?.generation }
  workspaces.set(path, workspace)
  try {
    const value = await invoke<T & { workspaceGeneration?: number }>(command, args)
    const generation = value.workspaceGeneration
    if (workspaces.get(path) !== workspace) {
      if (generation !== undefined) await invoke("git_close_workspace", { path, generation }).catch(() => {})
      throw new Error("Git workspace changed before discovery completed")
    }
    workspace.generation = generation
    return value
  } catch (error) {
    if (workspaces.get(path) === workspace) closeNativeGitWorkspace(path)
    throw error
  }
}
