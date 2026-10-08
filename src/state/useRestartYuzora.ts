import { useCallback } from "react"
import { relaunch } from "@tauri-apps/plugin-process"
import { useWorkspaceStore } from "./workspaceStore"

/** Relaunch is refused while any editor tab holds unsaved changes. */
export function useRestartYuzora(): { blocked: boolean; restart: () => Promise<void> } {
  const blocked = useWorkspaceStore((s) => s.groups.some((group) => group.tabs.some((tab) => tab.dirty)))
  const restart = useCallback(async () => {
    // Re-read at click time so a tab that became dirty after render still blocks.
    const dirty = useWorkspaceStore.getState().groups.some((group) => group.tabs.some((tab) => tab.dirty))
    if (dirty) return
    await relaunch()
  }, [])
  return { blocked, restart }
}
