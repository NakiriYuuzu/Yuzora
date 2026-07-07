import { useEffect } from "react"

import { perfSnapshot } from "../lib/ipc"
import { usePerfStore } from "../state/perfStore"

const POLL_INTERVAL_MS = 2000

// Polls the app's own cpu/memory every 2s and feeds the StatusBar chip. Skips a
// round while the window is unfocused so background polling stays quiet (same
// throttle as GitBridge's remote check).
export function PerfBridge() {
    useEffect(() => {
        const poll = () => {
            if (!document.hasFocus()) return
            void perfSnapshot()
                .then((snapshot) => usePerfStore.getState().setSnapshot(snapshot))
                .catch(() => {})
        }
        const id = setInterval(poll, POLL_INTERVAL_MS)
        return () => clearInterval(id)
    }, [])

    return null
}
