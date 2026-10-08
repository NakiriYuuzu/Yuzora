/**
 * Window backdrop preferences. "accent" keeps the accent-driven four-glow
 * atmosphere from system-tone.css; "gradient" overrides those glows with up to
 * three user colors (Arc-style), deriving the light and dark variants so one
 * gradient reads in both themes; "image" covers the window with a user image
 * under a theme-colored veil. The accent itself is never touched.
 */

export type BackgroundSource = "accent" | "gradient" | "image"

export interface BackgroundGradient {
  /** One to three `#rrggbb` colors, top-left to bottom-right. */
  colors: string[]
  /** 0–100: how strongly the colors show through the neutral base. */
  intensity: number
}

export const MAX_GRADIENT_COLORS = 3
export const MAX_SAVED_GRADIENTS = 12
export const DEFAULT_GRADIENT_INTENSITY = 50
/** Glass mode: how much of the backdrop covers the native vibrancy (0–100). */
export const DEFAULT_GLASS_TINT = 20
/** Image backdrop: 0–100, how strongly the image shows through the theme veil. */
export const DEFAULT_IMAGE_INTENSITY = 70

export const BACKGROUND_PRESETS = {
  aurora: ["#3ddc97", "#46a0ff", "#a66bff"],
  sunset: ["#ff7a59", "#ffb347", "#ff5c8a"],
  ocean: ["#1fb6ff", "#00c2a8", "#3d5afe"],
  sakura: ["#ff8fb1", "#ffc6d9", "#b39dff"],
  forest: ["#5fa463", "#a3c45a", "#3f8f7f"],
  lavender: ["#9b7bff", "#c792ea"],
  citrus: ["#ffd23f", "#9be15d"],
  graphite: ["#7d8597"],
} as const satisfies Record<string, readonly string[]>

export type BackgroundPresetId = keyof typeof BACKGROUND_PRESETS

export const DEFAULT_BACKGROUND_GRADIENT: BackgroundGradient = {
  colors: [...BACKGROUND_PRESETS.aurora],
  intensity: DEFAULT_GRADIENT_INTENSITY,
}

const LIGHT_BASE = "#f6f5ef"
const DARK_BASE = "#100f14"
const GLOW_CORNERS = ["a", "b", "c", "d"] as const
const MANAGED_PROPERTIES = [
  ...GLOW_CORNERS.flatMap(corner => [`--yz-glow-${corner}-light`, `--yz-glow-${corner}-dark`]),
  "--yz-base-light",
  "--yz-base-dark",
]

export function isHexColor(value: unknown): value is string {
  return typeof value === "string" && /^#[0-9a-f]{6}$/i.test(value)
}

export function normalizeGradientIntensity(value: unknown): number {
  if (typeof value !== "number" || !Number.isFinite(value)) return DEFAULT_GRADIENT_INTENSITY
  return Math.min(100, Math.max(0, Math.round(value)))
}

export function isBackgroundSource(value: unknown): value is BackgroundSource {
  return value === "accent" || value === "gradient" || value === "image"
}

export function normalizeGlassTint(value: unknown): number {
  if (typeof value !== "number" || !Number.isFinite(value)) return DEFAULT_GLASS_TINT
  return Math.min(100, Math.max(0, Math.round(value)))
}

export function normalizeBackgroundGradient(value: unknown): BackgroundGradient | null {
  if (!value || typeof value !== "object") return null
  const { colors, intensity } = value as Partial<BackgroundGradient>
  if (!Array.isArray(colors)) return null
  const valid = colors.filter(isHexColor).map(color => color.toLowerCase()).slice(0, MAX_GRADIENT_COLORS)
  if (valid.length === 0) return null
  return { colors: valid, intensity: normalizeGradientIntensity(intensity) }
}

export function sameGradientColors(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((color, index) => color.toLowerCase() === right[index]?.toLowerCase())
}

/** Newest first; re-saving the same colors moves them to the front instead of duplicating. */
export function addSavedGradient(saved: readonly BackgroundGradient[], gradient: BackgroundGradient): BackgroundGradient[] {
  const rest = saved.filter(entry => !sameGradientColors(entry.colors, gradient.colors))
  return [gradient, ...rest].slice(0, MAX_SAVED_GRADIENTS)
}

/** Spreads one to three colors over the four glow corners (a=TL, b=TR, c=BR, d=BL). */
export function gradientCorners(colors: readonly string[]): [string, string, string, string] {
  const [first, second = first, third] = colors
  if (third === undefined) return [first, second, second, first]
  return [first, second, third, mixHex(first, third)]
}

