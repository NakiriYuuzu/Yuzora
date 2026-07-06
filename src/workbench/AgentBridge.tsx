import { useEffect, useMemo, useState } from "react"

import { createAcpConnection, type AgentConnection } from "../agent/acpConnection"
import { loadAgentSettings, resolveAgentCommand } from "../app/workbench/SettingsDialog"
import { agentKill, agentList, agentSetTrace } from "../lib/ipc"
import { useAgentStore } from "../state/agentStore"
import { useWorkspaceStore } from "../state/workspaceStore"

// Headless bridge wiring the ACP connection lifecycle to app-level agent state.
export function AgentBridge() {
    const workspacePath = useWorkspaceStore((s) => s.workspacePath)
    const connection = useMemo(createStoreBackedConnection, [])
    const [startupTraceSettled, setStartupTraceSettled] = useState(false)

    useEffect(() => {
        let cancelled = false
        void (async () => {
            await agentSetTrace(loadAgentSettings().traceEnabled).catch(() => undefined)
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
        let cancelled = false
        void recoverStaleAgents(workspacePath, connection, () => cancelled)
        return () => {
            cancelled = true
        }
    }, [workspacePath, connection, startupTraceSettled])

    return null
}

function createStoreBackedConnection(): AgentConnection {
    return createAcpConnection({
        command: () => resolveAgentCommand(loadAgentSettings()),
        onTranscript: (sessionId, transcript) => {
            useAgentStore.getState().replaceTranscript(sessionId, transcript)
        },
        onAvailableCommands: (sessionId, commands) => {
            useAgentStore.getState().setAvailableCommands(sessionId, commands)
        },
        onSessionInfo: (sessionId, info) => {
            useAgentStore.getState().onSessionInfo(sessionId, info)
        },
        onPermissionRequest: (sessionId, block, choose) => {
            useAgentStore.getState().onPermissionRequest(sessionId, block, choose)
        }
    })
}

async function recoverStaleAgents(
    cwd: string,
    connection: AgentConnection,
    isCancelled: () => boolean
) {
    let staleIds: string[]
    try {
        staleIds = await agentList(cwd)
    } catch {
        return
    }
    if (isCancelled() || staleIds.length === 0) return

    try {
        for (const id of staleIds) {
            if (isCancelled()) return
            await agentKill(id)
        }
    } catch {
        return
    }
    if (isCancelled()) return

    useAgentStore.getState().reset()
    useAgentStore.getState().setConnection(connection)
}
