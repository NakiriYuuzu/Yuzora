import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { I18nextProvider } from "react-i18next"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { i18n } from "../i18n"
import { toBase64Url } from "../lib/base64url"
import { RpcError } from "../lib/rpc"
import { useUpdate } from "../serviceWorker"

const client = vi.hoisted(() => ({ call: vi.fn() }))
vi.mock("../client", () => ({ call: client.call }))
import { disablePush, enablePush, pushState } from "../push"
import { PushControl } from "./PushControl"

// A P-256 point: 0x04, then 64 bytes.
const keyBytes = Uint8Array.from([4, ...Array.from({ length: 64 }, (_, index) => index + 1)])
const vapid = toBase64Url(keyBytes)
// What the desktop used before its key was replaced.
const oldKeyBytes = Uint8Array.from(keyBytes, (byte, index) => (index === 64 ? byte ^ 0xff : byte))
const IPHONE = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 Mobile/15E148"

function subscriptionStub(key: Uint8Array = keyBytes, endpoint = "https://web.push.apple.com/abc") {
  return {
    endpoint,
    options: { applicationServerKey: key.buffer },
    unsubscribe: vi.fn(async () => true),
    toJSON: () => ({ endpoint, keys: { p256dh: "p256dh-key", auth: "auth-secret" } }),
  }
}

// A worker that is downloading; `settle` is the browser moving it on.
function installingStub() {
  const listeners = new Set<() => void>()
  const worker = {
    state: "installing",
    postMessage: vi.fn(),
    listeners,
    addEventListener: vi.fn((type: string, listener: () => void) => void (type === "statechange" && listeners.add(listener))),
    removeEventListener: vi.fn((type: string, listener: () => void) => void (type === "statechange" && listeners.delete(listener))),
    settle(state: string) {
      worker.state = state
      for (const listener of [...listeners]) listener()
    },
  }
  return worker
}

// What the browser offers; each test changes only what it is about.
let permission: NotificationPermission
let existing: ReturnType<typeof subscriptionStub> | null
let gatewaySubscribed: boolean
let waitingWorker: { postMessage: ReturnType<typeof vi.fn> } | null
let installingWorker: ReturnType<typeof installingStub> | null
const requestPermission = vi.fn()
const subscribe = vi.fn()
const getSubscription = vi.fn()
const getRegistration = vi.fn()

beforeEach(async () => {
  await i18n.changeLanguage("en")
  permission = "default"
  existing = null
  gatewaySubscribed = false
  waitingWorker = null
  installingWorker = null
  useUpdate.setState({ waiting: null })
  client.call.mockReset()
  client.call.mockImplementation(async (method: string) => (method === "push.status" ? { subscribed: gatewaySubscribed } : { subscribed: true }))
  requestPermission.mockReset().mockImplementation(async () => {
    permission = "granted"
    return permission
  })
  subscribe.mockReset().mockImplementation(async () => subscriptionStub())
  getSubscription.mockReset().mockImplementation(async () => existing)
  getRegistration.mockReset().mockImplementation(async () => ({ active: {}, waiting: waitingWorker, installing: installingWorker, pushManager: { subscribe, getSubscription } }))
  vi.stubGlobal("PushManager", class {})
  vi.stubGlobal("Notification", { get permission() { return permission }, requestPermission })
  Object.defineProperty(navigator, "serviceWorker", { value: { getRegistration }, configurable: true })
  render(<I18nextProvider i18n={i18n}><PushControl vapidPublicKey={vapid} /></I18nextProvider>)
})
afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
  Reflect.deleteProperty(navigator, "serviceWorker")
  useUpdate.setState({ waiting: null })
})

const openDialog = async (message: RegExp) => {
  fireEvent.click(screen.getByRole("button", { name: "Notifications" }))
  expect(await screen.findByText(message)).toBeInTheDocument()
}

