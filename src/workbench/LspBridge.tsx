import { useEffect } from "react"
import { listen } from "@tauri-apps/api/event"

import { useWorkspaceStore } from "../state/workspaceStore"
import { useLspStore } from "../state/lspStore"
import { stopWorkspace } from "../lsp/lspManager"
import type { LspServerInfo } from "../lib/types"

// Headless bridge wiring LSP lifecycle to app-level state (T6).
export function LspBridge() {
    const workspacePath = useWorkspaceStore((s) => s.workspacePath)

    // On workspace change, tear down the LEAVING workspace's clients and clear
    // reactive state. The cleanup closure captures the previous workspacePath,
    // so React runs it (before the next effect) with the old value.
    useEffect(() => {
        return () => {
            if (!workspacePath) return
            stopWorkspace(workspacePath)
            useLspStore.getState().reset()
        }
    }, [workspacePath])

    // Server status updates from the Rust side feed the status bar / Settings.
    // Drop events for a workspace the UI has already left (the Rust side keys by
    // raw workspace string; a slow event from the old one must not leak into the
    // new one). Read the current workspace at event time, not from a closure
    // snapshot, so a late event is compared against the live value (S1).
    useEffect(() => {
        const unlisten = listen<LspServerInfo>("lsp:server-status", (e) => {
            if (e.payload.workspace !== useWorkspaceStore.getState().workspacePath) return
            useLspStore.getState().setServerInfo(e.payload)
        })
        return () => {
            void unlisten.then((fn) => fn())
        }
    }, [])

    return null
}
