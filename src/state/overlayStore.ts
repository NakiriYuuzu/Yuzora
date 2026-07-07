import { useEffect } from "react"
import { create } from "zustand"

import { useContextMenuStore } from "./contextMenuStore"
import { useDiffModalStore } from "./diffModalStore"
import { useSshStore } from "./sshStore"
import { useUiStore } from "./uiStore"

// The preview child webview (P3) is a native layer that paints above every DOM
// overlay, so any open modal/popover/menu would be hidden behind it — and an
// askpass dialog that pops up during a background git op would be unreachable,
// freezing the app. useAnyOverlayOpen is the single z-order gate: PreviewPanel
// hides the webview whenever it returns true.
//
// Store-backed overlays (settings, external-change resolver, context menu, diff
// modal, ssh auth prompt) are read directly below. Overlays whose open state is
// only local component state (command palette, branch popover, askpass) register
// via useOverlayPresence so the gate can see them without lifting their state.

interface OverlayState {
    count: number
    push: () => void
    pop: () => void
}

export const useOverlayStore = create<OverlayState>((set) => ({
    count: 0,
    push: () => set((s) => ({ count: s.count + 1 })),
    pop: () => set((s) => ({ count: Math.max(0, s.count - 1) }))
}))

// Register a locally-managed overlay's open state with the central gate.
export function useOverlayPresence(active: boolean): void {
    const push = useOverlayStore((s) => s.push)
    const pop = useOverlayStore((s) => s.pop)
    useEffect(() => {
        if (!active) return
        push()
        return () => pop()
    }, [active, push, pop])
}

// True when ANY overlay is open (see the module comment).
export function useAnyOverlayOpen(): boolean {
    const settingsOpen = useUiStore((s) => s.settingsOpen)
    const resolverOpen = useUiStore((s) => s.resolverPath !== null)
    const contextMenuOpen = useContextMenuStore((s) => s.kind !== null)
    const diffModalOpen = useDiffModalStore((s) => s.open)
    const sshAuthOpen = useSshStore((s) => s.pendingAuthHostId !== null)
    const localCount = useOverlayStore((s) => s.count)
    return (
        settingsOpen ||
        resolverOpen ||
        contextMenuOpen ||
        diffModalOpen ||
        sshAuthOpen ||
        localCount > 0
    )
}
