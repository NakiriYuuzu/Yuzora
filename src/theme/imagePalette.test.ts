import { describe, expect, it } from "vitest"

import { extractPaletteFromPixels } from "./imagePalette"

function image(regions: [count: number, rgba: [number, number, number, number]][]): Uint8ClampedArray {
  return new Uint8ClampedArray(regions.flatMap(([count, rgba]) => Array.from({ length: count }, () => rgba).flat()))
}

describe("image palette extraction", () => {
  it("returns distinct dominant colors, most prominent first", () => {
    const pixels = image([[500, [30, 120, 220, 255]], [300, [230, 90, 40, 255]], [150, [60, 180, 90, 255]]])
    expect(extractPaletteFromPixels(pixels)).toEqual(["#1e78dc", "#e65a28", "#3cb45a"])
  })

  it("prefers vivid colors over a large paper-white or black backdrop", () => {
    const pixels = image([[2000, [250, 250, 248, 255]], [800, [5, 5, 5, 255]], [200, [220, 60, 120, 255]]])
    expect(extractPaletteFromPixels(pixels, 1)).toEqual(["#dc3c78"])
  })

  it("merges near-identical shades instead of returning one color twice", () => {
    const pixels = image([[400, [30, 120, 220, 255]], [380, [40, 128, 228, 255]], [100, [230, 90, 40, 255]]])
    expect(extractPaletteFromPixels(pixels)).toEqual(["#1e78dc", "#e65a28"])
  })

  it("still yields grays for a grayscale image and ignores transparent pixels", () => {
    const pixels = image([[300, [120, 120, 120, 255]], [900, [255, 0, 0, 0]]])
    expect(extractPaletteFromPixels(pixels)).toEqual(["#787878"])
    expect(extractPaletteFromPixels(new Uint8ClampedArray())).toEqual([])
  })
})
