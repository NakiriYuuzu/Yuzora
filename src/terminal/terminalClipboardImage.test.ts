import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { readImage } from "@tauri-apps/plugin-clipboard-manager"
import { invoke } from "@/lib/ipc"
import { requestHost } from "@/lib/hostIpc"
import { registerRuntimeHost, unregisterRuntimeHost } from "@/lib/herdrProvider"
import { useHostStore } from "@/state/hostStore"
import { pasteTerminalClipboardImage } from "./terminalClipboardImage"

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ readImage: vi.fn() }))
vi.mock("@/lib/ipc", () => ({ invoke: vi.fn() }))
vi.mock("@/lib/hostIpc", async (original) => ({ ...await original<object>(), requestHost: vi.fn() }))

const pngBase64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a0ioAAAAASUVORK5CYII="
const png = () => {
  const bytes = Uint8Array.from(atob(pngBase64), (value) => value.charCodeAt(0))
  const blob = new Blob([bytes], { type: "image/png" })
  // jsdom has Blob metadata but omits the browser arrayBuffer API.
  Object.defineProperty(blob, "arrayBuffer", { value: async () => bytes.buffer })
  return blob
}
const owner = { hostId: "clipboard-wsl", generation: 1 }
const scope = JSON.stringify([owner.hostId, "default"])
function connect(generation = 1, methods = ["clipboardImage"]) {
  const connection = { owner: { ...owner, generation }, hello: { protocol: 1, version: "fixture", os: "linux", arch: "x64", home: "/home/test", methods } }
  registerRuntimeHost(connection, "/herdr", "WSL fixture", "wsl")
  useHostStore.setState({ hosts: { [owner.hostId]: { connection, connecting: false, error: null, target: { kind: "wsl", distro: "Fixture" }, attempt: 0, retryAt: 0 } } })
}
beforeEach(() => { vi.mocked(invoke).mockResolvedValue("/tmp/local.png"); vi.mocked(requestHost).mockResolvedValue("/tmp/remote.png") })
afterEach(() => {
  unregisterRuntimeHost(owner)
  unregisterRuntimeHost({ ...owner, generation: 2 })
  useHostStore.setState({ hosts: {} })
  vi.resetAllMocks(); vi.restoreAllMocks(); vi.unstubAllGlobals()
})

it("stages a PNG on the local host and pastes its path once without Enter", async () => {
  const paste = vi.fn().mockResolvedValue(undefined)
  await pasteTerminalClipboardImage("default", () => true, paste, png())
  expect(invoke).toHaveBeenCalledExactlyOnceWith("terminal_clipboard_image", { pngBase64 })
  expect(paste).toHaveBeenCalledExactlyOnceWith("/tmp/local.png")
  expect(requestHost).not.toHaveBeenCalled()
})

it("stages on the matching WSL host, never the desktop filesystem", async () => {
  connect()
  const paste = vi.fn().mockResolvedValue(undefined)
  await pasteTerminalClipboardImage(scope, () => true, paste, png())
  expect(requestHost).toHaveBeenCalledExactlyOnceWith(owner, { method: "clipboardImage", params: { png_base64: pngBase64 } })
  expect(paste).toHaveBeenCalledExactlyOnceWith("/tmp/remote.png")
  expect(invoke).not.toHaveBeenCalled()
})

it("rejects unsupported helpers before reading the clipboard", async () => {
  connect(1, [])
  await expect(pasteTerminalClipboardImage(scope, () => true, vi.fn())).rejects.toThrow("host-unavailable")
  expect(readImage).not.toHaveBeenCalled()
})

it.each(["switch", "reconnect"])("drops an image if %s occurs while the helper stages it", async (action) => {
  connect()
  let current = true
  vi.mocked(requestHost).mockImplementationOnce(async () => {
    if (action === "switch") current = false
    else connect(2)
    return "/tmp/old.png"
  })
  const paste = vi.fn()
  await pasteTerminalClipboardImage(scope, () => current, paste, png())
  expect(paste).not.toHaveBeenCalled()
})

it("rejects oversize or non-PNG blobs before staging", async () => {
  const paste = vi.fn()
  for (const blob of [new Blob(["text"], { type: "text/plain" }), new Blob([new Uint8Array(8 * 1024 * 1024 + 1)], { type: "image/png" })]) {
    await expect(pasteTerminalClipboardImage("default", () => true, paste, blob as unknown as Blob)).rejects.toThrow("clipboard-image")
  }
  expect(invoke).not.toHaveBeenCalled()
  expect(paste).not.toHaveBeenCalled()
})

it.each(["success", "too-large", "read-error"])("closes the native image resource on %s", async (outcome) => {
  const resource = { size: vi.fn().mockResolvedValue(outcome === "too-large" ? { width: 8192, height: 8192 } : { width: 1, height: 1 }), rgba: vi.fn().mockResolvedValue(new Uint8Array([255, 255, 255, 255])), close: vi.fn().mockResolvedValue(undefined) }
  if (outcome === "read-error") resource.rgba.mockRejectedValue(new Error("read-error"))
  vi.mocked(readImage).mockResolvedValue(resource as never)
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({ putImageData: vi.fn() } as never)
  vi.spyOn(HTMLCanvasElement.prototype, "toBlob").mockImplementation((callback) => callback(png()))
  vi.stubGlobal("ImageData", class { constructor(public data: Uint8ClampedArray, public width: number, public height: number) {} })
  const paste = vi.fn().mockResolvedValue(undefined)
  const result = pasteTerminalClipboardImage("default", () => true, paste)
  if (outcome === "success") { await result; expect(paste).toHaveBeenCalledOnce() }
  else { await expect(result).rejects.toThrow(); expect(paste).not.toHaveBeenCalled() }
  expect(resource.close).toHaveBeenCalledOnce()
  if (outcome === "too-large") expect(resource.rgba).not.toHaveBeenCalled()
})
