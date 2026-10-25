import { beforeEach, describe, expect, it, vi } from "vitest"
import template from "../../companion/sw.js?raw"
import { paneFromMessage, takePaneLink } from "./lib/deepLink"

type Handler = (event: unknown) => void

// The service worker as the vite plugin ships it, run against a fake global scope.
const source = template.replace("\"__YZC_BUILD__\"", "\"test\"").replace("[\"__YZC_PRECACHE__\"]", "[\"/\"]")

function load(language: string) {
  const handlers = new Map<string, Handler>()
  const showNotification = vi.fn(async () => undefined)
  const client = { postMessage: vi.fn(), focus: vi.fn(async () => undefined) }
  const clients = { matchAll: vi.fn(async () => [client] as unknown[]), openWindow: vi.fn(async () => null) }
  const scope = {
    addEventListener: (type: string, handler: Handler) => handlers.set(type, handler),
    registration: { showNotification },
    navigator: { language },
    location: { origin: "https://yuzora.example" },
  }
  new Function("self", "caches", "clients", "fetch", "URL", "Response", source)(scope, {}, clients, vi.fn(), URL, Response)
  const waited: Promise<unknown>[] = []
  const waitUntil = (promise: Promise<unknown>) => waited.push(promise)
  return {
    client,
    clients,
    showNotification,
    push: async (data: unknown) => {
      handlers.get("push")!({ data: data === undefined ? null : { json: () => (typeof data === "string" ? JSON.parse(data) : data) }, waitUntil })
      await Promise.all(waited)
    },
    click: async (data: unknown) => {
      const notification = { close: vi.fn(), data }
      handlers.get("notificationclick")!({ notification, waitUntil })
      await Promise.all(waited)
      return notification
    },
  }
}

const blocked = { v: 1, kind: "blocked", title: "pi", space: "Yuzora", session: "work", pane: "w1:p" }

describe("push", () => {
  it("shows the agent, its Space and the state, one notification per pane", async () => {
    const worker = load("en-US")
    await worker.push(blocked)
    expect(worker.showNotification).toHaveBeenCalledWith("pi", {
      body: "Yuzora · Waiting for input",
      tag: "work\u0000w1:p",
      renotify: true,
      data: { session: "work", pane: "w1:p" },
    })
  })

  it("words a finished agent in Traditional Chinese for any Chinese browser", async () => {
    const worker = load("zh-Hant-HK")
    await worker.push({ ...blocked, kind: "done" })
    expect(worker.showNotification).toHaveBeenCalledWith("pi", expect.objectContaining({ body: "Yuzora · 已完成" }))
  })

  it("leaves the Space out when the push has none, and names the app when it has no title", async () => {
    const worker = load("en")
    await worker.push({ ...blocked, space: null, title: "" })
    expect(worker.showNotification).toHaveBeenCalledWith("Yuzora", expect.objectContaining({ body: "Waiting for input" }))
  })

  it("keeps the browser waiting for the notification", async () => {
    const worker = load("en")
    let shown = false
    worker.showNotification.mockImplementation(async () => {
      await Promise.resolve()
      shown = true
    })
    await worker.push(blocked)
    expect(shown).toBe(true)
  })

  it.each([
    ["no payload", undefined],
    ["a payload that is not JSON", "not json"],
    ["JSON that is not an object", "5"],
    ["JSON null", "null"],
    ["an unknown kind", { ...blocked, kind: "working" }],
    ["no session", { ...blocked, session: undefined }],
    ["an empty pane", { ...blocked, pane: "" }],
  ])("still shows a generic notification for %s", async (_, payload) => {
    const worker = load("en")
    await worker.push(payload)
    // Without a tag, `renotify` would make the browser refuse the notification.
    expect(worker.showNotification).toHaveBeenCalledExactlyOnceWith("Yuzora", { body: "New activity" })
  })
})

describe("notificationclick", () => {
  let worker: ReturnType<typeof load>
  beforeEach(() => {
    worker = load("en")
  })

  it("closes the notification, tells a running app which pane and brings it forward", async () => {
    const notification = await worker.click({ session: "work", pane: "w1:p" })
    expect(notification.close).toHaveBeenCalled()
    expect(worker.clients.matchAll).toHaveBeenCalledWith({ type: "window", includeUncontrolled: true })
    expect(worker.client.postMessage).toHaveBeenCalledWith({ type: "yzc-open-pane", session: "work", pane: "w1:p" })
    expect(worker.client.focus).toHaveBeenCalled()
    expect(worker.clients.openWindow).not.toHaveBeenCalled()
    expect(paneFromMessage(worker.client.postMessage.mock.calls[0][0])).toEqual({ session: "work", pane: "w1:p" })
  })

  it("starts a closed app on a link the boot path reads back", async () => {
    worker.clients.matchAll.mockResolvedValue([])
    await worker.click({ session: "wörk", pane: "w1:p&x=1#" })
    const [url] = worker.clients.openWindow.mock.calls[0] as unknown as [string]
    expect(url).toBe("/#pane=%5B%22w%C3%B6rk%22%2C%22w1%3Ap%26x%3D1%23%22%5D")
    const replaceState = vi.fn()
    expect(takePaneLink({ hash: new URL(url, "https://yuzora.example").hash }, { replaceState })).toEqual({ session: "wörk", pane: "w1:p&x=1#" })
    expect(replaceState).toHaveBeenCalledOnce()
  })

  it("still brings the app forward when the browser won't focus it", async () => {
    worker.client.focus.mockRejectedValue(new Error("not allowed"))
    await worker.click({ session: "work", pane: "w1:p" })
    expect(worker.client.postMessage).toHaveBeenCalled()
  })

  it("only brings the app forward for a notification that names no pane", async () => {
    await worker.click(null)
    expect(worker.client.postMessage).not.toHaveBeenCalled()
    expect(worker.client.focus).toHaveBeenCalled()
    worker.clients.matchAll.mockResolvedValue([])
    await worker.click({ session: 5 })
    expect(worker.clients.openWindow).toHaveBeenCalledWith("/")
  })
})
