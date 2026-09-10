import { readImage } from "@tauri-apps/plugin-clipboard-manager"
import { invoke } from "@/lib/ipc"
import { requestHost } from "@/lib/hostIpc"
import { parseRuntimeScope, runtimeOwner } from "@/lib/herdrProvider"
import { LOCAL_HOST_ID, sameConnection } from "@/lib/runtimeIdentity"
import { useHostStore } from "@/state/hostStore"

const MAX_BYTES = 8 * 1024 * 1024
const MAX_PIXELS = 16 * 1024 * 1024

async function readPng(): Promise<Blob> {
  const image = await readImage()
  try {
    const { width, height } = await image.size()
    if (width < 1 || height < 1 || width * height > MAX_PIXELS) throw new Error("clipboard-image-too-large")
    const rgba = await image.rgba()
    const canvas = document.createElement("canvas")
    canvas.width = width
    canvas.height = height
    try {
      const context = canvas.getContext("2d")
      if (!context) throw new Error("clipboard-image-unavailable")
      context.putImageData(new ImageData(new Uint8ClampedArray(rgba), width, height), 0, 0)
      return await new Promise<Blob>((resolve, reject) => canvas.toBlob(
        (blob) => blob ? resolve(blob) : reject(new Error("clipboard-image-unavailable")), "image/png"
      ))
    } finally { canvas.width = 0; canvas.height = 0 }
  } finally { await image.close() }
}

export async function pasteTerminalClipboardImage(
  scope: string,
  isCurrent: () => boolean,
  paste: (text: string) => Promise<void>,
  suppliedImage?: Blob
): Promise<void> {
  if (!isCurrent()) return
  const { hostId } = parseRuntimeScope(scope)
  const owner = hostId === LOCAL_HOST_ID ? null : runtimeOwner(scope)
  const host = hostId === LOCAL_HOST_ID ? null : useHostStore.getState().hosts[hostId]?.connection
  if (hostId !== LOCAL_HOST_ID && (!owner || !host || !sameConnection(owner, host.owner)
    || !host.hello.methods.includes("clipboardImage"))) throw new Error("clipboard-image-host-unavailable")
  const current = () => isCurrent() && (!owner || (runtimeOwner(scope) != null && sameConnection(owner, runtimeOwner(scope)!)))
  const blob = suppliedImage ?? await readPng()
  if (!current()) return
  if (blob.type !== "image/png") throw new Error("clipboard-image-format")
  if (blob.size === 0 || blob.size > MAX_BYTES) throw new Error("clipboard-image-too-large")
  const bytes = new Uint8Array(await blob.arrayBuffer())
  let binary = ""
  for (let i = 0; i < bytes.length; i += 8192) binary += String.fromCharCode(...bytes.subarray(i, i + 8192))
  if (!current()) return
  const pngBase64 = btoa(binary)
  const path = owner
    ? await requestHost<string>(owner, { method: "clipboardImage", params: { png_base64: pngBase64 } })
    : await invoke<string>("terminal_clipboard_image", { pngBase64 })
  if (current()) await paste(path)
}
