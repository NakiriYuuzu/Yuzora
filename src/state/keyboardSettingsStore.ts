import { create } from "zustand"
import { isMacPlatform } from "@/lib/platform"
import type { PreviewShortcutBinding } from "@/lib/previewTypes"

/**
 * `terminalSafe` commands also fire while focus is in a terminal or text input
 * (VS Code's commandsToSkipShell). Their key is taken from the terminal on
 * Windows/Linux, where Mod is Ctrl. `macDefaultBinding` overrides the default
 * on macOS only.
 */
export interface AppCommandSpec {
    readonly id: string
    readonly defaultBinding: string
    readonly macDefaultBinding?: string
    readonly terminalSafe?: boolean
}
export const APP_COMMANDS = [
    { id: "commandPalette", defaultBinding: "Mod+K", terminalSafe: true },
    { id: "newTerminal", defaultBinding: "Ctrl+`" },
    { id: "settings", defaultBinding: "Mod+,", terminalSafe: true },
    { id: "toggleSidebar", defaultBinding: "Mod+Shift+B", terminalSafe: true },
    { id: "toggleTools", defaultBinding: "Mod+Alt+B", terminalSafe: true },
    { id: "toggleBrowser", defaultBinding: "Mod+Shift+P", terminalSafe: true },
    { id: "modeAde", defaultBinding: "Mod+Alt+1", terminalSafe: true },
    { id: "modeFiles", defaultBinding: "Mod+Alt+2", terminalSafe: true },
    { id: "modeGit", defaultBinding: "Mod+Alt+3", terminalSafe: true },
    { id: "modeDatabase", defaultBinding: "Mod+Alt+4", terminalSafe: true },
    { id: "tab1", defaultBinding: "Mod+1" },
    { id: "tab2", defaultBinding: "Mod+2" },
    { id: "tab3", defaultBinding: "Mod+3" },
    { id: "tab4", defaultBinding: "Mod+4" },
    { id: "tab5", defaultBinding: "Mod+5" },
    { id: "tab6", defaultBinding: "Mod+6" },
    { id: "tab7", defaultBinding: "Mod+7" },
    { id: "tab8", defaultBinding: "Mod+8" },
    { id: "tab9", defaultBinding: "Mod+9" },
    { id: "nextTab", defaultBinding: "Ctrl+Tab" },
    { id: "previousTab", defaultBinding: "Ctrl+Shift+Tab" },
    { id: "toggleSidebarView", defaultBinding: "Mod+Shift+E", terminalSafe: true },
    { id: "agent1", defaultBinding: "Alt+1", terminalSafe: true },
    { id: "agent2", defaultBinding: "Alt+2", terminalSafe: true },
    { id: "agent3", defaultBinding: "Alt+3", terminalSafe: true },
    { id: "agent4", defaultBinding: "Alt+4", terminalSafe: true },
    { id: "agent5", defaultBinding: "Alt+5", terminalSafe: true },
    { id: "agent6", defaultBinding: "Alt+6", terminalSafe: true },
    { id: "agent7", defaultBinding: "Alt+7", terminalSafe: true },
    { id: "agent8", defaultBinding: "Alt+8", terminalSafe: true },
    { id: "agent9", defaultBinding: "Alt+9", terminalSafe: true },
    { id: "agentCycleNext", defaultBinding: "Alt+`", macDefaultBinding: "Alt+Tab", terminalSafe: true },
    { id: "agentCyclePrevious", defaultBinding: "Alt+Shift+`", macDefaultBinding: "Alt+Shift+Tab", terminalSafe: true },
] as const satisfies readonly AppCommandSpec[]
export type AppCommandId = typeof APP_COMMANDS[number]["id"]
function commandSpec(id: AppCommandId): AppCommandSpec {
    return APP_COMMANDS.find(c => c.id === id)!
}
/** Platform default binding; macOS may use a different chord. */
export function defaultBindingFor(id: AppCommandId, mac = isMacPlatform()): string {
    const spec = commandSpec(id)
    return (mac && spec.macDefaultBinding) || spec.defaultBinding
}
export function isTerminalSafeCommand(id: AppCommandId): boolean {
    return commandSpec(id).terminalSafe === true
}
export function effectiveBinding(id: AppCommandId, overrides: Overrides): string {
    return overrides[id] ?? defaultBindingFor(id)
}
export function tabShortcutBindings(): PreviewShortcutBinding[] {
    const mac = isMacPlatform()
    return APP_COMMANDS.filter(command => /^(tab[1-9]|nextTab|previousTab)$/.test(command.id)).map(command => {
        const parts = (useKeyboardSettingsStore.getState().overrides[command.id] ?? defaultBindingFor(command.id, mac)).split("+")
        return { id: command.id, key: parts.pop()!.toUpperCase(), ctrl: parts.includes("Ctrl") || (parts.includes("Mod") && !mac), meta: parts.includes("Mod") && mac, alt: parts.includes("Alt"), shift: parts.includes("Shift") }
    })
}
type Overrides = Partial<Record<AppCommandId, string>>
export const KEYBOARD_STORAGE_KEY = "yuzora.keyboard.v1"

