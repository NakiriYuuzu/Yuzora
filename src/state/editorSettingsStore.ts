import { create } from "zustand"

// Editor-surface preferences (font size + minimap). Unlike the Terminal / Preview
// stores (read on the next action), this one is reactive: an already-open editor
// must reflect a change at once, so EditorPane subscribes to it (F6).
export const EDITOR_SETTINGS_STORAGE_KEY = "yuzora.editor.settings.v1"

export type EditorFontSize = 12 | 13 | 14 | 15

const FONT_SIZES: readonly EditorFontSize[] = [12, 13, 14, 15]
const DEFAULT_FONT_SIZE: EditorFontSize = 13
const DEFAULT_MINIMAP = false

export interface EditorSettings {
    fontSize: EditorFontSize
    minimap: boolean
}

interface EditorSettingsStore extends EditorSettings {
    setFontSize: (size: EditorFontSize) => void
    setMinimap: (enabled: boolean) => void
}

function isFontSize(value: unknown): value is EditorFontSize {
    return FONT_SIZES.includes(value as EditorFontSize)
}

// Whitelist-validate each field so a hand-edited / stale payload can't inject an
// out-of-range font size or a non-boolean toggle; anything off falls back.
export function loadEditorSettings(): EditorSettings {
    try {
        const raw = localStorage.getItem(EDITOR_SETTINGS_STORAGE_KEY)
        if (!raw) return { fontSize: DEFAULT_FONT_SIZE, minimap: DEFAULT_MINIMAP }
        const parsed = JSON.parse(raw) as Record<string, unknown>
        return {
            fontSize: isFontSize(parsed.fontSize) ? parsed.fontSize : DEFAULT_FONT_SIZE,
            minimap: typeof parsed.minimap === "boolean" ? parsed.minimap : DEFAULT_MINIMAP
        }
    } catch {
        return { fontSize: DEFAULT_FONT_SIZE, minimap: DEFAULT_MINIMAP }
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
    setFontSize: (fontSize) => {
        set({ fontSize })
        saveEditorSettings({ fontSize, minimap: get().minimap })
    },
    setMinimap: (minimap) => {
        set({ minimap })
        saveEditorSettings({ fontSize: get().fontSize, minimap })
    }
}))
