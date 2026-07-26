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
    // `document` / `window` 在呼叫當下就捕捉進 closure，讓延後執行的路徑
    // （`finish` → `remove`）完全不查全域。下面那個 fallback timer 必然會比
    // 呼叫者活得久——jsdom 與被節流的瀏覽器都可能永遠不觸發 transitionend，
    // 所以它是刻意保留的補刀。測試環境跑完一個檔案就會拆掉 jsdom，此時裸
    // `document` 參照會拋 ReferenceError，vitest 記成 Unhandled Error 並讓整個
    // run exit 1，即使每一個測試都通過（CI 上的 splash flake 即為此）。
    const doc = document
    const win = window
    const el = doc.getElementById(SPLASH_ID)
    if (!el || el.classList.contains(LEAVE_CLASS)) return

    const remove = () => {
        el.remove()
        doc.documentElement.style.removeProperty("background-color")
    }

    if (win.matchMedia?.("(prefers-reduced-motion: reduce)").matches) {
        remove()
        return
    }

    el.classList.add(LEAVE_CLASS)
    let done = false
    let fallback = 0
    const finish = () => {
        if (done) return
        done = true
        // transitionend 先到時把補刀取消，不留一個 400ms 後才空轉的 timer。
        win.clearTimeout(fallback)
        remove()
    }
    el.addEventListener("transitionend", finish, { once: true })
    // jsdom and throttled browsers may never fire transitionend.
    fallback = win.setTimeout(finish, FADE_MS + 150)
}
