/* global self, caches, clients, fetch, URL, Response */
// Yuzora Companion service worker. vite.companion.config.ts fills BUILD and
// PRECACHE from the bundle's contents; this file is served as /sw.js.
const BUILD = "__YZC_BUILD__"
const PRECACHE = ["__YZC_PRECACHE__"]
const CACHE = `yzc-${BUILD}`

self.addEventListener("install", (event) => {
  // One request at a time: the gateway rate-limits static files.
  event.waitUntil((async () => {
    const cache = await caches.open(CACHE)
    for (const url of PRECACHE) {
      const response = await fetch(url, { cache: "no-cache" })
      if (!response.ok) throw new Error(`precache ${url}: ${response.status}`)
      await cache.put(url, response)
    }
  })())
})

self.addEventListener("activate", (event) => {
  event.waitUntil((async () => {
    for (const name of await caches.keys()) {
      if (name.startsWith("yzc-") && name !== CACHE) await caches.delete(name)
    }
    await clients.claim()
  })())
})

// The page asks for this after the user accepts the update prompt.
self.addEventListener("message", (event) => {
  if (event.data === "yzc-skip-waiting") self.skipWaiting()
})

// A push carries only labels and ids; the words around them come from here.
const WORDS = {
  en: { blocked: "Waiting for input", done: "Finished", other: "New activity" },
  "zh-TW": { blocked: "等待輸入", done: "已完成", other: "有新的動態" },
}

// A push that can't be read still shows something: a browser drops the
// subscription of a worker that receives pushes without showing them.
self.addEventListener("push", (event) => {
  let payload = null
  try {
    payload = event.data ? event.data.json() : null
  } catch {
    // Not JSON: it is shown as generic activity below.
  }
  const [title, options] = notification(payload)
  event.waitUntil(self.registration.showNotification(title, options))
})

function notification(payload) {
  const words = WORDS[/^zh\b/i.test(self.navigator.language) ? "zh-TW" : "en"]
  const text = (value) => (typeof value === "string" ? value : "")
  const kind = payload?.kind
  if ((kind !== "blocked" && kind !== "done") || !text(payload.session) || !text(payload.pane)) return ["Yuzora", { body: words.other }]
  const space = text(payload.space)
  return [text(payload.title) || "Yuzora", {
    body: space ? `${space} · ${words[kind]}` : words[kind],
    // One notification per pane; a new push for it alerts again.
    tag: `${payload.session}\u0000${payload.pane}`,
    renotify: true,
    data: { session: payload.session, pane: payload.pane },
  }]
}

self.addEventListener("notificationclick", (event) => {
  event.notification.close()
  const { session, pane } = event.notification.data ?? {}
  event.waitUntil(openPane(session, pane))
})

// Brings the app forward and has it open the pane. A closed app starts on a
// link that does the same.
async function openPane(session, pane) {
  const known = typeof session === "string" && typeof pane === "string"
  const [client] = await clients.matchAll({ type: "window", includeUncontrolled: true })
  if (!client) {
    await clients.openWindow(known ? `/#pane=${encodeURIComponent(JSON.stringify([session, pane]))}` : "/")
    return
  }
  if (known) client.postMessage({ type: "yzc-open-pane", session, pane })
  await client.focus().catch(() => undefined)
}

self.addEventListener("fetch", (event) => {
  const request = event.request
  const url = new URL(request.url)
  if (request.method !== "GET" || url.origin !== self.location.origin) return
  // Authenticated API calls and the socket never touch the cache.
  if (url.pathname.startsWith("/api/") || url.pathname === "/ws") return
  if (request.mode === "navigate") {
    event.respondWith(navigate(request))
    return
  }
  if (PRECACHE.includes(url.pathname)) {
    event.respondWith(caches.match(url.pathname).then((hit) => hit ?? fetch(request)))
  }
})

// Network first, so a new build reaches the page; the cached shell covers an
// unreachable desktop and any refusal (an expired policy answers 403), and the
// app then explains the state itself.
async function navigate(request) {
  try {
    const response = await fetch(request)
    if (response.ok) return response
  } catch {
    // Offline or the desktop is unreachable.
  }
  return (await caches.match("/")) ?? Response.error()
}
