import { isTauri } from "@tauri-apps/api/core"

// Re-exported so the rest of the app detects the Tauri shell through this module
// instead of importing `@tauri-apps/api/core` directly (kept to ipc.ts + here).
export { isTauri }

/**
 * True only inside the Tauri shell on macOS — the sole case where the native
 * traffic lights overlay the UI (titleBarStyle: Overlay) and the rail must
 * reserve space for them. Windows/Linux keep their native title bar.
 */
export function showsNativeTrafficLights(): boolean {
    return isTauri() && isMacPlatform()
}

export function isWindowsPlatform(): boolean {
    return /Windows/.test(navigator.userAgent)
}

/**
 * True when the user agent reports a Mac host. Unknown platforms fall through
 * as non-Mac so UI labels prefer the Ctrl / neutral branch.
 */
export function isMacPlatform(): boolean {
    return /Mac/.test(navigator.userAgent)
}

export type ShortcutChord = "mod-k" | "mod-enter" | "mod-shift-enter"

/**
 * Platform-aware keyboard badge text only. Does not bind or interpret keydown
 * events; callers keep existing Mod-star or metaKey/ctrlKey handlers.
 */
export function shortcutLabel(chord: ShortcutChord): string {
    const mac = isMacPlatform()
    switch (chord) {
        case "mod-k":
            return mac ? "⌘K" : "Ctrl+K"
        case "mod-enter":
            return mac ? "⌘↵" : "Ctrl+Enter"
        case "mod-shift-enter":
            return mac ? "⇧⌘↵" : "Ctrl+Shift+Enter"
    }
}
