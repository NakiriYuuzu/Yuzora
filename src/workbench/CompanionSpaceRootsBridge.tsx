import { useEffect, useRef } from "react"
import { companionSpaceRootsSet, type CompanionSpaceRoot } from "@/lib/companionIpc"
import { parseRuntimeScope } from "@/lib/herdrProvider"
import type { HerdrSessionRuntime } from "@/lib/herdrTypes"
import { isAbsolutePath } from "@/lib/paths"
import { LOCAL_HOST_ID } from "@/lib/runtimeIdentity"
import { useCompanionStore } from "@/state/companionStore"
import { useHerdrStore } from "@/state/herdrStore"

const DEBOUNCE_MS = 500

/** Folders of this computer's Spaces: remote hosts' paths mean nothing to the gateway. */
function spaceRoots(runtimes: Record<string, HerdrSessionRuntime>): CompanionSpaceRoot[] {
  return Object.entries(runtimes).flatMap(([scope, runtime]) => {
    const { hostId, sessionName } = parseRuntimeScope(scope)
    if (hostId !== LOCAL_HOST_ID) return []
    return (runtime.snapshot?.spaces ?? []).flatMap(space =>
      isAbsolutePath(space.path) ? [{ session: sessionName, workspaceId: space.id, root: space.path }] : [])
  })
}

/**
 * Tells the Companion which folder each local Space is in, so a phone can
 * read its git state. Sent shortly after a change, and only while the
 * Experimental Companion is on.
 */
export function CompanionSpaceRootsBridge() {
  const enabled = useCompanionStore(s => s.overview?.settings.experimental === true)
  // A string, so a snapshot that moves nothing does not restart the debounce.
  const roots = useHerdrStore(s => JSON.stringify(spaceRoots(s.runtimesBySession)))
  // The Companion starts out knowing none.
  const sent = useRef("[]")

  useEffect(() => {
    if (!useCompanionStore.getState().overview) void useCompanionStore.getState().refresh().catch(() => undefined)
  }, [])

  useEffect(() => {
    if (!enabled || roots === sent.current) return
    const timer = setTimeout(() => {
      companionSpaceRootsSet(JSON.parse(roots) as CompanionSpaceRoot[]).then(
        () => { sent.current = roots },
        cause => console.warn("Companion Space roots update failed", cause),
      )
    }, DEBOUNCE_MS)
    return () => clearTimeout(timer)
  }, [enabled, roots])

  return null
}
