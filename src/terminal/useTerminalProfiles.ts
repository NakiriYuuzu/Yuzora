import { useEffect, useState } from "react"

import { ptyListProfiles } from "@/lib/ipc"
import type { TerminalProfile } from "@/lib/types"

export function useTerminalProfiles(): TerminalProfile[] {
    const [profiles, setProfiles] = useState<TerminalProfile[]>([])

    useEffect(() => {
        let disposed = false
        void ptyListProfiles()
            .then((next) => {
                if (!disposed && Array.isArray(next)) setProfiles(next)
            })
            .catch(() => undefined)
        return () => {
            disposed = true
        }
    }, [])

    return profiles
}
