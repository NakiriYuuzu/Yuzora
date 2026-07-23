import { useEffect, useMemo, useRef, useState } from "react"

import { type AgentConnection } from "../agent/acpConnection"
import { createAgentRouter } from "../agent/agentRouter"
import { loadAgentSettings } from "../app/workbench/settingsStorage"
import { agentKill, agentList, agentSetTrace } from "../lib/ipc"
import { firstAbsolutePath } from "../lib/paths"
import { initBuiltinPiAdapterCommand } from "../lib/platform"
import { useAgentStore } from "../state/agentStore"
import { normalizeWorkspacePath } from "../state/recentWorkspaces"
import { loadSessionIndex } from "../state/sessionIndexStorage"
import { useWorkspaceStore } from "../state/workspaceStore"

// Headless bridge wiring the ACP connection lifecycle to app-level agent state.
export function AgentBridge() {
    const workspacePath = useWorkspaceStore((s) => s.workspacePath)
    const connection = useMemo(createStoreBackedConnection, [])
    const [startupTraceSettled, setStartupTraceSettled] = useState(false)
    const recoveryRef = useRef<Promise<void> | null>(null)
    const workspaceGenerationRef = useRef(0)

    useEffect(() => {
        let cancelled = false
        void (async () => {
            // builtin pi adapter 的 command cache 必須先於任何 session/new
            // ready——resolveAgentCommandRoute 是同步的。
            await Promise.all([
                agentSetTrace(loadAgentSettings().traceEnabled).catch(() => undefined),
                initBuiltinPiAdapterCommand().catch(() => undefined)
            ])
            if (cancelled) return
            useAgentStore.getState().setConnection(connection)
            setStartupTraceSettled(true)
        })()
        return () => {
            cancelled = true
            if (useAgentStore.getState().connection === connection) {
                useAgentStore.getState().setConnection(null)
            }
        }
    }, [connection])

    useEffect(() => {
        if (!workspacePath || !startupTraceSettled) return
        const cwd = firstAbsolutePath(workspacePath)
        if (!cwd) return
        const generation = ++workspaceGenerationRef.current
        let cancelled = false
        const isCurrent = () => !cancelled && workspaceGenerationRef.current === generation

        void (async () => {
            // Recovery is app-start work and stays single-flight across workspace
            // changes/StrictMode setup cycles. Each workspace still hydrates its own
            // Session Index after that shared prerequisite settles.
            recoveryRef.current ??= recoverStaleAgents(cwd, connection)
            await recoveryRef.current
            if (!isCurrent()) return
            hydrateSessionIndexForWorkspace(cwd)
            useAgentStore.getState().markWorkspaceHydrated(cwd)
        })()

        return () => {
            cancelled = true
            if (workspaceGenerationRef.current === generation) {
                workspaceGenerationRef.current += 1
            }
            // AgentZone may have started an interactive prepare for this workspace.
            // Tear down only a prepared-only child; owned sessions remain intact.
            void connection.disposePrepared?.(cwd).catch(() => undefined)
        }
    }, [workspacePath, connection, startupTraceSettled])

    return null
}

function createStoreBackedConnection(): AgentConnection {
    return createAgentRouter({
        onTranscript: (sessionId, transcript) => {
            useAgentStore.getState().replaceTranscript(sessionId, transcript)
        },
        onAvailableCommands: (sessionId, commands) => {
            useAgentStore.getState().setAvailableCommands(sessionId, commands)
        },
        onSessionInfo: (sessionId, info) => {
            useAgentStore.getState().onSessionInfo(sessionId, info)
        },
        onUsage: (sessionId, usage) => {
            useAgentStore.getState().setUsage(sessionId, usage)
        },
        onConfigOptions: (sessionId, configOptions) => {
            useAgentStore.getState().replaceConfigOptions(sessionId, configOptions)
        },
        onSessionTitle: (sessionId, title) => {
            useAgentStore.getState().applyAgentTitle(sessionId, title)
        },
        onElicitationRequest: (sessionId, request, respond) => {
            useAgentStore.getState().onElicitationRequest(sessionId, request, respond)
        },
        onPermissionRequest: (sessionId, block, choose) => {
            useAgentStore.getState().onPermissionRequest(sessionId, block, choose)
        }
    })
}

// 重啟後從 Session Index 重建當前 workspace 的 restored sessions（先回收孤兒
// 再 hydrate，避免把已死行程的 session 誤判成可續聊）。沒有絕對路徑的 workspace
// 不 hydrate——沿既有 cwd 防呆慣例。
function hydrateSessionIndexForWorkspace(workspacePath: string): void {
    const cwd = firstAbsolutePath(workspacePath)
    if (!cwd) return
    const entries = loadSessionIndex().filter(
        (entry) => normalizeWorkspacePath(entry.cwd) === normalizeWorkspacePath(cwd)
    )
    if (entries.length === 0) return
    useAgentStore.getState().hydrateRestoredSessions(entries)
}

async function recoverStaleAgents(cwd: string, connection: AgentConnection) {
    let staleIds: string[]
    try {
        staleIds = await agentList(cwd)
    } catch {
        return
    }
    if (staleIds.length === 0) return

    const owned = new Set(
        await (connection as { ownedProcessIds?: () => Promise<string[]> }).ownedProcessIds?.() ?? []
    )

    try {
        for (const id of staleIds) {
            if (owned.has(id)) continue
            await agentKill(id, "app_exit")
        }
    } catch {
        return
    }
}