/** CSS custom properties that make `--yz-bg` paint the gradient in both themes. */
export function gradientCustomProperties(gradient: BackgroundGradient): Record<string, string> {
  const light = 8 + gradient.intensity * 0.44
  const dark = 6 + gradient.intensity * 0.3
  const properties: Record<string, string> = {}
  gradientCorners(gradient.colors).forEach((color, index) => {
    const corner = GLOW_CORNERS[index]
    properties[`--yz-glow-${corner}-light`] = mixDeclaration(color, light, LIGHT_BASE)
    properties[`--yz-glow-${corner}-dark`] = mixDeclaration(color, dark, DARK_BASE)
  })
  properties["--yz-base-light"] = mixDeclaration(gradient.colors[0], light * 0.15, LIGHT_BASE)
  properties["--yz-base-dark"] = mixDeclaration(gradient.colors[0], dark * 0.2, DARK_BASE)
  return properties
}

export function applyBackgroundPreference(
  source: BackgroundSource,
  gradient: BackgroundGradient,
  root: HTMLElement = document.documentElement
): void {
  const properties = source === "gradient" ? gradientCustomProperties(gradient) : {}
  for (const name of MANAGED_PROPERTIES) {
    const value = properties[name]
    if (value) root.style.setProperty(name, value)
    else root.style.removeProperty(name)
  }
  root.dataset.background = source
}

/** Theme-base share laid over the image; never below 20% so text stays readable. */
export function imageVeilPercent(intensity: number): number {
  return Math.round(100 - normalizeGradientIntensity(intensity) * 0.8)
}

/** `url` is an object URL for the stored image, or null to drop it. */
export function applyBackgroundImage(
  url: string | null,
  intensity: number,
  root: HTMLElement = document.documentElement
): void {
  if (!url) {
    root.style.removeProperty("--yz-bg-image")
    root.style.removeProperty("--yz-image-veil")
    return
  }
  root.style.setProperty("--yz-bg-image", `url("${url}")`)
  root.style.setProperty("--yz-image-veil", `color-mix(in srgb, var(--yz-tone-base) ${imageVeilPercent(intensity)}%, transparent)`)
}

/** Preview for a swatch: the same corner layout, at full color. */
export function gradientSwatchBackground(colors: readonly string[]): string {
  const [a, b, c, d] = gradientCorners(colors)
  return `linear-gradient(135deg, ${a} 0%, ${b} 45%, ${c} 100%), ${d}`
}

// ---- color pad: x = hue, y = vivid (top) → pastel (bottom) ----

export function padColor(x: number, y: number): string {
  const clampedY = clamp01(y)
  return hslToHex(clamp01(x) * 360, 1 - clampedY * 0.75, 0.52 + clampedY * 0.3)
}

export function padPosition(hex: string): { x: number; y: number } {
  const { h, s } = hexToHsl(hex)
  return { x: h / 360, y: clamp01((1 - s) / 0.75) }
}

function mixDeclaration(color: string, percent: number, base: string): string {
  return `color-mix(in oklab, ${color} ${Math.round(percent)}%, ${base})`
}

function clamp01(value: number): number {
  return Math.min(1, Math.max(0, Number.isFinite(value) ? value : 0))
}

export function hexToRgb(hex: string): [number, number, number] {
  const value = Number.parseInt(hex.slice(1), 16)
  return [(value >> 16) & 255, (value >> 8) & 255, value & 255]
}

export function rgbToHex(r: number, g: number, b: number): string {
  return `#${[r, g, b].map(channel => Math.round(Math.min(255, Math.max(0, channel))).toString(16).padStart(2, "0")).join("")}`
}

function mixHex(left: string, right: string): string {
  const [r1, g1, b1] = hexToRgb(left)
  const [r2, g2, b2] = hexToRgb(right)
  return rgbToHex((r1 + r2) / 2, (g1 + g2) / 2, (b1 + b2) / 2)
}

function hexToHsl(hex: string): { h: number; s: number; l: number } {
  const [r, g, b] = hexToRgb(hex).map(channel => channel / 255)
  const max = Math.max(r, g, b)
  const min = Math.min(r, g, b)
  const l = (max + min) / 2
  const delta = max - min
  if (delta === 0) return { h: 0, s: 0, l }
  const s = delta / (1 - Math.abs(2 * l - 1))
  const h = max === r ? ((g - b) / delta + 6) % 6 : max === g ? (b - r) / delta + 2 : (r - g) / delta + 4
  return { h: h * 60, s: Math.min(1, s), l }
}

function hslToHex(h: number, s: number, l: number): string {
  const chroma = (1 - Math.abs(2 * l - 1)) * s
  const segment = (h % 360) / 60
  const x = chroma * (1 - Math.abs((segment % 2) - 1))
  const [r, g, b] =
    segment < 1 ? [chroma, x, 0]
      : segment < 2 ? [x, chroma, 0]
        : segment < 3 ? [0, chroma, x]
          : segment < 4 ? [0, x, chroma]
            : segment < 5 ? [x, 0, chroma]
              : [chroma, 0, x]
  const m = l - chroma / 2
  return rgbToHex((r + m) * 255, (g + m) * 255, (b + m) * 255)
}
