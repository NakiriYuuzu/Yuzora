import { create } from "zustand"
import { isMacPlatform } from "@/lib/platform"

export const APP_COMMANDS = [
    { id: "commandPalette", defaultBinding: "Mod+K" },
    { id: "newTerminal", defaultBinding: "Ctrl+`" },
    { id: "settings", defaultBinding: "Mod+," },
    { id: "toggleSidebar", defaultBinding: "Mod+Shift+B" },
    { id: "toggleBrowser", defaultBinding: "Mod+Shift+P" },
    { id: "modeAde", defaultBinding: "Mod+Alt+1" },
    { id: "modeFiles", defaultBinding: "Mod+Alt+2" },
    { id: "modeGit", defaultBinding: "Mod+Alt+3" },
    { id: "modeDatabase", defaultBinding: "Mod+Alt+4" },
] as const
export type AppCommandId = typeof APP_COMMANDS[number]["id"]
type Overrides = Partial<Record<AppCommandId, string>>
export const KEYBOARD_STORAGE_KEY = "yuzora.keyboard.v1"

export function normalizeBinding(value: string): string | null {
    const parts = value.trim().split("+").map(p => p.trim())
    const key = parts.pop()?.toUpperCase()
    if (!key || !/^(?:[A-Z0-9`,.;/[\]\\-]|F(?:[1-9]|1[0-2]))$/.test(key)) return null
    const modifiers = parts.map(p => ({ mod: "Mod", ctrl: "Ctrl", shift: "Shift", alt: "Alt" })[p.toLowerCase()])
    if (!modifiers.length || modifiers.some(p => !p) || new Set(modifiers).size !== modifiers.length) return null
    if (modifiers.includes("Mod") && modifiers.includes("Ctrl")) return null
    return ["Mod", "Ctrl", "Alt", "Shift"].filter(p => modifiers.includes(p)).concat(key).join("+")
}

export function bindingLabel(binding: string, mac = isMacPlatform()): string {
    return binding.replace("Mod", mac ? "⌘" : "Ctrl").replace("Ctrl", mac ? "⌃" : "Ctrl").replace("Shift", mac ? "⇧" : "Shift").replace("Alt", mac ? "⌥" : "Alt")
}

function bindingValueError(value: string): "invalid" | "reserved" | null {
    const normalized = normalizeBinding(value)
    if (!normalized) return "invalid"
    // Protect editing, browser/OS and terminal protocol keys on both platforms.
    if (/^(Mod|Ctrl)\+(?:[ACFVXZYWQHNOPRSTUL]|Tab)$/.test(normalized) || normalized === "Alt+F4" || /^(Mod|Ctrl)\+Shift\+[ACVXYZ345]$/.test(normalized)) return "reserved"
    return null
}

export function bindingError(id: AppCommandId, value: string, overrides: Overrides): "invalid" | "reserved" | "conflict" | null {
    const invalid = bindingValueError(value)
    if (invalid) return invalid
    const normalized = normalizeBinding(value)!
    const canonical = (s: string) => s.replace("Mod+", "Ctrl+")
    if (APP_COMMANDS.some(c => c.id !== id && canonical(overrides[c.id] ?? c.defaultBinding) === canonical(normalized))) return "conflict"
    return null
}

export function loadKeyboardOverrides(): Overrides {
    try {
        const raw: unknown = JSON.parse(localStorage.getItem(KEYBOARD_STORAGE_KEY) ?? "{}")
        if (!raw || typeof raw !== "object") return {}
        const result: Overrides = {}
        for (const command of APP_COMMANDS) {
            const value = (raw as Record<string, unknown>)[command.id]
            if (typeof value === "string" && !bindingValueError(value)) result[command.id] = normalizeBinding(value)!
        }
        // Validate the complete candidate first: defaults must not erase a
        // valid persisted reassignment to a key another command has vacated.
        for (;;) {
            const conflicts = APP_COMMANDS.filter(c => result[c.id] && bindingError(c.id, result[c.id]!, result))
            if (!conflicts.length) break
            for (const command of conflicts) delete result[command.id]
        }
        return result
    } catch { return {} }
}

export const useKeyboardSettingsStore = create<{
    overrides: Overrides
    setBinding: (id: AppCommandId, value: string) => boolean
    reset: () => void
}>((set, get) => {
    const save = (overrides: Overrides) => {
        set({ overrides })
        try { localStorage.setItem(KEYBOARD_STORAGE_KEY, JSON.stringify(overrides)) } catch { /* Keep session preferences. */ }
    }
    return {
        overrides: loadKeyboardOverrides(),
        setBinding: (id, value) => {
            if (bindingError(id, value, get().overrides)) return false
            save({ ...get().overrides, [id]: normalizeBinding(value)! })
            return true
        },
        reset: () => save({}),
    }
})

export function dispatchAppShortcut(event: KeyboardEvent, id: AppCommandId, run: () => void): boolean {
    if (event.defaultPrevented || event.isComposing || event.keyCode === 229 || event.repeat || event.getModifierState("AltGraph")) return false
    const target = event.target instanceof Element ? event.target : null
    if (target?.closest('[role="dialog"], [data-shortcut-capture]')) return false
    const command = APP_COMMANDS.find(c => c.id === id)!
    const binding = useKeyboardSettingsStore.getState().overrides[id] ?? command.defaultBinding
    const parts = binding.split("+")
    const key = parts.pop()!
    const mac = isMacPlatform()
    const meta = parts.includes("Mod") && mac
    const ctrl = parts.includes("Ctrl") || (parts.includes("Mod") && !mac)
    if (event.metaKey !== meta || event.ctrlKey !== ctrl || event.altKey !== parts.includes("Alt") || event.shiftKey !== parts.includes("Shift") || (event.key.toUpperCase() !== key && !((parts.includes("Shift") || parts.includes("Alt")) && /^[0-9]$/.test(key) && event.code === `Digit${key}`))) return false
    // Plain Ctrl chords are terminal protocol input. App shortcuts do not intercept them.
    if (target?.closest('.xterm') && !event.metaKey) return false
    if (target?.closest('input, textarea, [contenteditable="true"]') && !target.closest('.cm-editor') && !event.metaKey) return false
    event.preventDefault()
    run()
    return true
}