it("asks nothing of the browser until the tap on Turn on", async () => {
  await openDialog(/Get a notification when an agent needs/)
  expect(requestPermission).not.toHaveBeenCalled()
  expect(subscribe).not.toHaveBeenCalled()
  // Nothing is subscribed in this browser: the desktop is asked about no endpoint.
  expect(client.call).toHaveBeenCalledExactlyOnceWith("push.status", { endpoint: null })
})

it("asks permission straight from the tap, subscribes with the desktop's key and hands the subscription over", async () => {
  await openDialog(/Get a notification/)
  fireEvent.click(screen.getByRole("button", { name: "Turn on" }))
  // Nothing may be awaited before the prompt: Safari needs the gesture.
  expect(requestPermission).toHaveBeenCalledOnce()
  expect(await screen.findByText("Notifications are on for this phone.")).toBeInTheDocument()
  expect(subscribe).toHaveBeenCalledOnce()
  const options = subscribe.mock.calls[0][0] as { userVisibleOnly: boolean; applicationServerKey: Uint8Array }
  expect(options.userVisibleOnly).toBe(true)
  expect([...options.applicationServerKey]).toEqual([...keyBytes])
  expect(client.call).toHaveBeenLastCalledWith("push.subscribe", { endpoint: "https://web.push.apple.com/abc", keys: { p256dh: "p256dh-key", auth: "auth-secret" } })
  expect(screen.getByRole("button", { name: "Turn off" })).toBeInTheDocument()
})

it("skips the prompt when the permission is already granted, and replaces a subscription it can't vouch for", async () => {
  permission = "granted"
  existing = subscriptionStub()
  const stale = existing
  await openDialog(/Get a notification/)
  fireEvent.click(screen.getByRole("button", { name: "Turn on" }))
  expect(await screen.findByText("Notifications are on for this phone.")).toBeInTheDocument()
  expect(requestPermission).not.toHaveBeenCalled()
  expect(stale.unsubscribe).toHaveBeenCalledOnce()
  expect(subscribe).toHaveBeenCalledOnce()
})

it("subscribes nothing when the prompt is dismissed or refused", async () => {
  requestPermission.mockImplementation(async () => "default")
  await openDialog(/Get a notification/)
  fireEvent.click(screen.getByRole("button", { name: "Turn on" }))
  await waitFor(() => expect(screen.getByRole("button", { name: "Turn on" })).toBeEnabled())
  expect(subscribe).not.toHaveBeenCalled()

  requestPermission.mockImplementation(async () => {
    permission = "denied"
    return permission
  })
  fireEvent.click(screen.getByRole("button", { name: "Turn on" }))
  expect(await screen.findByText(/Notifications are blocked for Yuzora/)).toBeInTheDocument()
  expect(subscribe).not.toHaveBeenCalled()
  expect(client.call).not.toHaveBeenCalledWith("push.subscribe", expect.anything())
})

it("explains a blocked permission without prompting", async () => {
  permission = "denied"
  await openDialog(/Notifications are blocked for Yuzora/)
  expect(screen.queryByRole("button", { name: "Turn on" })).toBeNull()
  expect(client.call).not.toHaveBeenCalled()
})

it("shows the desktop's refusal in words and stays off", async () => {
  client.call.mockImplementation(async (method: string) => {
    if (method === "push.subscribe") throw new RpcError("push-unavailable")
    return { subscribed: false }
  })
  await openDialog(/Get a notification/)
  fireEvent.click(screen.getByRole("button", { name: "Turn on" }))
  expect(await screen.findByText("The desktop can't send notifications right now.")).toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Turn on" })).toBeEnabled()
})

