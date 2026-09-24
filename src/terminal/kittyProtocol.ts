// HERDR emits inline Kitty RGB/RGBA/PNG uploads and explicit screen placements.
// Keep framing independent of xterm: xterm does not expose an APC handler.
export type KittyToken = { type: "text"; text: string } | { type: "graphics"; control: Record<string, string>; payload: string }
export const MAX_KITTY_BYTES = 64 * 1024 * 1024
const MAX_SEQUENCE = 64 * 1024

export class KittyStreamParser {
  private pending = ""
  feed(text: string): KittyToken[] {
    this.pending += text
    const tokens: KittyToken[] = []
    while (this.pending) {
      const start = this.pending.indexOf("\x1b_G")
      if (start < 0) {
        const keep = this.pending.endsWith("\x1b_") ? 2 : this.pending.endsWith("\x1b") ? 1 : 0
        const body = this.pending.slice(0, this.pending.length - keep)
        if (body) tokens.push({ type: "text", text: body })
        this.pending = keep ? this.pending.slice(-keep) : ""
        break
      }
      if (start > 0) tokens.push({ type: "text", text: this.pending.slice(0, start) })
      this.pending = this.pending.slice(start)
      const end = this.pending.indexOf("\x1b\\", 3)
      if ((end < 0 ? this.pending.length : end) > MAX_SEQUENCE) throw new Error("Kitty sequence exceeds the inline chunk limit")
      if (end < 0) break
      const sequence = this.pending.slice(3, end)
      const delimiter = sequence.indexOf(";")
      const header = delimiter < 0 ? sequence : sequence.slice(0, delimiter)
      const control: Record<string, string> = {}
      for (const field of header.split(",")) {
        const [key, value] = field.split("=")
        if (/^[A-Za-z]$/.test(key) && value !== undefined) control[key] = value
      }
      tokens.push({ type: "graphics", control, payload: delimiter < 0 ? "" : sequence.slice(delimiter + 1) })
      this.pending = this.pending.slice(end + 2)
    }
    return tokens
  }
}

export interface KittyImage<T> { image: T; width: number; height: number }
export interface KittyPlacement { imageId: number; id: number; col: number; row: number; x: number; y: number; width: number; height: number; offsetX: number; offsetY: number; cols: number; rows: number; z: number }
interface GraphicsTarget<T> {
  load: (bytes: Uint8Array<ArrayBuffer>, width: number, height: number, format: number) => Promise<KittyImage<T>>
  dispose: (image: T) => void
  cursor: () => { col: number; row: number }
  reply: (data: string) => void
  changed: () => void
}

function integer(value: string | undefined, fallback = 0): number {
  if (value === undefined) return fallback
  if (!/^-?\d+$/.test(value) || !Number.isSafeInteger(Number(value))) throw new Error("EINVAL: Invalid graphics integer")
  return Number(value)
}

// Kitty client IDs are 32-bit; anonymous uploads/placements use IDs above that range so they never replace a client's image.
const AUTO_ID_BASE = 0x1_0000_0000
function clientId(value: string | undefined): number {
  const id = integer(value)
  if (id < 0 || id >= AUTO_ID_BASE) throw new Error("EINVAL: Invalid graphics id")
  return id
}

async function inflate(bytes: Uint8Array<ArrayBuffer>): Promise<Uint8Array<ArrayBuffer>> {
  const source = new ReadableStream<Uint8Array<ArrayBuffer>>({ start(controller) { controller.enqueue(bytes); controller.close() } })
  const reader = source.pipeThrough(new DecompressionStream("deflate")).getReader()
  const chunks: Uint8Array[] = []
  let size = 0
  try {
    for (;;) {
      const { done, value } = await reader.read()
      if (done) break
      size += value.length
      if (size > MAX_KITTY_BYTES) throw new Error("EFBIG: Image exceeds memory limit")
      chunks.push(value)
    }
  } finally { await reader.cancel().catch(() => undefined); reader.releaseLock() }
  const result = new Uint8Array(size)
  let offset = 0
  for (const chunk of chunks) { result.set(chunk, offset); offset += chunk.length }
  return result
}

export class KittyGraphics<T> {
  readonly images = new Map<number, KittyImage<T>>()
  readonly placements = new Map<string, KittyPlacement>()
  private upload: { control: Record<string, string>; parts: string[]; size: number } | null = null
  private serial = 0
  private disposed = false
  constructor(private target: GraphicsTarget<T>) {}

