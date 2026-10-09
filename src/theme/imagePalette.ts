import { MAX_GRADIENT_COLORS, rgbToHex } from "@/theme/background"

const SAMPLE_SIZE = 64
// Picks closer than this (RGB distance) would read as one color in the gradient.
const MIN_PICK_DISTANCE = 64

/**
 * Dominant colors of an RGBA buffer, most prominent first. Vivid, mid-tone
 * colors outrank near-white/near-black areas (paper, letterboxing), but a
 * grayscale image still yields its grays instead of nothing.
 */
export function extractPaletteFromPixels(data: Uint8ClampedArray, count = MAX_GRADIENT_COLORS): string[] {
  const buckets = new Map<number, { n: number; r: number; g: number; b: number }>()
  for (let index = 0; index + 3 < data.length; index += 4) {
    if (data[index + 3] < 128) continue
    const r = data[index], g = data[index + 1], b = data[index + 2]
    const key = ((r >> 4) << 8) | ((g >> 4) << 4) | (b >> 4)
    const bucket = buckets.get(key)
    if (bucket) { bucket.n += 1; bucket.r += r; bucket.g += g; bucket.b += b }
    else buckets.set(key, { n: 1, r, g, b })
  }

  const ranked = [...buckets.values()]
    .map(({ n, r, g, b }) => {
      const rgb: [number, number, number] = [r / n, g / n, b / n]
      const max = Math.max(...rgb) / 255
      const min = Math.min(...rgb) / 255
      const lightness = (max + min) / 2
      const saturation = max === min ? 0 : (max - min) / (1 - Math.abs(2 * lightness - 1))
      const tone = lightness < 0.1 || lightness > 0.92 ? 0.15 : 1
      return { rgb, score: n * (0.25 + saturation) * tone }
    })
    .sort((left, right) => right.score - left.score)

  const picks: [number, number, number][] = []
  for (const { rgb } of ranked) {
    if (picks.length >= count) break
    if (picks.every(pick => Math.hypot(pick[0] - rgb[0], pick[1] - rgb[1], pick[2] - rgb[2]) >= MIN_PICK_DISTANCE)) picks.push(rgb)
  }
  return picks.map(([r, g, b]) => rgbToHex(r, g, b))
}

/** Decodes an image file and samples it down before extracting its palette. */
export async function extractPaletteFromImage(file: Blob, count = MAX_GRADIENT_COLORS): Promise<string[]> {
  const bitmap = await createImageBitmap(file)
  try {
    const canvas = document.createElement("canvas")
    canvas.width = SAMPLE_SIZE
    canvas.height = SAMPLE_SIZE
    const context = canvas.getContext("2d", { willReadFrequently: true })
    if (!context) return []
    context.drawImage(bitmap, 0, 0, SAMPLE_SIZE, SAMPLE_SIZE)
    return extractPaletteFromPixels(context.getImageData(0, 0, SAMPLE_SIZE, SAMPLE_SIZE).data, count)
  } finally {
    bitmap.close()
  }
}
