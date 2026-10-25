import { beforeEach, describe, expect, it, vi } from "vitest"
import { bindingError, dispatchAppShortcut, normalizeBinding, loadKeyboardOverrides, KEYBOARD_STORAGE_KEY, useKeyboardSettingsStore } from "./keyboardSettingsStore"

beforeEach(() => {
    const values = new Map<string, string>()
    vi.stubGlobal("localStorage", { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => values.set(key, value) })
    useKeyboardSettingsStore.setState({ overrides: {} })
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Windows")
})
describe("app shortcut bindings", () => {
    it("normalizes and dispatches tab navigation inside terminal inputs", () => {
        expect(normalizeBinding("ctrl+shift+tab")).toBe("Ctrl+Shift+Tab")
        const run = vi.fn()
        const terminal = document.createElement("div")
        terminal.className = "xterm"
        const input = terminal.appendChild(document.createElement("textarea"))
        input.addEventListener("keydown", event => dispatchAppShortcut(event, "nextTab", run))
        const event = new KeyboardEvent("keydown", { key: "Tab", ctrlKey: true, cancelable: true })
        input.dispatchEvent(event)
        expect(run).toHaveBeenCalledOnce()
        expect(event.defaultPrevented).toBe(true)
    })
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
    it("leaves composition, repeat, extra modifiers and non-terminal-safe Ctrl input alone", () => {
        const run = vi.fn()
        for (const extra of [{ isComposing: true }, { repeat: true }, { altKey: true }]) {
            expect(dispatchAppShortcut(new KeyboardEvent("keydown", { key: "k", ctrlKey: true, ...extra }), "commandPalette", run)).toBe(false)
        }
        const terminal = document.createElement("div")
        terminal.className = "xterm"
        const input = terminal.appendChild(document.createElement("textarea"))
        input.addEventListener("keydown", event => dispatchAppShortcut(event, "newTerminal", run))
        input.dispatchEvent(new KeyboardEvent("keydown", { key: "`", ctrlKey: true }))
        expect(run).not.toHaveBeenCalled()
    })
    it("dispatches terminal-safe Ctrl shortcuts inside terminals and inputs before they receive the key", () => {
        const run = vi.fn()
        const target = vi.fn()
        const capture = (event: KeyboardEvent) => { dispatchAppShortcut(event, "commandPalette", run) }
        window.addEventListener("keydown", capture, true)
        try {
            const terminal = document.body.appendChild(document.createElement("div"))
            terminal.className = "xterm"
            const xtermInput = terminal.appendChild(document.createElement("textarea"))
            xtermInput.addEventListener("keydown", target)
            const inTerminal = new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true, cancelable: true })
            xtermInput.dispatchEvent(inTerminal)
            expect(run).toHaveBeenCalledOnce()
            expect(inTerminal.defaultPrevented).toBe(true)
            expect(target).not.toHaveBeenCalled()
            const field = document.body.appendChild(document.createElement("input"))
            field.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true, cancelable: true }))
            expect(run).toHaveBeenCalledTimes(2)
            const dialog = document.body.appendChild(document.createElement("div"))
            dialog.setAttribute("role", "dialog")
            const dialogField = dialog.appendChild(document.createElement("input"))
            dialogField.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true, cancelable: true }))
            expect(run).toHaveBeenCalledTimes(2)
            terminal.remove(); field.remove(); dialog.remove()
        } finally {
            window.removeEventListener("keydown", capture, true)
        }
    })
    it("reserves Alt+Tab on Windows/Linux only and matches dead-key punctuation by physical key", () => {
        expect(bindingError("commandPalette", "Alt+Tab", {})).toBe("reserved")
        expect(bindingError("commandPalette", "Alt+Shift+Tab", {})).toBe("reserved")
        vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Macintosh")
        expect(bindingError("agentCycleNext", "Alt+Tab", {})).toBeNull()
        expect(bindingError("commandPalette", "Alt+Tab", {})).toBe("conflict")
        vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Windows")
        expect(bindingError("commandPalette", "Alt+`", {})).toBe("conflict")
        useKeyboardSettingsStore.getState().setBinding("commandPalette", "Alt+;")
        const run = vi.fn()
        expect(dispatchAppShortcut(new KeyboardEvent("keydown", { key: "Dead", code: "Semicolon", altKey: true }), "commandPalette", run)).toBe(true)
        expect(run).toHaveBeenCalledOnce()
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

it("matches macOS Option-letter shortcuts even when event.key is a symbol", () => {
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Macintosh")
    const run = vi.fn()
    expect(dispatchAppShortcut(new KeyboardEvent("keydown", { key: "∫", code: "KeyB", metaKey: true, altKey: true }), "toggleTools", run)).toBe(true)
    expect(run).toHaveBeenCalledOnce()
    expect(dispatchAppShortcut(new KeyboardEvent("keydown", { key: "∫", code: "KeyB", metaKey: true }), "toggleTools", run)).toBe(false)
})

it.each([false, true])("keeps macOS Command and Control handlers independent (reverse order=%s)", reverse => {
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Macintosh")
    expect(bindingError("settings", "Ctrl+K", {})).toBeNull()
    expect(useKeyboardSettingsStore.getState().setBinding("settings", "Ctrl+K")).toBe(true)
    const palette = vi.fn(), settings = vi.fn()
    const commands = reverse ? ["settings", "commandPalette"] as const : ["commandPalette", "settings"] as const
    const handler = (event: KeyboardEvent) => {
        for (const id of commands) dispatchAppShortcut(event, id, id === "settings" ? settings : palette)
    }
    const field = document.body.appendChild(document.createElement("input"))
    window.addEventListener("keydown", handler, true)
    try {
        field.dispatchEvent(new KeyboardEvent("keydown", { key: "k", metaKey: true, bubbles: true, cancelable: true }))
        expect(palette).toHaveBeenCalledTimes(1)
        expect(settings).not.toHaveBeenCalled()
        field.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true, cancelable: true }))
        expect(palette).toHaveBeenCalledTimes(1)
        expect(settings).toHaveBeenCalledTimes(1)
        field.dispatchEvent(new KeyboardEvent("keydown", { key: "k", metaKey: true, ctrlKey: true, bubbles: true, cancelable: true }))
        expect(palette).toHaveBeenCalledTimes(1)
        expect(settings).toHaveBeenCalledTimes(1)
    } finally { window.removeEventListener("keydown", handler, true); field.remove() }
})

