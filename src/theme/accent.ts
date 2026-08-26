export const ACCENT_THEMES = {
  lime: { solid: "#86b81f", rgb: "134, 184, 31", ink: "#5f8c1e" },
  blue: { solid: "#2f6bff", rgb: "47, 107, 255", ink: "#2456cc" },
  violet: { solid: "#7b5bff", rgb: "123, 91, 255", ink: "#5d3fd3" },
  coral: { solid: "#ff6b54", rgb: "255, 107, 84", ink: "#c0562f" },
  amber: { solid: "#e0a11f", rgb: "224, 161, 31", ink: "#a8690f" },
} as const

export type AccentPreference = keyof typeof ACCENT_THEMES

export const DEFAULT_ACCENT_PREFERENCE: AccentPreference = "lime"

export function isAccentPreference(value: unknown): value is AccentPreference {
  return typeof value === "string" && Object.prototype.hasOwnProperty.call(ACCENT_THEMES, value)
}

export function applyAccentPreference(
  accent: AccentPreference,
  root: HTMLElement = document.documentElement
): void {
  const palette = ACCENT_THEMES[accent]
  root.style.setProperty("--yz-accent", palette.solid)
  root.style.setProperty("--yz-accent-rgb", palette.rgb)
  root.style.setProperty("--yz-accent-ink", palette.ink)
}
