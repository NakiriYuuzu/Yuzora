import { useHostStore } from "@/state/hostStore"
import { invoke } from "./ipc"
import { remotePreviewSource, retainRemoteWorkspace, runtimeWorkspaceService } from "./remoteFiles"
import { relativeRemoteHostPath, remoteFilePath, sameConnection } from "./runtimeIdentity"
import type { FileNameSearchResponse } from "./fileNameSearchTypes"

/** null means legacy helper/SFTP: only these remote backends use the bounded BFS. */
export async function searchRemoteFileNames(root: string, query: string): Promise<FileNameSearchResponse | null> {
  const source = remotePreviewSource(root)
  if (source.source.kind === "sftp") return null
  const service = runtimeWorkspaceService(root)
  const connection = useHostStore.getState().hosts[service.owner.hostId]?.connection
  service.assertCurrent()
  if (!connection || !sameConnection(connection.owner, service.owner)) throw new Error("Remote workspace connection changed; response discarded")
  if (!connection.hello.methods.includes("fileNameSearch")) return null
  const release = retainRemoteWorkspace(root)
  try {
    const operation: { method: "fileNameSearch"; params: { workspace: string; query: string } } = {
      method: "fileNameSearch", params: { workspace: service.capabilityId, query },
    }
    const result = await invoke<FileNameSearchResponse>("host_request", { owner: service.owner, operation })
    service.assertCurrent()
    return {
      ...result,
      files: result.files.filter((file) => !file.isDir && file.kind === "file" && relativeRemoteHostPath(service.root, file.path) !== null)
        .map((file) => ({ ...file, path: remoteFilePath(service.owner.hostId, file.path, service.root) })),
    }
  } finally { await release() }
}
