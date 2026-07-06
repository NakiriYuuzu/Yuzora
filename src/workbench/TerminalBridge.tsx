import { useEffect } from "react"

import { ptyCloseWorkspace } from "../lib/ipc"
import { useTerminalStore } from "../state/terminalStore"
import { useWorkspaceStore } from "../state/workspaceStore"

// Headless bridge wiring terminal workspace lifecycle to app-level state (T8).
export function TerminalBridge() {
    const workspacePath = useWorkspaceStore((s) => s.workspacePath)

    // The cleanup closure captures the workspace being left. App-close cleanup
    // is owned by the Rust RunEvent path in T11; this bridge handles switches.
    useEffect(() => {
        return () => {
            if (!workspacePath) return
            void ptyCloseWorkspace(workspacePath)
            useTerminalStore.getState().reset()
        }
    }, [workspacePath])

    return null
}
