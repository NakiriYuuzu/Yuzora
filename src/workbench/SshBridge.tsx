import { useEffect } from "react"
import { listen } from "@tauri-apps/api/event"

import type { SftpProgressEvent } from "../lib/types"
import { useSftpStore } from "../state/sftpStore"

// Headless bridge for SFTP transfer progress (F5). SSH shell lifecycle events
// are no longer part of the application.
export function SshBridge() {
    useEffect(() => {
        const unlistenProgress = listen<SftpProgressEvent>("sftp://progress", (e) => {
            useSftpStore.getState().applyProgress(e.payload)
        })
        return () => {
            void unlistenProgress.then((fn) => fn())
        }
    }, [])

    return null
}
