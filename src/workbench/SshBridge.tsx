import { useEffect } from "react"
import { listen } from "@tauri-apps/api/event"

import type { SftpProgressEvent } from "../lib/types"
import { forgetSftpHosts, useSftpStore } from "../state/sftpStore"
import { useSshStore } from "../state/sshStore"

// Headless bridge for SFTP transfer progress (F5). SSH shell lifecycle events
// are no longer part of the application.
export function SshBridge() {
    useEffect(() => {
        const known = new Set(useSshStore.getState().hosts.map(host => host.id))
        const retained = useSftpStore.getState()
        const retainedHosts = new Set([
            ...Object.keys(retained.remote),
            ...Object.values(retained.transfers).map(transfer => transfer.hostId)
        ])
        forgetSftpHosts([...retainedHosts].filter(hostId => !known.has(hostId)))
        const unsubscribeHosts = useSshStore.subscribe((state, previous) => {
            // Removing/replacing a host identity also replaces sessions. Renames
            // and additions keep that reference, so they need no retirement scan.
            if (state.hosts === previous.hosts || state.sessions === previous.sessions) return
            const current = new Set(state.hosts.map(host => host.id))
            forgetSftpHosts(previous.hosts.filter(host => !current.has(host.id)).map(host => host.id))
        })
        const unlistenProgress = listen<SftpProgressEvent>("sftp://progress", (e) => {
            useSftpStore.getState().applyProgress(e.payload)
        })
        return () => {
            unsubscribeHosts()
            void unlistenProgress.then((fn) => fn())
        }
    }, [])

    return null
}
