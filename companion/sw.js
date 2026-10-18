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
