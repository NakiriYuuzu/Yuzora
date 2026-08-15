const SPLASH_ID = "yz-splash"

/**
 * Dismisses the index.html startup splash. Removal is synchronous: startup
 * completion must not depend on transition events or timers because WebKit can
 * discard a dev-server document while Tauri IPC is falling back from its
 * custom protocol. A missing splash node is an idempotent no-op.
 */
export function dismissSplash(): void {
    const el = document.getElementById(SPLASH_ID)
    if (!el) return

    el.remove()
    document.documentElement.style.removeProperty("background-color")
}
