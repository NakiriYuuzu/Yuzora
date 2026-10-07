import { create } from "zustand"

// Editor-surface preferences (font size + minimap). Unlike the Terminal / Preview
// stores (read on the next action), this one is reactive: an already-open editor
// must reflect a change at once, so EditorPane subscribes to it (F6).
export const EDITOR_SETTINGS_STORAGE_KEY = "yuzora.editor.settings.v1"

export const SYNTAX_THEMES = ["github", "yuzora", "one"] as const
export type SyntaxTheme = typeof SYNTAX_THEMES[number]

export type EditorFontSize = 12 | 13 | 14 | 15

/** How a Markdown file opens until the user switches that file's mode. */
export const MARKDOWN_VIEW_MODES = ["document", "source"] as const
export type MarkdownViewMode = typeof MARKDOWN_VIEW_MODES[number]

const FONT_SIZES: readonly EditorFontSize[] = [12, 13, 14, 15]
const DEFAULT_FONT_SIZE: EditorFontSize = 13
const DEFAULT_MINIMAP = false
const DEFAULT_MARKDOWN_MODE: MarkdownViewMode = "document"

export interface EditorSettings {
    fontSize: EditorFontSize
    minimap: boolean
    syntaxTheme: SyntaxTheme
    markdownDefaultMode: MarkdownViewMode
}

interface EditorSettingsStore extends EditorSettings {
    setSyntaxTheme: (theme: SyntaxTheme) => void
    setFontSize: (size: EditorFontSize) => void
    setMinimap: (enabled: boolean) => void
    setMarkdownDefaultMode: (mode: MarkdownViewMode) => void
}

function isFontSize(value: unknown): value is EditorFontSize {
    return FONT_SIZES.includes(value as EditorFontSize)
}

// Whitelist-validate each field so a hand-edited / stale payload can't inject an
// out-of-range font size or a non-boolean toggle; anything off falls back.
export function loadEditorSettings(): EditorSettings {
    try {
        const raw = localStorage.getItem(EDITOR_SETTINGS_STORAGE_KEY)
        if (!raw) return { fontSize: DEFAULT_FONT_SIZE, minimap: DEFAULT_MINIMAP, syntaxTheme: "github", markdownDefaultMode: DEFAULT_MARKDOWN_MODE }
        const parsed = JSON.parse(raw) as Record<string, unknown>
        return {
            syntaxTheme: SYNTAX_THEMES.includes(parsed.syntaxTheme as SyntaxTheme) ? parsed.syntaxTheme as SyntaxTheme : "github",
            fontSize: isFontSize(parsed.fontSize) ? parsed.fontSize : DEFAULT_FONT_SIZE,
            minimap: typeof parsed.minimap === "boolean" ? parsed.minimap : DEFAULT_MINIMAP,
            markdownDefaultMode: MARKDOWN_VIEW_MODES.includes(parsed.markdownDefaultMode as MarkdownViewMode)
                ? parsed.markdownDefaultMode as MarkdownViewMode : DEFAULT_MARKDOWN_MODE
        }
    } catch {
        return { fontSize: DEFAULT_FONT_SIZE, minimap: DEFAULT_MINIMAP, syntaxTheme: "github", markdownDefaultMode: DEFAULT_MARKDOWN_MODE }
    }
}

function saveEditorSettings(settings: EditorSettings): void {
    try {
        localStorage.setItem(EDITOR_SETTINGS_STORAGE_KEY, JSON.stringify(settings))
    } catch {
        // private mode / quota — in-memory state stays authoritative
    }
}

export const useEditorSettingsStore = create<EditorSettingsStore>()((set, get) => ({
    ...loadEditorSettings(),
    setSyntaxTheme: (syntaxTheme) => {
        set({ syntaxTheme })
        saveEditorSettings(get())
    },
    setFontSize: (fontSize) => {
        set({ fontSize })
        saveEditorSettings(get())
    },
    setMinimap: (minimap) => {
        set({ minimap })
        saveEditorSettings(get())
    },
    setMarkdownDefaultMode: (markdownDefaultMode) => {
        set({ markdownDefaultMode })
        saveEditorSettings(get())
    }
}))
