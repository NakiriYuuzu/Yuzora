import { showActionError } from "@/lib/actionFeedback"
import { canonicalRuntimeWorkspace, parseRuntimeScope } from "@/lib/herdrProvider"
import i18n from "@/lib/i18n"
import { takeOpenRequests, type OpenRequest } from "@/lib/ipc"
import { openExternalFiles } from "@/lib/openExternalFiles"
import { workspacePathBasename } from "@/lib/paths"
import { isTauri } from "@/lib/platform"
import { LOCAL_HOST_ID } from "@/lib/runtimeIdentity"
import { openWorkspaceAtPath } from "@/lib/workspaceActions"
import { pathsMatch, useHerdrStore } from "@/state/herdrStore"

/** How long a launch-time folder waits for the local HERDR Session before opening as a plain workspace. */
export const HERDR_SETTLE_TIMEOUT_MS = 5_000

let launchRequests: Promise<OpenRequest[]> | null = null

/**
 * The requests queued before the frontend loaded (cold start from Finder,
 * the Dock or Explorer). Taken once and shared, so the session restore can
 * see a launch folder before the bridge that opens it.
 */
export function launchOpenRequests(): Promise<OpenRequest[]> {
    launchRequests ??= isTauri() ? takeOpenRequests().catch(() => []) : Promise.resolve([])
    return launchRequests
}

type HerdrStoreState = ReturnType<typeof useHerdrStore.getState>

/**
 * The local Session a folder can become a Space in, `false` when it opens as
 * a plain workspace, or `null` while HERDR may still be starting up.
 */
function localSpaceSession(state: HerdrStoreState): string | false | null {
    const sessionName = state.selectedSessionName
    if (sessionName && parseRuntimeScope(sessionName).hostId !== LOCAL_HOST_ID) return false
    if (sessionName && state.connectionState === "ready" && state.capabilities) return sessionName
    if (state.herdrStartup.state === "failed") return false
    if (sessionName && state.connectionState !== "idle" && state.connectionState !== "connecting") return false
    return null
}

function waitForLocalSpaceSession(): Promise<string | false> {
    const settled = localSpaceSession(useHerdrStore.getState())
    if (settled !== null) return Promise.resolve(settled)
    return new Promise((resolve) => {
        let unsubscribe = () => {}
        const finish = (value: string | false) => {
            clearTimeout(timer)
            unsubscribe()
            resolve(value)
        }
        const timer = setTimeout(() => finish(false), HERDR_SETTLE_TIMEOUT_MS)
        unsubscribe = useHerdrStore.subscribe((state) => {
            const next = localSpaceSession(state)
            if (next !== null) finish(next)
        })
    })
}

/**
 * A folder from the OS: focus the local Space already rooted there, else
 * create one, else open the folder as a plain workspace. Both HERDR paths
 * switch the editor workspace behind the unsaved-changes guard.
 */
export async function openFolderFromOs(path: string): Promise<void> {
    try {
        const sessionName = await waitForLocalSpaceSession()
        if (sessionName) {
            const herdr = useHerdrStore.getState()
            const root = await canonicalRuntimeWorkspace(sessionName, path)
            const space = herdr.selectedSessionName === sessionName
                ? herdr.spaces().find((item) => pathsMatch(item.path, root))
                : undefined
            if (space || herdr.canCreateSpace()) {
                const result = space
                    ? await herdr.activateSpace({ sessionName, workspaceId: space.id, path: root })
                    : await herdr.createSpaceFromFolder(root, workspacePathBasename(root))
                if (!result.ok && !result.cancelled) {
                    throw new Error(result.error ?? i18n.t("openRequests.openFolderAction", { ns: "workbench" }))
                }
                return
            }
        }
        await openWorkspaceAtPath(path)
    } catch (error) {
        void showActionError(i18n.t("openRequests.openFolderAction", { ns: "workbench" }), error)
    }
}

let queue: Promise<void> = Promise.resolve()

/** Opens requests one at a time in arrival order; consecutive files open as one batch. */
export function processOpenRequests(requests: OpenRequest[]): Promise<void> {
    const run = async () => {
        let files: string[] = []
        const flushFiles = async () => {
            if (files.length === 0) return
            const batch = files
            files = []
            await openExternalFiles(batch, "os")
        }
        for (const request of requests) {
            if (request.kind === "file") {
                files.push(request.path)
                continue
            }
            await flushFiles()
            await openFolderFromOs(request.path)
        }
        await flushFiles()
    }
    queue = queue.then(run).catch((error) => console.warn("open request failed", error))
    return queue
}

/** Test seam: forget the memoized launch batch and the pending queue. */
export function resetOpenRequestsForTest(): void {
    launchRequests = null
    queue = Promise.resolve()
}
