import { useEffect } from "react"
import { listen } from "@tauri-apps/api/event"

import type { SftpProgressEvent, SshExitEvent } from "../lib/types"
import { useSftpStore } from "../state/sftpStore"
import { useSshStore } from "../state/sshStore"

// Headless bridge (FEAT-2): a server-side SSH session close reaches every host
// row / panel via the store, regardless of whether that host's terminal is
// currently mounted (a non-active tab unmounts its xterm). The terminal still
// shows its own inline [Disconnected] notice; this drives the store status.
// Also fans SFTP transfer progress (F5) into sftpStore so the browser's progress
// UI updates without every transfer awaiting its own listener.
export function SshBridge() {
    useEffect(() => {
        const unlistenExit = listen<SshExitEvent>("ssh://exit", (e) => {
            useSshStore.getState().markExit(e.payload.sessionId)
        })
        const unlistenProgress = listen<SftpProgressEvent>("sftp://progress", (e) => {
            useSftpStore.getState().applyProgress(e.payload)
        })
        return () => {
            void unlistenExit.then((fn) => fn())
            void unlistenProgress.then((fn) => fn())
        }
    }, [])

    return null
}