  async accept(token: Extract<KittyToken, { type: "graphics" }>): Promise<void> {
    if (this.disposed) return
    let control = token.control
    try {
      const action = control.a ?? this.upload?.control.a ?? "t"
      if (["t", "T", "q"].includes(action)) {
        this.upload ??= { control, parts: [], size: 0 }
        const upload = this.upload
        control = upload.control
        upload.size += token.payload.length
        // Padded base64 length of the byte limit; decoded size is checked again below.
        if (upload.size > 4 * Math.ceil(MAX_KITTY_BYTES / 3)) throw new Error("EFBIG: Image exceeds memory limit")
        if (!/^[A-Za-z0-9+/]*={0,2}$/.test(token.payload)) throw new Error("EINVAL: Invalid image encoding")
        upload.parts.push(token.payload)
        if (token.control.m === "1") return
        this.upload = null
        if (control.t && control.t !== "d") throw new Error("ENOTSUP: Only inline image data is supported")
        const encoded = upload.parts.join("")
        let bytes = Uint8Array.from(atob(encoded), char => char.charCodeAt(0))
        if (control.o === "z") bytes = await inflate(bytes)
        else if (control.o) throw new Error("ENOTSUP: Unknown compression")
        const format = integer(control.f, 32)
        let width = integer(control.s), height = integer(control.v)
        if (format === 100) {
          if (bytes.length < 24 || ![137, 80, 78, 71, 13, 10, 26, 10].every((v, i) => bytes[i] === v)
            || String.fromCharCode(...bytes.subarray(12, 16)) !== "IHDR") throw new Error("EINVAL: Invalid PNG")
          const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
          width = view.getUint32(16); height = view.getUint32(20)
        } else if (![24, 32].includes(format) || bytes.length !== width * height * (format / 8)) throw new Error("EINVAL: Invalid pixel data")
        if (width < 1 || height < 1 || width > 8192 || height > 8192 || width * height * 4 > MAX_KITTY_BYTES) throw new Error("EFBIG: Image dimensions exceed limit")
        if (action === "q") { this.respond(control, "OK"); return }
        const id = clientId(control.i) || AUTO_ID_BASE + ++this.serial
        let memory = width * height * 4
        for (const [key, image] of this.images) if (key !== id) memory += image.width * image.height * 4
        if (memory > MAX_KITTY_BYTES || (!this.images.has(id) && this.images.size >= 128)) throw new Error("ENOSPC: Image cache is full")
        const image = await this.target.load(bytes, width, height, format)
        if (this.disposed) { this.target.dispose(image.image); return }
        const previous = this.images.get(id)
        if (previous) this.target.dispose(previous.image)
        this.images.set(id, image)
        if (action === "T") this.place(id, control)
      } else if (action === "p") {
        this.place(integer(control.i), control)
      } else if (action === "d") {
        const mode = control.d ?? "a", imageId = integer(control.i), placementId = integer(control.p)
        if (!["a", "A", "i", "I"].includes(mode)) throw new Error("ENOTSUP: Unsupported deletion")
        for (const [key, placement] of this.placements) {
          if (mode.toLowerCase() === "a" || (placement.imageId === imageId && (!placementId || placement.id === placementId))) this.placements.delete(key)
        }
        if (mode === "A") this.clearImages()
        if (mode === "I" && ![...this.placements.values()].some(p => p.imageId === imageId)) {
          const image = this.images.get(imageId)
          if (image) this.target.dispose(image.image)
          this.images.delete(imageId)
        }
      } else throw new Error("ENOTSUP: Unsupported graphics command")
      this.respond(control, "OK")
      this.target.changed()
    } catch (error) {
      this.upload = null
      this.respond(control, error instanceof Error ? error.message : "EINVAL: Invalid graphics")
    }
  }

  private respond(control: Record<string, string>, message: string) {
    if (control.q === "2" || (control.q === "1" && message === "OK")) return
    const id = /^\d+$/.test(control.i ?? "") ? `i=${control.i}` : "i=0"
    const safe = [...message].filter(char => char.charCodeAt(0) >= 32).join("").slice(0, 160)
    this.target.reply(`\x1b_G${id};${safe}\x1b\\`)
  }

  private place(imageId: number, control: Record<string, string>) {
    const image = this.images.get(imageId)
    if (!image) throw new Error("ENOENT: Image not found")
    const id = clientId(control.p) || AUTO_ID_BASE + ++this.serial
    const cursor = this.target.cursor()
    const x = integer(control.x), y = integer(control.y)
    const placement: KittyPlacement = {
      imageId, id, ...cursor, x, y, width: integer(control.w, image.width - x), height: integer(control.h, image.height - y),
      offsetX: integer(control.X), offsetY: integer(control.Y), cols: integer(control.c), rows: integer(control.r), z: integer(control.z)
    }
    if (x < 0 || y < 0 || placement.width < 1 || placement.height < 1 || x + placement.width > image.width || y + placement.height > image.height
      || placement.cols < 0 || placement.cols > 1000 || placement.rows < 0 || placement.rows > 1000) throw new Error("EINVAL: Invalid placement")
    if (this.placements.size >= 512 && !this.placements.has(`${imageId}:${id}`)) throw new Error("ENOSPC: Too many placements")
    this.placements.set(`${imageId}:${id}`, placement)
  }
  private clearImages() {
    for (const image of this.images.values()) this.target.dispose(image.image)
    this.images.clear()
  }
  clear() { this.upload = null; this.placements.clear(); this.clearImages(); this.target.changed() }
  dispose() { this.disposed = true; this.clear() }
}
