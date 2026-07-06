import { useEffect } from "react"
import { listen } from "@tauri-apps/api/event"

import { devServerStopWorkspace } from "../lib/ipc"
import type { DevServerInfo } from "../lib/types"
import { usePreviewStore } from "../state/previewStore"
import { useWorkspaceStore } from "../state/workspaceStore"

// Headless bridge wiring managed dev-server lifecycle to app-level state (T9).
export function ProcessBridge() {
    const workspacePath = useWorkspaceStore((s) => s.workspacePath)

    // The cleanup closure captures the workspace being left. App-close cleanup
    // is owned by the Rust RunEvent path in T11; this bridge handles switches.
    useEffect(() => {
        return () => {
            if (!workspacePath) return
            void devServerStopWorkspace(workspacePath)
            usePreviewStore.getState().reset()
        }
    }, [workspacePath])

    // Drop late process events from workspaces the UI has already left. Read the
    // live workspace at event time, mirroring LspBridge's stale-event guard.
    useEffect(() => {
        const unlisten = listen<DevServerInfo>("dev-server:status", (event) => {
            if (event.payload.workspace !== useWorkspaceStore.getState().workspacePath) return
            usePreviewStore.getState().setDevServer(event.payload)
        })
        return () => {
            void unlisten.then((fn) => fn())
        }
    }, [])

    return null
}
