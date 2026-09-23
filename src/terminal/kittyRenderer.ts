import type { Terminal } from "@xterm/xterm"
import { KittyGraphics, KittyStreamParser } from "./kittyProtocol"

async function loadImage(bytes: Uint8Array<ArrayBuffer>, width: number, height: number, format: number) {
  const canvas = document.createElement("canvas")
  canvas.width = width; canvas.height = height
  const context = canvas.getContext("2d")
  if (!context) throw new Error("Canvas is unavailable")
  if (format === 100) {
    const url = URL.createObjectURL(new Blob([bytes], { type: "image/png" }))
    try {
      const image = new Image()
      image.src = url
      await image.decode()
      context.drawImage(image, 0, 0)
    } finally { URL.revokeObjectURL(url) }
  } else {
    const pixels = new Uint8ClampedArray(width * height * 4)
    if (format === 32) pixels.set(bytes)
    else for (let i = 0, j = 0; i < bytes.length; i += 3, j += 4) {
      pixels[j] = bytes[i]; pixels[j + 1] = bytes[i + 1]; pixels[j + 2] = bytes[i + 2]; pixels[j + 3] = 255
    }
    context.putImageData(new ImageData(pixels, width, height), 0, 0)
  }
  return { image: canvas, width, height }
}

/** Two bounded canvas layers preserve Kitty's placement order around text. */
export function installKittyRenderer(term: Terminal, reply: (text: string) => void) {
  const screen = term.element?.querySelector<HTMLElement>(".xterm-screen")
  if (!screen) throw new Error("Terminal screen is unavailable")
  const layers = [document.createElement("canvas"), document.createElement("canvas")]
  for (const [index, layer] of layers.entries()) {
    Object.assign(layer.style, { position: "absolute", inset: "0", pointerEvents: "none", zIndex: index === 0 ? "0" : "3" })
    layer.setAttribute("aria-hidden", "true")
    screen.append(layer)
  }
  // The standard xterm DOM text renderer stays above negative-z images.
  const rows = screen.querySelector<HTMLElement>(".xterm-rows")
  const previousZ = rows?.style.zIndex ?? ""
  const previousPosition = rows?.style.position ?? ""
  if (rows) {
    rows.style.position = "relative"
    rows.style.zIndex = "1"
  }
  let disposed = false, frame = 0
  const redraw = () => {
    frame = 0
    if (disposed) return
    const width = screen.clientWidth, height = screen.clientHeight
    if (!width || !height) return
    const scale = window.devicePixelRatio || 1
    for (const [index, layer] of layers.entries()) {
      layer.width = Math.ceil(width * scale); layer.height = Math.ceil(height * scale)
      layer.style.width = `${width}px`; layer.style.height = `${height}px`
      const context = layer.getContext("2d")
      if (!context) continue
      context.scale(scale, scale)
      for (const placement of [...graphics.placements.values()].sort((a, b) => a.z - b.z)) {
        if ((placement.z < 0 ? 0 : 1) !== index) continue
        const image = graphics.images.get(placement.imageId)
        if (!image) continue
        const cellWidth = width / term.cols, cellHeight = height / term.rows
        const x = placement.col * cellWidth + placement.offsetX
        const y = (placement.row - term.buffer.active.viewportY) * cellHeight + placement.offsetY
        const destWidth = placement.cols ? placement.cols * cellWidth - placement.offsetX : placement.width
        const destHeight = placement.rows ? placement.rows * cellHeight - placement.offsetY : placement.height
        if (destWidth <= 0 || destHeight <= 0) continue
        context.drawImage(image.image, placement.x, placement.y, placement.width, placement.height, x, y, destWidth, destHeight)
      }
    }
  }
  const schedule = () => { if (!disposed && !frame) frame = requestAnimationFrame(redraw) }
  const graphics = new KittyGraphics<HTMLCanvasElement>({
    load: loadImage,
    dispose: canvas => { canvas.width = 0; canvas.height = 0 },
    cursor: () => ({ col: term.buffer.active.cursorX, row: term.buffer.active.baseY + term.buffer.active.cursorY }),
    reply, changed: schedule
  })
  const parser = new KittyStreamParser(), decoder = new TextDecoder()
  const listeners = [term.onResize(schedule), term.onScroll(schedule), term.parser.registerEscHandler({ final: "c" }, () => { graphics.clear(); return false })]
  return {
    async write(bytes: Uint8Array) {
      for (const token of parser.feed(decoder.decode(bytes, { stream: true }))) {
        if (disposed) return
        if (token.type === "text") await new Promise<void>(resolve => term.write(token.text, resolve))
        else await graphics.accept(token)
      }
    },
    dispose() {
      disposed = true; cancelAnimationFrame(frame)
      listeners.forEach(listener => listener.dispose()); graphics.dispose()
      layers.forEach(layer => layer.remove())
      if (rows) {
        rows.style.zIndex = previousZ
        rows.style.position = previousPosition
      }
    }
  }
}