it.each([
  ["push-store-failed", "The desktop could not save this notification setting. Try again."],
  ["push-unconfirmed", "The desktop did not confirm the notification setting in time. Try again."],
])("stays off and says why when the desktop answers %s to the subscribe", async (code, message) => {
  client.call.mockImplementation(async (method: string) => {
    if (method === "push.subscribe") throw new RpcError(code)
    return { subscribed: false }
  })
  await openDialog(/Get a notification/)
  fireEvent.click(screen.getByRole("button", { name: "Turn on" }))
  expect(await screen.findByText(message)).toBeInTheDocument()
  expect(screen.queryByText("Notifications are on for this phone.")).toBeNull()
  expect(screen.getByRole("button", { name: "Turn on" })).toBeEnabled()
  expect(screen.queryByRole("button", { name: "Turn off" })).toBeNull()
})

it("says on only once the desktop's answer to the subscribe has come", async () => {
  let confirm: (value: unknown) => void = () => {}
  client.call.mockImplementation((method: string) =>
    method === "push.subscribe" ? new Promise((resolve) => { confirm = resolve }) : Promise.resolve({ subscribed: false }),
  )
  await openDialog(/Get a notification/)
  fireEvent.click(screen.getByRole("button", { name: "Turn on" }))
  await waitFor(() => expect(client.call).toHaveBeenCalledWith("push.subscribe", expect.anything()))
  expect(screen.queryByText("Notifications are on for this phone.")).toBeNull()
  confirm({ subscribed: true })
  expect(await screen.findByText("Notifications are on for this phone.")).toBeInTheDocument()
})

it("reports a browser failure generically", async () => {
  subscribe.mockRejectedValue(new Error("push service error"))
  await openDialog(/Get a notification/)
  fireEvent.click(screen.getByRole("button", { name: "Turn on" }))
  expect(await screen.findByText("Notifications could not be changed. Try again.")).toBeInTheDocument()
  expect(client.call).not.toHaveBeenCalledWith("push.subscribe", expect.anything())
})

it("is on only when the desktop and the browser both hold the subscription", async () => {
  gatewaySubscribed = true
  await openDialog(/Get a notification/)
  cleanup()

  existing = subscriptionStub()
  render(<I18nextProvider i18n={i18n}><PushControl vapidPublicKey={vapid} /></I18nextProvider>)
  await openDialog(/Notifications are on for this phone/)
})

it("asks the desktop about the endpoint of this browser's own subscription", async () => {
  gatewaySubscribed = true
  existing = subscriptionStub(keyBytes, "https://web.push.apple.com/mine")
  await openDialog(/Notifications are on for this phone/)
  expect(client.call).toHaveBeenCalledExactlyOnceWith("push.status", { endpoint: "https://web.push.apple.com/mine" })
})

it("turns off at the desktop first, then in the browser", async () => {
  gatewaySubscribed = true
  existing = subscriptionStub()
  const local = existing
  await openDialog(/Notifications are on for this phone/)
  fireEvent.click(screen.getByRole("button", { name: "Turn off" }))
  expect(await screen.findByText(/Get a notification/)).toBeInTheDocument()
  expect(client.call).toHaveBeenLastCalledWith("push.unsubscribe", { endpoint: "https://web.push.apple.com/abc" })
  expect(local.unsubscribe).toHaveBeenCalledOnce()
})

it("stays on when the desktop can't be told", async () => {
  gatewaySubscribed = true
  existing = subscriptionStub()
  const local = existing
  await openDialog(/Notifications are on for this phone/)
  client.call.mockRejectedValue(new RpcError("outcome-unknown"))
  fireEvent.click(screen.getByRole("button", { name: "Turn off" }))
  expect(await screen.findByRole("alert")).toBeInTheDocument()
  expect(local.unsubscribe).not.toHaveBeenCalled()
  expect(screen.getByRole("button", { name: "Turn off" })).toBeEnabled()
})

