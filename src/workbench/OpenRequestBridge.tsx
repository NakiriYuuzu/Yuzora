import { useEffect } from "react"
import { listen } from "@tauri-apps/api/event"

import { takeOpenRequests, type OpenRequest } from "@/lib/ipc"
import { launchOpenRequests, processOpenRequests } from "@/lib/openRequests"
import { isTauri } from "@/lib/platform"
import { useWorkspaceStore } from "@/state/workspaceStore"

const OPEN_REQUESTS_AVAILABLE_EVENT = "open-requests:available"

function whenSessionRestoreReady(): Promise<void> {
  if (useWorkspaceStore.getState().sessionRestoreReady) return Promise.resolve()
  return new Promise((resolve) => {
    const unsubscribe = useWorkspaceStore.subscribe((state) => {
      if (!state.sessionRestoreReady) return
      unsubscribe()
      resolve()
    })
  })
}

/** Opens what Finder "Open With", the Dock and Explorer hand Yuzora: files as tabs, folders as Spaces. */
export function OpenRequestBridge() {
  useEffect(() => {
    if (!isTauri()) return

    let disposed = false
    let unlisten: (() => void) | undefined
    // Every request waits for the session restore: restoring a workspace
    // resets the editor groups and would drop tabs opened before it. The
    // launch batch is taken first, and take() drains the shared Rust queue,
    // so a request is never opened twice.
    const ready = launchOpenRequests().then(async (requests) => {
      await whenSessionRestoreReady()
      return requests
    })
    const open = (requests: OpenRequest[]) => {
      if (!disposed && requests.length > 0) void processOpenRequests(requests)
    }
    const drain = () => {
      void ready
        .then(() => takeOpenRequests())
        .then(open)
        .catch(() => {})
    }
    void ready.then(open)
    void listen(OPEN_REQUESTS_AVAILABLE_EVENT, drain)
      .then((nextUnlisten) => {
        if (disposed) {
          nextUnlisten()
          return
        }
        unlisten = nextUnlisten
        // A request queued after the launch take but before this listener
        // existed announced itself to no one; take it now. It still opens
        // after the launch batch, which was chained on `ready` first.
        drain()
      })
      .catch(() => {})

    return () => {
      disposed = true
      unlisten?.()
    }
  }, [])

  return null
}
