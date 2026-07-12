import { beforeEach, describe, expect, it, vi } from "vitest"

import { uiInitialState, useUiStore } from "./uiStore"
import {
    gitChangeRows,
    isGitToggleModifier,
    rollbackTargetsFromKeys
} from "@/workbench/git/gitChangeSelection"
import type { GitStatus } from "@/lib/types"

function status(over: Partial<GitStatus> = {}): GitStatus {
    return {
        branch: "main",
        headOid: "0".repeat(40),
        detached: false,
        upstream: null,
        ahead: 0,
        behind: 0,
        staged: [],
        unstaged: [],
        untracked: [],
        conflicted: [],
        inProgress: null,
        ...over
    }
}

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

    it("Git change single/toggle/range selection keeps an independent primary and anchor", () => {
        const rows = gitChangeRows(status({
            staged: [{ path: "a.ts", origPath: null, status: "M" }],
            unstaged: [{ path: "b.ts", origPath: null, status: "M" }],
            untracked: ["c.ts"]
        }))
        const store = useUiStore.getState()
        store.selectGitChange(rows[0], rows, "single")
        store.selectGitChange(rows[1], rows, "toggle")
        expect(useUiStore.getState().gitChangeSelection.map((row) => row.path)).toEqual([
            "a.ts",
            "b.ts"
        ])
        expect(useUiStore.getState().gitChangePrimary?.path).toBe("b.ts")
        expect(useUiStore.getState().gitSelectedPath).toBe("b.ts")

        useUiStore.getState().selectGitChange(rows[2], rows, "range")
        expect(useUiStore.getState().gitChangeSelection.map((row) => row.path)).toEqual([
            "b.ts",
            "c.ts"
        ])
        expect(useUiStore.getState().gitChangeAnchor?.path).toBe("b.ts")
        expect(useUiStore.getState().gitChangePrimary?.path).toBe("c.ts")
    })

    it("partially-staged rows have separate identities and remap to one surviving side", () => {
        const before = gitChangeRows(status({
            staged: [{ path: "partial.ts", origPath: null, status: "M" }],
            unstaged: [{ path: "partial.ts", origPath: null, status: "M" }]
        }))
        useUiStore.getState().selectGitChange(before[0], before, "single")
        useUiStore.getState().selectGitChange(before[1], before, "toggle")
        expect(useUiStore.getState().gitChangeSelection).toHaveLength(2)

        const after = gitChangeRows(status({
            staged: [{ path: "partial.ts", origPath: null, status: "M" }]
        }))
        useUiStore.getState().reconcileGitChangeSelection(after, { "partial.ts": true })
        expect(useUiStore.getState().gitChangeSelection).toEqual([after[0]])
        expect(useUiStore.getState().gitChangePrimary).toEqual(after[0])
        expect(useUiStore.getState().gitSelectedStaged).toBe(true)
    })

    it("right-click selection keeps an already-selected row, but replaces selection for an unselected row", () => {
        const rows = gitChangeRows(status({ untracked: ["a.ts", "b.ts", "c.ts"] }))
        useUiStore.getState().selectGitChange(rows[0], rows, "single")
        useUiStore.getState().selectGitChange(rows[1], rows, "toggle")
        useUiStore.getState().ensureGitChangeContextSelection(rows[0])
        expect(useUiStore.getState().gitChangeSelection).toHaveLength(2)

        useUiStore.getState().ensureGitChangeContextSelection(rows[2])
        expect(useUiStore.getState().gitChangeSelection).toEqual([rows[2]])
        expect(useUiStore.getState().gitChangePrimary).toEqual(rows[2])
    })

    it("uses Command on macOS and Control on other platforms for additive selection", () => {
        const platform = vi.spyOn(navigator, "platform", "get")
        platform.mockReturnValue("MacIntel")
        expect(isGitToggleModifier({ metaKey: true, ctrlKey: false })).toBe(true)
        expect(isGitToggleModifier({ metaKey: false, ctrlKey: true })).toBe(false)

        platform.mockReturnValue("Linux x86_64")
        expect(isGitToggleModifier({ metaKey: false, ctrlKey: true })).toBe(true)
        expect(isGitToggleModifier({ metaKey: true, ctrlKey: false })).toBe(false)
        platform.mockRestore()
    })

    it("classifies an unstaged A snapshot as added for the rollback contract", () => {
        const rows = gitChangeRows(status({
            unstaged: [{ path: "intent-to-add.ts", origPath: null, status: "A" }]
        }))
        expect(rows[0].classification).toBe("added")
        expect(rollbackTargetsFromKeys(rows)).toEqual([{
            path: "intent-to-add.ts",
            classification: {
                kind: "added",
                stagedStatus: null,
                unstagedStatus: "A"
            }
        }])
    })
})