it("says so when this browser has no push", async () => {
  Reflect.deleteProperty(window, "PushManager")
  await openDialog(/can't receive notifications from Yuzora/)
  expect(screen.queryByRole("button", { name: "Turn on" })).toBeNull()
})

it("says so when no service worker is active yet", async () => {
  getRegistration.mockResolvedValue({ active: null })
  await openDialog(/can't receive notifications from Yuzora/)
  expect(client.call).not.toHaveBeenCalled()
})

it("sends an iPhone browser tab to the Home Screen first", async () => {
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(IPHONE)
  // Safari hides the API outside an installed web app.
  Reflect.deleteProperty(window, "PushManager")
  await openDialog(/add Yuzora to your Home Screen first/)
  expect(screen.queryByRole("button", { name: "Turn on" })).toBeNull()
})

it("offers an installed iPhone app the same switch", async () => {
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(IPHONE)
  vi.spyOn(window, "matchMedia").mockReturnValue({ matches: true } as MediaQueryList)
  await openDialog(/Get a notification/)
  expect(screen.getByRole("button", { name: "Turn on" })).toBeInTheDocument()
})

it("is off when this phone's subscription was made for another key: the desktop's key was replaced", async () => {
  gatewaySubscribed = true
  existing = subscriptionStub(oldKeyBytes)
  const stale = existing
  await openDialog(/Get a notification/)
  expect(screen.queryByRole("button", { name: "Turn off" })).toBeNull()

  // Turning on starts over with the desktop's current key.
  fireEvent.click(screen.getByRole("button", { name: "Turn on" }))
  expect(await screen.findByText("Notifications are on for this phone.")).toBeInTheDocument()
  expect(stale.unsubscribe).toHaveBeenCalledOnce()
  const options = subscribe.mock.calls[0][0] as { applicationServerKey: Uint8Array }
  expect([...options.applicationServerKey]).toEqual([...keyBytes])
  expect(client.call).toHaveBeenLastCalledWith("push.subscribe", expect.objectContaining({ endpoint: "https://web.push.apple.com/abc" }))
})

it("is off when the browser can't say which key its subscription uses", async () => {
  gatewaySubscribed = true
  existing = { ...subscriptionStub(), options: { applicationServerKey: null as unknown as ArrayBuffer } }
  await openDialog(/Get a notification/)
  expect(screen.queryByRole("button", { name: "Turn off" })).toBeNull()
})

it("asks for the update first while a new worker waits, and subscribes nothing", async () => {
  waitingWorker = { postMessage: vi.fn() }
  await openDialog(/Reload first, then turn on notifications/)
  expect(screen.queryByRole("button", { name: "Turn on" })).toBeNull()
  expect(requestPermission).not.toHaveBeenCalled()
  expect(subscribe).not.toHaveBeenCalled()

  // The button is the banner's own update action: the waiting worker takes over.
  fireEvent.click(screen.getByRole("button", { name: "Reload" }))
  expect(waitingWorker.postMessage).toHaveBeenCalledExactlyOnceWith("yzc-skip-waiting")
})

it("asks for the update first when the page already knows of a waiting worker", async () => {
  const worker = { postMessage: vi.fn() }
  useUpdate.setState({ waiting: worker as unknown as ServiceWorker })
  await openDialog(/Reload first, then turn on notifications/)
  expect(screen.queryByRole("button", { name: "Turn on" })).toBeNull()
  fireEvent.click(screen.getByRole("button", { name: "Reload" }))
  expect(worker.postMessage).toHaveBeenCalledExactlyOnceWith("yzc-skip-waiting")
})

it("does not subscribe against the old worker when an update arrives while the dialog is open", async () => {
  await openDialog(/Get a notification/)
  useUpdate.setState({ waiting: { postMessage: vi.fn() } as unknown as ServiceWorker })
  fireEvent.click(screen.getByRole("button", { name: "Turn on" }))
  expect(await screen.findByText(/Reload first, then turn on notifications/)).toBeInTheDocument()
  expect(requestPermission).not.toHaveBeenCalled()
  expect(subscribe).not.toHaveBeenCalled()
  expect(client.call).not.toHaveBeenCalledWith("push.subscribe", expect.anything())
})

it("does not subscribe when the registration holds a waiting worker the page never heard of", async () => {
  permission = "granted"
  waitingWorker = { postMessage: vi.fn() }
  expect(await enablePush(vapid)).toBe("update-first")
  expect(subscribe).not.toHaveBeenCalled()
  expect(client.call).not.toHaveBeenCalledWith("push.subscribe", expect.anything())
})

it("keeps Turn off available while an update waits", async () => {
  gatewaySubscribed = true
  existing = subscriptionStub()
  waitingWorker = { postMessage: vi.fn() }
  await openDialog(/Notifications are on for this phone/)
  expect(screen.getByRole("button", { name: "Turn off" })).toBeInTheDocument()
})

describe("a new worker that is still installing", () => {
  // The old active worker has no push handler: nothing may subscribe against it.
  it("asks to wait while it downloads, and offers neither Turn on nor Reload", async () => {
    installingWorker = installingStub()
    await openDialog(/Downloading a new version of Yuzora Companion/)
    expect(screen.queryByRole("button", { name: "Turn on" })).toBeNull()
    expect(screen.queryByRole("button", { name: "Reload" })).toBeNull()
    expect(requestPermission).not.toHaveBeenCalled()
    expect(subscribe).not.toHaveBeenCalled()
  })

  it("keeps Turn off available when notifications are already on", async () => {
    gatewaySubscribed = true
    existing = subscriptionStub()
    installingWorker = installingStub()
    await openDialog(/Notifications are on for this phone/)
    expect(screen.getByRole("button", { name: "Turn off" })).toBeInTheDocument()
    expect(installingWorker.addEventListener).not.toHaveBeenCalled()
  })

  it("is not an update on the first install, when no worker is active yet", async () => {
    installingWorker = installingStub()
    getRegistration.mockImplementation(async () => ({ active: null, installing: installingWorker }))
    await openDialog(/can't receive notifications from Yuzora/)
    expect(installingWorker.addEventListener).not.toHaveBeenCalled()
  })

  it("moves to update-first once it is installed, and Reload then takes the worker over", async () => {
    const worker = installingWorker = installingStub()
    await openDialog(/Downloading a new version/)
    await waitFor(() => expect(worker.listeners.size).toBe(1))

    // The browser moves the worker from installing to waiting, then says so.
    installingWorker = null
    waitingWorker = worker
    worker.settle("installed")
    expect(await screen.findByText(/Reload first, then turn on notifications/)).toBeInTheDocument()
    expect(worker.listeners.size).toBe(0)
    expect(screen.queryByRole("button", { name: "Turn on" })).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "Reload" }))
    expect(worker.postMessage).toHaveBeenCalledExactlyOnceWith("yzc-skip-waiting")
  })

  it("goes back to off when it turns out redundant", async () => {
    const worker = installingWorker = installingStub()
    await openDialog(/Downloading a new version/)
    await waitFor(() => expect(worker.listeners.size).toBe(1))

    installingWorker = null
    worker.settle("redundant")
    expect(await screen.findByText(/Get a notification/)).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Turn on" })).toBeEnabled()
    expect(worker.listeners.size).toBe(0)
  })

  it("reads everything again when it activates", async () => {
    const worker = installingWorker = installingStub()
    await openDialog(/Downloading a new version/)
    await waitFor(() => expect(worker.listeners.size).toBe(1))

    // It took over and this phone's subscription is already there.
    installingWorker = null
    gatewaySubscribed = true
    existing = subscriptionStub()
    worker.settle("activated")
    expect(await screen.findByText("Notifications are on for this phone.")).toBeInTheDocument()
  })

  it("follows the next worker when another one replaces it", async () => {
    const first = installingWorker = installingStub()
    await openDialog(/Downloading a new version/)
    await waitFor(() => expect(first.listeners.size).toBe(1))

    const second = installingWorker = installingStub()
    first.settle("redundant")
    await waitFor(() => expect(second.listeners.size).toBe(1))
    expect(first.listeners.size).toBe(0)
    expect(screen.getByText(/Downloading a new version/)).toBeInTheDocument()

    installingWorker = null
    waitingWorker = second
    second.settle("installed")
    expect(await screen.findByText(/Reload first, then turn on notifications/)).toBeInTheDocument()
  })

  it("shows the latest read when an earlier one finishes late", async () => {
    const worker = installingWorker = installingStub()
    await openDialog(/Downloading a new version/)
    await waitFor(() => expect(worker.listeners.size).toBe(1))

    // The first read after this is held back at the desktop.
    let release: () => void = () => undefined
    let statusReads = 0
    client.call.mockImplementation(async (method: string) => {
      if (method === "push.status" && ++statusReads === 1) await new Promise<void>((resolve) => { release = resolve })
      return { subscribed: false }
    })
    installingWorker = null
    waitingWorker = worker
    worker.settle("installed")
    // The next state change comes before it answers, and the worker is not waiting any more.
    waitingWorker = null
    worker.settle("activating")
    expect(await screen.findByText(/Get a notification/)).toBeInTheDocument()

    release()
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(screen.queryByText(/Reload first, then turn on notifications/)).toBeNull()
    expect(screen.getByText(/Get a notification/)).toBeInTheDocument()
  })

  it("reads again at once when the worker settled before it could be watched", async () => {
    const worker = installingStub()
    // The dialog sees the worker installing; by the time it is watched, it is gone.
    getRegistration.mockReset()
      .mockImplementationOnce(async () => ({ active: {}, installing: worker, pushManager: { subscribe, getSubscription } }))
      .mockImplementation(async () => ({ active: {}, installing: null, pushManager: { subscribe, getSubscription } }))
    await openDialog(/Get a notification/)
    expect(worker.addEventListener).not.toHaveBeenCalled()
  })

  it("drops its listener when the dialog closes, and reads nothing afterwards", async () => {
    const worker = installingWorker = installingStub()
    await openDialog(/Downloading a new version/)
    await waitFor(() => expect(worker.listeners.size).toBe(1))

    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }))
    expect(screen.queryByText(/Downloading a new version/)).toBeNull()
    expect(worker.removeEventListener).toHaveBeenCalledOnce()
    expect(worker.listeners.size).toBe(0)

    const reads = getRegistration.mock.calls.length
    worker.settle("installed")
    expect(getRegistration).toHaveBeenCalledTimes(reads)
  })

  it("attaches nothing when the dialog closes before the worker could be watched", async () => {
    const worker = installingStub()
    let release: (registration: unknown) => void = () => undefined
    getRegistration.mockReset()
      .mockImplementationOnce(async () => ({ active: {}, installing: worker, pushManager: { subscribe, getSubscription } }))
      .mockImplementationOnce(() => new Promise((resolve) => { release = resolve }))
    await openDialog(/Downloading a new version/)
    await waitFor(() => expect(getRegistration).toHaveBeenCalledTimes(2))

    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }))
    release({ active: {}, installing: worker })
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(worker.addEventListener).not.toHaveBeenCalled()
  })

  it("is found by the tap on Turn on when it began after the dialog read the state, and followed from there", async () => {
    await openDialog(/Get a notification/)
    const worker = installingWorker = installingStub()
    fireEvent.click(screen.getByRole("button", { name: "Turn on" }))
    expect(await screen.findByText(/Downloading a new version/)).toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Turn on" })).toBeNull()
    expect(subscribe).not.toHaveBeenCalled()
    expect(client.call).not.toHaveBeenCalledWith("push.subscribe", expect.anything())
    await waitFor(() => expect(worker.listeners.size).toBe(1))

    installingWorker = null
    waitingWorker = worker
    worker.settle("installed")
    expect(await screen.findByText(/Reload first, then turn on notifications/)).toBeInTheDocument()
  })

  it("does not subscribe against the old worker, and asks nothing more of the browser than the tap did", async () => {
    installingWorker = installingStub()
    expect(await enablePush(vapid)).toBe("updating")
    // The prompt belongs to the tap and stays ahead of anything awaited.
    expect(requestPermission).toHaveBeenCalledOnce()
    expect(subscribe).not.toHaveBeenCalled()
    expect(client.call).not.toHaveBeenCalledWith("push.subscribe", expect.anything())

    requestPermission.mockClear()
    expect(await enablePush(vapid)).toBe("updating")
    expect(requestPermission).not.toHaveBeenCalled()
    expect(subscribe).not.toHaveBeenCalled()
  })
})