export function normalizeBinding(value: string): string | null {
    const parts = value.trim().split("+").map(p => p.trim())
    const rawKey = parts.pop()?.toUpperCase()
    const key = rawKey === "TAB" ? "Tab" : rawKey
    if (!key || !/^(?:[A-Z0-9`,.;/[\]\\-]|Tab|F(?:[1-9]|1[0-2]))$/.test(key)) return null
    const modifiers = parts.map(p => ({ mod: "Mod", ctrl: "Ctrl", shift: "Shift", alt: "Alt" })[p.toLowerCase()])
    if (!modifiers.length || modifiers.some(p => !p) || new Set(modifiers).size !== modifiers.length) return null
    if (modifiers.includes("Mod") && modifiers.includes("Ctrl")) return null
    return ["Mod", "Ctrl", "Alt", "Shift"].filter(p => modifiers.includes(p)).concat(key).join("+")
}

export function bindingLabel(binding: string, mac = isMacPlatform()): string {
    return binding.replace("Mod", mac ? "⌘" : "Ctrl").replace("Ctrl", mac ? "⌃" : "Ctrl").replace("Shift", mac ? "⇧" : "Shift").replace("Alt", mac ? "⌥" : "Alt")
}

function bindingValueError(value: string, mac = isMacPlatform()): "invalid" | "reserved" | null {
    const normalized = normalizeBinding(value)
    if (!normalized) return "invalid"
    // Protect editing, browser/OS and terminal protocol keys on both platforms.
    if (/^(Mod|Ctrl)\+[ACFVXZYWQHNOPRSTUL]$/.test(normalized) || normalized === "Alt+F4" || /^(Mod|Ctrl)\+Shift\+[ACVXYZ345]$/.test(normalized)) return "reserved"
    // Windows and Linux desktops own Alt+Tab; the app never receives it.
    if (!mac && /^Alt\+(Shift\+)?Tab$/.test(normalized)) return "reserved"
    return null
}

export function bindingError(id: AppCommandId, value: string, overrides: Overrides): "invalid" | "reserved" | "conflict" | null {
    const invalid = bindingValueError(value)
    if (invalid) return invalid
    const normalized = normalizeBinding(value)!
    const canonical = (s: string) => s.replace("Mod+", "Ctrl+")
    if (APP_COMMANDS.some(c => c.id !== id && canonical(overrides[c.id] ?? defaultBindingFor(c.id)) === canonical(normalized))) return "conflict"
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

// macOS Option turns B into ∫ and 1 into ¡, and some layouts make ` a dead
// key, so Alt/Shift chords fall back to the physical key.
const PHYSICAL_PUNCTUATION: Record<string, string> = {
    "`": "Backquote", ",": "Comma", ".": "Period", ";": "Semicolon", "/": "Slash",
    "[": "BracketLeft", "]": "BracketRight", "\\": "Backslash", "-": "Minus", TAB: "Tab",
}
function physicalKeyCode(key: string): string | null {
    if (/^[0-9]$/.test(key)) return `Digit${key}`
    if (/^[A-Z]$/.test(key)) return `Key${key}`
    return PHYSICAL_PUNCTUATION[key] ?? null
}

export function dispatchAppShortcut(event: KeyboardEvent, id: AppCommandId, run: () => void): boolean {
    if (event.defaultPrevented || event.isComposing || event.keyCode === 229 || event.repeat || event.getModifierState("AltGraph")) return false
    const target = event.target instanceof Element ? event.target : null
    if (target?.closest('[role="dialog"], [role="alertdialog"], dialog[open], [data-shortcut-capture]')) return false
    if (document.querySelector('[aria-modal="true"]:not([data-state="closed"]), dialog[open]')) return false
    const mac = isMacPlatform()
    const binding = useKeyboardSettingsStore.getState().overrides[id] ?? defaultBindingFor(id, mac)
    const parts = binding.split("+")
    const key = parts.pop()!.toUpperCase()
    const meta = parts.includes("Mod") && mac
    const ctrl = parts.includes("Ctrl") || (parts.includes("Mod") && !mac)
    if (event.metaKey !== meta || event.ctrlKey !== ctrl || event.altKey !== parts.includes("Alt") || event.shiftKey !== parts.includes("Shift") || (event.key.toUpperCase() !== key && !((parts.includes("Shift") || parts.includes("Alt")) && event.code === physicalKeyCode(key)))) return false
    // Plain Ctrl chords are terminal protocol input. Only terminal-safe app
    // commands intercept them; the terminal no longer receives those keys.
    const tabNavigation = id === "nextTab" || id === "previousTab" || /^tab[1-9]$/.test(id)
    const terminalSafe = isTerminalSafeCommand(id)
    if (!terminalSafe) {
        if (target?.closest('.xterm') && !event.metaKey && !tabNavigation) return false
        if (target?.closest('input, textarea, [contenteditable="true"]') && !target.closest('.cm-editor') && !event.metaKey && !tabNavigation) return false
    }
    event.preventDefault()
    // Dispatched from capture listeners: stop xterm from also emitting the chord.
    if (tabNavigation || terminalSafe) event.stopPropagation()
    run()
    return true
}
