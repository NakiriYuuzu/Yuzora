import { beforeEach, describe, expect, it } from "vitest"

import {
    EDITOR_SETTINGS_STORAGE_KEY,
    loadEditorSettings,
    useEditorSettingsStore
} from "./editorSettingsStore"

// The Bun-hosted test runtime injects an empty `localStorage` global with no
// Storage methods; install a minimal in-memory Storage so persistence runs for
// real (mirrors sshStore.test.ts).
function installLocalStorage(): void {
    const store = new Map<string, string>()
    const mock = {
        getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
        setItem: (k: string, v: string) => void store.set(k, String(v)),
        removeItem: (k: string) => void store.delete(k),
        clear: () => store.clear(),
        key: (i: number) => [...store.keys()][i] ?? null,
        get length() {
            return store.size
        }
    }
    Object.defineProperty(globalThis, "localStorage", {
        value: mock,
        configurable: true,
        writable: true
    })
}

beforeEach(() => {
    installLocalStorage()
    useEditorSettingsStore.setState({ fontSize: 13, minimap: false, syntaxTheme: "github", markdownDefaultMode: "document" })
})

describe("loadEditorSettings", () => {
    it("returns the defaults (13 / false) when nothing is stored", () => {
        expect(loadEditorSettings()).toEqual({ fontSize: 13, minimap: false, syntaxTheme: "github", markdownDefaultMode: "document" })
    })

    it("round-trips a valid persisted payload", () => {
        localStorage.setItem(EDITOR_SETTINGS_STORAGE_KEY, JSON.stringify({ fontSize: 15, minimap: true, syntaxTheme: "github" }))
        expect(loadEditorSettings()).toEqual({ fontSize: 15, minimap: true, syntaxTheme: "github", markdownDefaultMode: "document" })
    })

    it("falls back per-field on out-of-whitelist font size / non-boolean minimap", () => {
        localStorage.setItem(EDITOR_SETTINGS_STORAGE_KEY, JSON.stringify({ fontSize: 99, minimap: "yes" }))
        expect(loadEditorSettings()).toEqual({ fontSize: 13, minimap: false, syntaxTheme: "github", markdownDefaultMode: "document" })
        // A valid field survives even when its sibling is invalid.
        localStorage.setItem(EDITOR_SETTINGS_STORAGE_KEY, JSON.stringify({ fontSize: 14, minimap: 1 }))
        expect(loadEditorSettings()).toEqual({ fontSize: 14, minimap: false, syntaxTheme: "github", markdownDefaultMode: "document" })
    })

    it("falls back to defaults on malformed JSON", () => {
        localStorage.setItem(EDITOR_SETTINGS_STORAGE_KEY, "{not json")
        expect(loadEditorSettings()).toEqual({ fontSize: 13, minimap: false, syntaxTheme: "github", markdownDefaultMode: "document" })
    })
})

describe("useEditorSettingsStore", () => {
    it("setFontSize updates state and persists (survives a fresh load)", () => {
        useEditorSettingsStore.getState().setFontSize(15)
        expect(useEditorSettingsStore.getState().fontSize).toBe(15)
        expect(loadEditorSettings().fontSize).toBe(15)
    })

    it("setMinimap updates state and persists without clobbering the font size", () => {
        useEditorSettingsStore.getState().setFontSize(12)
        useEditorSettingsStore.getState().setMinimap(true)
        expect(useEditorSettingsStore.getState()).toMatchObject({ fontSize: 12, minimap: true, syntaxTheme: "github" })
        // Both fields land together in localStorage — a fresh app load restores both.
        expect(loadEditorSettings()).toEqual({ fontSize: 12, minimap: true, syntaxTheme: "github", markdownDefaultMode: "document" })
    })
})

it("persists syntax theme without losing other editor preferences", () => {
    useEditorSettingsStore.getState().setSyntaxTheme("one")
    useEditorSettingsStore.getState().setFontSize(15)
    expect(loadEditorSettings()).toEqual({ fontSize: 15, minimap: false, syntaxTheme: "one", markdownDefaultMode: "document" })
})

it("persists the Markdown default view and rejects unknown modes", () => {
    useEditorSettingsStore.getState().setMarkdownDefaultMode("source")
    expect(loadEditorSettings().markdownDefaultMode).toBe("source")
    localStorage.setItem(EDITOR_SETTINGS_STORAGE_KEY, JSON.stringify({ fontSize: 14, markdownDefaultMode: "preview" }))
    expect(loadEditorSettings()).toMatchObject({ fontSize: 14, markdownDefaultMode: "document" })
})