// Synced passkeys sign several browsers in as one device; the desktop keeps an endpoint per browser.
describe("two browsers of one device", () => {
  const ENDPOINT = { iphone: "https://web.push.apple.com/IPHONE", ipad: "https://web.push.apple.com/IPAD" }
  type Browser = keyof typeof ENDPOINT
  let browser: Browser
  let local: Record<Browser, ReturnType<typeof subscriptionStub> | null>
  let held: Set<string>

  beforeEach(() => {
    permission = "granted"
    browser = "iphone"
    local = { iphone: null, ipad: null }
    held = new Set()
    getSubscription.mockImplementation(async () => local[browser])
    subscribe.mockImplementation(async () => (local[browser] = subscriptionStub(keyBytes, ENDPOINT[browser])))
    // The desktop of the contract: `subscribed` is true only for an endpoint it holds for the device.
    client.call.mockImplementation(async (method: string, params: { endpoint: string | null }) => {
      if (method === "push.status") return { available: true, subscribed: params.endpoint !== null && held.has(params.endpoint) }
      if (method === "push.subscribe" && params.endpoint) held.add(params.endpoint)
      if (method === "push.unsubscribe" && params.endpoint) held.delete(params.endpoint)
      return { subscribed: method === "push.subscribe" }
    })
  })

  it("reports each browser's own state", async () => {
    expect(await enablePush(vapid)).toBe("on")
    // The other browser has nothing of its own yet, although the device is subscribed.
    browser = "ipad"
    expect(await pushState(vapid)).toBe("off")
    expect(await enablePush(vapid)).toBe("on")
    expect([...held]).toEqual([ENDPOINT.iphone, ENDPOINT.ipad])
    browser = "iphone"
    expect(await pushState(vapid)).toBe("on")

    // The desktop lost the first browser's endpoint: only that browser is off.
    held.delete(ENDPOINT.iphone)
    expect(await pushState(vapid)).toBe("off")
    browser = "ipad"
    expect(await pushState(vapid)).toBe("on")
  })

  it("turning one off leaves the other's subscription", async () => {
    await enablePush(vapid)
    browser = "ipad"
    await enablePush(vapid)
    const [iphone, ipad] = [local.iphone!, local.ipad!]

    browser = "iphone"
    expect(await disablePush()).toBe("off")
    expect(client.call).toHaveBeenLastCalledWith("push.unsubscribe", { endpoint: ENDPOINT.iphone })
    expect([...held]).toEqual([ENDPOINT.ipad])
    expect(iphone.unsubscribe).toHaveBeenCalledOnce()
    expect(ipad.unsubscribe).not.toHaveBeenCalled()

    browser = "ipad"
    expect(await pushState(vapid)).toBe("on")
  })

  it("has nothing to tell the desktop when this browser holds no subscription", async () => {
    await enablePush(vapid)
    browser = "ipad"
    client.call.mockClear()
    expect(await disablePush()).toBe("off")
    expect(client.call).not.toHaveBeenCalled()
    expect([...held]).toEqual([ENDPOINT.iphone])
  })
})
