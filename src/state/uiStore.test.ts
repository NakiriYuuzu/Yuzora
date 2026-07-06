import { beforeEach, describe, expect, it } from "vitest"

import { uiInitialState, useUiStore } from "./uiStore"

describe("uiStore", () => {
    beforeEach(() => {
        useUiStore.setState(uiInitialState)
    })

    it("openDiffInGitMode selects file and switches mode", () => {
        useUiStore.getState().openDiffInGitMode("/w/a.ts")
        const s = useUiStore.getState()
        expect(s.mode).toBe("git")
        expect(s.gitSelectedPath).toBe("/w/a.ts")
        expect(s.gitSelectedStaged).toBe(false)
    })
    it("resolver open/close roundtrip", () => {
        useUiStore.getState().openResolver("/w/b.ts")
        expect(useUiStore.getState().resolverPath).toBe("/w/b.ts")
        useUiStore.getState().closeResolver()
        expect(useUiStore.getState().resolverPath).toBe(null)
    })
    it("openSettings targets a section and language", () => {
        useUiStore.getState().openSettings("lsp", "python")
        const s = useUiStore.getState()
        expect(s.settingsOpen).toBe(true)
        expect(s.settingsSection).toBe("lsp")
        expect(s.settingsLanguage).toBe("python")
    })
    it("openSettings targets a logs source", () => {
        useUiStore.getState().openSettings("logs", { source: "dev_server" })
        const s = useUiStore.getState()
        expect(s.settingsOpen).toBe(true)
        expect(s.settingsSection).toBe("logs")
        expect(s.settingsLanguage).toBe(null)
        expect(s.settingsLogSource).toBe("dev_server")
    })
    it("openSettings without arguments opens with no target", () => {
        useUiStore.getState().openSettings()
        const s = useUiStore.getState()
        expect(s.settingsOpen).toBe(true)
        expect(s.settingsSection).toBe(null)
        expect(s.settingsLanguage).toBe(null)
        expect(s.settingsLogSource).toBe(null)
    })
    it("setSettingsOpen(false) closes the dialog", () => {
        useUiStore.getState().openSettings("lsp", "python")
        useUiStore.getState().setSettingsOpen(false)
        expect(useUiStore.getState().settingsOpen).toBe(false)
    })
    it("openSettings bumps settingsNonce on every call (re-target while open)", () => {
        useUiStore.getState().openSettings("lsp", "python")
        const n1 = useUiStore.getState().settingsNonce
        useUiStore.getState().openSettings("lsp", "python")
        expect(useUiStore.getState().settingsNonce).toBe(n1 + 1)
    })
    it("setTraceEnabled toggles in-memory trace state (default off)", () => {
        expect(useUiStore.getState().traceEnabled).toBe(false)
        useUiStore.getState().setTraceEnabled(true)
        expect(useUiStore.getState().traceEnabled).toBe(true)
    })
    it("toggleTerminal flips terminal drawer visibility from the initial closed state", () => {
        expect(useUiStore.getState().terminalOpen).toBe(false)
        useUiStore.getState().toggleTerminal()
        expect(useUiStore.getState().terminalOpen).toBe(true)
        useUiStore.getState().toggleTerminal()
        expect(useUiStore.getState().terminalOpen).toBe(false)
    })
    it("togglePreview flips preview dock visibility from the initial closed state", () => {
        expect(useUiStore.getState().previewOpen).toBe(false)
        useUiStore.getState().togglePreview()
        expect(useUiStore.getState().previewOpen).toBe(true)
        useUiStore.getState().togglePreview()
        expect(useUiStore.getState().previewOpen).toBe(false)
    })
})