it("keeps macOS modifiers distinct while rejecting actual duplicates", () => {
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Macintosh")
    expect(bindingError("settings", "Mod+K", {})).toBe("conflict")
    expect(bindingError("settings", "Mod+`", {})).toBeNull()
    expect(bindingError("settings", "Ctrl+`", {})).toBe("conflict")
    expect(bindingError("toggleSidebar", "Ctrl+K", { settings: "Ctrl+K" })).toBe("conflict")
    localStorage.setItem(KEYBOARD_STORAGE_KEY, JSON.stringify({ settings: "Ctrl+K", toggleSidebar: "Ctrl+K" }))
    expect(loadKeyboardOverrides()).toEqual({})
})

it.each(["Windows", "Linux"])("preserves Mod/Ctrl conflict semantics on %s", platform => {
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue(platform)
    expect(bindingError("settings", "Ctrl+K", {})).toBe("conflict")
    expect(bindingError("settings", "Mod+K", { commandPalette: "Ctrl+K" })).toBe("conflict")
    expect(useKeyboardSettingsStore.getState().setBinding("settings", "Ctrl+K")).toBe(false)
    expect(useKeyboardSettingsStore.getState().overrides).toEqual({})
})

it("reloads a macOS Control override and safely filters it on other platforms without rewriting storage", () => {
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Macintosh")
    expect(useKeyboardSettingsStore.getState().setBinding("settings", "Ctrl+K")).toBe(true)
    const stored = localStorage.getItem(KEYBOARD_STORAGE_KEY)
    expect(loadKeyboardOverrides()).toEqual({ settings: "Ctrl+K" })
    for (const platform of ["Windows", "Linux"]) {
        vi.spyOn(navigator, "userAgent", "get").mockReturnValue(platform)
        expect(loadKeyboardOverrides()).toEqual({})
        expect(localStorage.getItem(KEYBOARD_STORAGE_KEY)).toBe(stored)
    }
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Macintosh")
    expect(loadKeyboardOverrides()).toEqual({ settings: "Ctrl+K" })
})
