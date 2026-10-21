import { create } from "zustand"
import { showPane } from "./client"
import { paneFromMessage } from "./lib/deepLink"

/** A waiting service worker with a new build; the user decides when to reload. */
export const useUpdate = create<{ waiting: ServiceWorker | null }>(() => ({ waiting: null }))

export function registerServiceWorker(): void {
  if (!("serviceWorker" in navigator) || location.protocol !== "https:") return
  void navigator.serviceWorker.register("/sw.js", { scope: "/" }).then((registration) => {
    const offer = (worker: ServiceWorker | null) => {
      // Only an update: the first install has no page to replace.
      if (worker && navigator.serviceWorker.controller) useUpdate.setState({ waiting: worker })
    }
    offer(registration.waiting)
    registration.addEventListener("updatefound", () => {
      const worker = registration.installing
      worker?.addEventListener("statechange", () => {
        if (worker.state === "installed") offer(worker)
      })
    })
  }, () => undefined)
  // A tapped notification asks the running app to open its pane.
  navigator.serviceWorker.addEventListener("message", (event) => {
    const target = paneFromMessage(event.data)
    if (target) showPane(target)
  })
  let reloading = false
  navigator.serviceWorker.addEventListener("controllerchange", () => {
    if (reloading || !useUpdate.getState().waiting) return
    reloading = true
    location.reload()
  })
}

export function applyUpdate(): void {
  useUpdate.getState().waiting?.postMessage("yzc-skip-waiting")
}
