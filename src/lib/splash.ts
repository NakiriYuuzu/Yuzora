const SPLASH_ID = "yz-splash"
const LEAVE_CLASS = "yz-splash-leave"
const FADE_MS = 250

/**
 * Dismisses the index.html startup splash. Idempotent by DOM state (no module
 * flag, so HMR/tests re-running it stay quiet): a missing splash node or one
 * already fading out is a no-op. The html inline background set by the inline
 * boot script is cleared together with the node so the app theme fully owns
 * the page background afterwards.
 */
export function dismissSplash(): void {
    const el = document.getElementById(SPLASH_ID)
    if (!el || el.classList.contains(LEAVE_CLASS)) return

    const remove = () => {
        el.remove()
        document.documentElement.style.removeProperty("background-color")
    }

    if (window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) {
        remove()
        return
    }

    el.classList.add(LEAVE_CLASS)
    let done = false
    const finish = () => {
        if (done) return
        done = true
        remove()
    }
    el.addEventListener("transitionend", finish, { once: true })
    // jsdom and throttled browsers may never fire transitionend.
    window.setTimeout(finish, FADE_MS + 150)
}
