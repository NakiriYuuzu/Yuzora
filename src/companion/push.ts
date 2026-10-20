import { call } from "./client"
import { fromBase64Url } from "./lib/base64url"
import { useUpdate } from "./serviceWorker"

/**
 * What the notifications dialog can offer. The browser decides the first
 * four; `update-first` is `off` with a newer worker waiting, `updating` the
 * same with one still installing; `on` needs this browser's own subscription
 * at the desktop and in the browser, made for the desktop's key.
 */
export type PushState = "unavailable" | "home-screen" | "denied" | "off" | "update-first" | "updating" | "on"

const onIos = () => /iPad|iPhone|iPod/.test(navigator.userAgent) || (/Macintosh/.test(navigator.userAgent) && navigator.maxTouchPoints > 1)
const installed = () => (navigator as Navigator & { standalone?: boolean }).standalone === true || matchMedia("(display-mode: standalone)").matches

// Subscribing needs a worker that is already active.
async function activeRegistration(): Promise<ServiceWorkerRegistration | null> {
  const registration = await navigator.serviceWorker.getRegistration()
  return registration?.active ? registration : null
}

// The active worker may predate the push handler: a subscription made against
// it gets pushes nobody shows, and a browser revokes such subscriptions.
function updateWaiting(registration: ServiceWorkerRegistration): boolean {
  // The update action works on the store's worker.
  if (registration.waiting && !useUpdate.getState().waiting) useUpdate.setState({ waiting: registration.waiting })
  return useUpdate.getState().waiting !== null
}

// A newer worker still installing is the same hazard a few seconds earlier; the registration has an active worker by now.
const updateInstalling = (registration: ServiceWorkerRegistration) => Boolean(registration.installing)

/**
 * Calls `changed` whenever the worker now installing changes state, or at once
 * when none is installing any more. It follows that one worker only: whoever
 * is called reads the state again and watches again. Stop it to drop the listener.
 */
export function watchInstalling(changed: () => void): () => void {
  let live = true
  let detach: (() => void) | null = null
  void navigator.serviceWorker.getRegistration().then((registration) => {
    if (!live) return
    const worker = registration?.installing
    if (!worker) return changed()
    worker.addEventListener("statechange", changed)
    detach = () => worker.removeEventListener("statechange", changed)
  })
  return () => {
    live = false
    detach?.()
  }
}

// A subscription made for another key (the desktop's key was replaced) is dead: the push service refuses what is sent to it.
function madeFor(subscription: PushSubscription, vapidPublicKey: string): boolean {
  const key = subscription.options.applicationServerKey
  const expected = fromBase64Url(vapidPublicKey)
  return key !== null && key.byteLength === expected.length && new Uint8Array(key).every((byte, index) => byte === expected[index])
}

export async function pushState(vapidPublicKey: string): Promise<PushState> {
  // Safari only has push for an installed web app, and hides the API otherwise.
  if (onIos() && !installed()) return "home-screen"
  if (!("serviceWorker" in navigator) || !("PushManager" in window) || !("Notification" in window)) return "unavailable"
  if (Notification.permission === "denied") return "denied"
  const registration = await activeRegistration()
  if (!registration) return "unavailable"
  // Several browsers can be signed in as one device: the desktop answers for this browser's endpoint only.
  const subscription = await registration.pushManager.getSubscription()
  const status = await call<{ subscribed: boolean }>("push.status", { endpoint: subscription?.endpoint ?? null })
  if (status.subscribed && subscription && madeFor(subscription, vapidPublicKey)) return "on"
  if (updateWaiting(registration)) return "update-first"
  return updateInstalling(registration) ? "updating" : "off"
}

/**
 * Run it straight from the tap: the permission prompt needs the gesture, so
 * nothing is awaited before it.
 */
export async function enablePush(vapidPublicKey: string): Promise<PushState> {
  if (useUpdate.getState().waiting) return "update-first"
  const permission = Notification.permission === "default" ? await Notification.requestPermission() : Notification.permission
  if (permission !== "granted") return permission === "denied" ? "denied" : "off"
  const registration = await activeRegistration()
  if (!registration) return "unavailable"
  if (updateWaiting(registration)) return "update-first"
  if (updateInstalling(registration)) return "updating"
  // A subscription made with another key can't be reused: start from a new one.
  await (await registration.pushManager.getSubscription())?.unsubscribe()
  const subscription = await registration.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: fromBase64Url(vapidPublicKey) })
  const { endpoint, keys } = subscription.toJSON()
  if (!endpoint || !keys?.p256dh || !keys.auth) throw new Error("incomplete push subscription")
  await call("push.subscribe", { endpoint, keys: { p256dh: keys.p256dh, auth: keys.auth } })
  return "on"
}

export async function disablePush(): Promise<PushState> {
  const subscription = await (await activeRegistration())?.pushManager.getSubscription()
  // Only this browser's endpoint: another browser of the same device keeps its own.
  if (subscription) await call("push.unsubscribe", { endpoint: subscription.endpoint })
  // The desktop has forgotten this browser; a browser that keeps the subscription changes nothing.
  await subscription?.unsubscribe().catch(() => false)
  return "off"
}
