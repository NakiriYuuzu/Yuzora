import { workspacePathBasename } from "@/lib/paths"
import type {
  RecentWorkspaceColor,
  RecentWorkspacePresentation,
} from "@/state/recentWorkspaces"

export interface ProjectColorOption {
  id: RecentWorkspaceColor
  background: string
  foreground: string
}

export const PROJECT_COLOR_OPTIONS: readonly ProjectColorOption[] = [
  {
    id: "lime",
    background: "linear-gradient(150deg, #bfe04f, #7fae28)",
    foreground: "#1f2b06",
  },
  { id: "dusk", background: "var(--grad-dusk)", foreground: "#fff" },
  { id: "sunrise", background: "var(--grad-sunrise)", foreground: "#fff" },
  {
    id: "mint",
    background: "linear-gradient(150deg, #2bbf8a, #2f6bff)",
    foreground: "#fff",
  },
  {
    id: "coral",
    background: "linear-gradient(150deg, #ff8a5b, #e0539b)",
    foreground: "#fff",
  },
  {
    id: "ocean",
    background: "linear-gradient(150deg, #7b5bff, #2f6bff)",
    foreground: "#fff",
  },
]

const DEFAULT_COLOR = PROJECT_COLOR_OPTIONS[2]!
const FIXED_GLYPHS = ["🦀", "📦", "⚡", "🌐", "🧩", "🛠", "✦"] as const

export function resolveProjectPresentation(
  path: string,
  presentation?: RecentWorkspacePresentation
) {
  const folderName = workspacePathBasename(path)
  const name = presentation?.name?.trim() || folderName
  const glyph = presentation?.glyph?.trim() || name.trim().charAt(0).toUpperCase() || "P"
  const color = PROJECT_COLOR_OPTIONS.find((option) => option.id === presentation?.color)
    ?? DEFAULT_COLOR
  return { name, glyph, color }
}

export function projectGlyphOptions(name: string): string[] {
  const initial = name.trim().charAt(0).toUpperCase() || "P"
  return [initial, ...FIXED_GLYPHS.filter((glyph) => glyph !== initial)]
}
