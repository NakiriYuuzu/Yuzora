import { cleanup, fireEvent, render, screen, within } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { KeyboardSettings } from "./KeyboardSettings"
import i18n from "@/lib/i18n"
import { loadKeyboardOverrides, useKeyboardSettingsStore } from "@/state/keyboardSettingsStore"

beforeEach(() => {
    const values = new Map<string, string>()
    vi.stubGlobal("localStorage", { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => values.set(key, value) })
    useKeyboardSettingsStore.setState({ overrides: {} })
})
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals() })

it.each(["Macintosh", "Windows", "Linux"])("validates and applies a Control binding using the %s client platform", platform => {
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue(platform)
    const view = render(<KeyboardSettings />)
    const settings = screen.getByLabelText(i18n.t("commands.settings", { ns: "editorPreferences" }))
    fireEvent.change(settings, { target: { value: "Ctrl+K" } })
    const row = within(settings.closest(".settings-shortcut-row") as HTMLElement)
    const apply = row.getByRole("button", { name: i18n.t("apply", { ns: "editorPreferences" }) })
    if (platform === "Macintosh") {
        expect(settings).toHaveAttribute("aria-invalid", "false")
        expect(apply).toBeEnabled()
        fireEvent.click(apply)
        expect(useKeyboardSettingsStore.getState().overrides).toEqual({ settings: "Ctrl+K" })
        expect(screen.getByText("⌃+K")).toBeInTheDocument()
        expect(screen.getByText("⌘+K")).toBeInTheDocument()
        view.unmount()
        useKeyboardSettingsStore.setState({ overrides: loadKeyboardOverrides() })
        render(<KeyboardSettings />)
        expect(screen.getByLabelText(i18n.t("commands.settings", { ns: "editorPreferences" }))).toHaveValue("Ctrl+K")
    } else {
        expect(settings).toHaveAttribute("aria-invalid", "true")
        expect(apply).toBeDisabled()
        expect(row.getByText(i18n.t("errors.conflict", { ns: "editorPreferences" }))).toBeInTheDocument()
        expect(useKeyboardSettingsStore.getState().overrides).toEqual({})
    }
})

it("shows a command whose default key a persisted binding took as unbound, without an error", () => {
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Windows")
    useKeyboardSettingsStore.setState({ overrides: { toggleBrowser: "Mod+Shift+F" } })
    render(<KeyboardSettings />)
    const search = screen.getByLabelText(i18n.t("commands.workspaceSearch", { ns: "editorPreferences" }))
    expect(search).toHaveValue("")
    expect(search).toHaveAttribute("aria-invalid", "false")
    const row = within(search.closest(".settings-shortcut-row") as HTMLElement)
    expect(row.getByText(i18n.t("unbound", { ns: "editorPreferences" }))).toBeInTheDocument()
    expect(screen.getByLabelText(i18n.t("commands.toggleBrowser", { ns: "editorPreferences" }))).toHaveValue("Mod+Shift+F")
})
