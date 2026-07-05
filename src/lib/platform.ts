import { isTauri } from "@tauri-apps/api/core"

/**
 * True only inside the Tauri shell on macOS — the sole case where the native
 * traffic lights overlay the UI (titleBarStyle: Overlay) and the rail must
 * reserve space for them. Windows/Linux keep their native title bar.
 */
export function showsNativeTrafficLights(): boolean {
    return isTauri() && /Mac/.test(navigator.userAgent)
}
