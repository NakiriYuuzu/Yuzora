import { documentWorkspaceEpoch, getDocument } from "@/editor/documentRegistry"
import { logUserAction } from "@/features/logs/userAction"
import { showActionError } from "@/lib/actionFeedback"
import i18n from "@/lib/i18n"
import { useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

/** How the OS handed Yuzora the files: a Finder/Explorer drop, or "Open With" / Explorer verbs. */
export type ExternalFileSource = "drop" | "os"

/**
 * Opens OS files as editable tabs in the editor group that was active when they arrived.
 * Nothing opens if the caller unmounted, the workspace changed, or the group closed while the documents loaded.
 */
export async function openExternalFiles(
  paths: string[],
  source: ExternalFileSource,
  isMounted: () => boolean = () => true
): Promise<void> {
  if (paths.length === 0 || !isMounted()) return

  const owner = useWorkspaceStore.getState()
  const group = owner.groups[owner.activeGroupIndex]
  if (!group) return
  const epoch = documentWorkspaceEpoch()
  const results = await Promise.allSettled(paths.map((path) => getDocument(path)))
  const workspace = useWorkspaceStore.getState()
  if (!isMounted() || epoch !== documentWorkspaceEpoch()
    || owner.workspacePath !== workspace.workspacePath
    || owner.workspaceCapabilityId !== workspace.workspaceCapabilityId) return
  const groupIndex = group.id === undefined
    ? workspace.groups.indexOf(group)
    : workspace.groups.findIndex(candidate => candidate.id === group.id)
  if (groupIndex === -1) return
  const acceptedPaths = paths.filter((_, index) => results[index]?.status === "fulfilled")
  const firstFailure = results.find((result) => result.status === "rejected")

  if (acceptedPaths.length > 0) {
    const ui = useUiStore.getState()
    if (ui.mode !== "ade" && ui.mode !== "files") ui.setMode("files")
    for (const path of acceptedPaths) workspace.openTab(path, groupIndex)
    if (source === "drop") {
      void logUserAction("open_dropped_files", `Opened ${acceptedPaths.length} dropped file(s)`, {
        count: acceptedPaths.length,
      })
    } else {
      void logUserAction("open_os_files", `Opened ${acceptedPaths.length} file(s) from the OS`, {
        count: acceptedPaths.length,
      })
    }
  }

  if (firstFailure?.status === "rejected") {
    void showActionError(
      i18n.t(source === "drop" ? "fileDrop.openAction" : "openRequests.openFileAction", { ns: "workbench" }),
      firstFailure.reason
    )
  }
}
