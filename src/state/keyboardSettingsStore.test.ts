import { beforeEach, describe, expect, it, vi } from "vitest"
import { bindingError, dispatchAppShortcut, normalizeBinding, loadKeyboardOverrides, KEYBOARD_STORAGE_KEY, useKeyboardSettingsStore } from "./keyboardSettingsStore"

beforeEach(() => {
    const values = new Map<string, string>()
    vi.stubGlobal("localStorage", { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => values.set(key, value) })
    useKeyboardSettingsStore.setState({ overrides: {} })
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Windows")
})
describe("app shortcut bindings", () => {
    it("normalizes order and rejects ambiguous or unmodified input", () => {
        expect(normalizeBinding("shift+mod+k")).toBe("Mod+Shift+K")
        for (const value of ["K", "Mod+Ctrl+K", "Mod+Mod+K", "Mod+Enter"]) expect(normalizeBinding(value)).toBeNull()
    })
    it("rejects command conflicts and reserved editing/system chords", () => {
        expect(bindingError("newTerminal", "Ctrl+K", {})).toBe("conflict")
        expect(bindingError("commandPalette", "Mod+C", {})).toBe("reserved")
        expect(bindingError("commandPalette", "Alt+F4", {})).toBe("reserved")
        expect(bindingError("commandPalette", "Mod+Shift+3", {})).toBe("reserved")
        expect(bindingError("commandPalette", "Ctrl+Shift+V", {})).toBe("reserved")
    })
    it("uses overrides immediately and leaves the old binding alone", () => {
        useKeyboardSettingsStore.getState().setBinding("commandPalette", "Mod+Shift+J")
        const run = vi.fn()
        expect(dispatchAppShortcut(new KeyboardEvent("keydown", { key: "k", ctrlKey: true }), "commandPalette", run)).toBe(false)
        expect(dispatchAppShortcut(new KeyboardEvent("keydown", { key: "J", ctrlKey: true, shiftKey: true }), "commandPalette", run)).toBe(true)
        expect(run).toHaveBeenCalledOnce()
    })
    it("leaves composition, repeat, extra modifiers and terminal Ctrl input alone", () => {
        const run = vi.fn()
        for (const extra of [{ isComposing: true }, { repeat: true }, { altKey: true }]) {
            expect(dispatchAppShortcut(new KeyboardEvent("keydown", { key: "k", ctrlKey: true, ...extra }), "commandPalette", run)).toBe(false)
        }
        const terminal = document.createElement("div")
        terminal.className = "xterm"
        const input = terminal.appendChild(document.createElement("textarea"))
        input.addEventListener("keydown", event => dispatchAppShortcut(event, "commandPalette", run))
        input.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true }))
        expect(run).not.toHaveBeenCalled()
    })
    it("persists overrides and clears them when restoring defaults", () => {
        useKeyboardSettingsStore.getState().setBinding("commandPalette", "Mod+Shift+J")
        expect(loadKeyboardOverrides()).toEqual({ commandPalette: "Mod+Shift+J" })
        useKeyboardSettingsStore.getState().reset()
        expect(loadKeyboardOverrides()).toEqual({})
    })
    it("uses Command on macOS", () => {
        vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Macintosh")
        const run = vi.fn()
        expect(dispatchAppShortcut(new KeyboardEvent("keydown", { key: "k", metaKey: true }), "commandPalette", run)).toBe(true)
        expect(dispatchAppShortcut(new KeyboardEvent("keydown", { key: "k", ctrlKey: true }), "commandPalette", run)).toBe(false)
    })
})

it("preserves a valid reassignment across reload and rejects actual duplicate chords", () => {
    useKeyboardSettingsStore.getState().setBinding("newTerminal", "Ctrl+J")
    useKeyboardSettingsStore.getState().setBinding("commandPalette", "Ctrl+`")
    expect(loadKeyboardOverrides()).toEqual({ newTerminal: "Ctrl+J", commandPalette: "Ctrl+`" })
    localStorage.setItem(KEYBOARD_STORAGE_KEY, JSON.stringify({ newTerminal: "Ctrl+J", commandPalette: "Ctrl+J" }))
    expect(loadKeyboardOverrides()).toEqual({})
})
it("matches macOS Option-digit mode shortcuts even when event.key is a symbol", () => {
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Macintosh")
    const run = vi.fn()
    expect(dispatchAppShortcut(new KeyboardEvent("keydown", { key: "¡", code: "Digit1", metaKey: true, altKey: true }), "modeAde", run)).toBe(true)
    expect(run).toHaveBeenCalledOnce()
})
