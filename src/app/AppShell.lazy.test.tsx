import { act, cleanup, render, screen } from "@testing-library/react"
import { afterEach, expect, it, vi } from "vitest"
import { AppShell } from "./AppShell"
import { uiInitialState, useUiStore } from "@/state/uiStore"
import { useDiffModalStore } from "@/state/diffModalStore"

const chunks = vi.hoisted(() => ({ loaded: vi.fn() }))
vi.mock("@/lib/platform", () => ({ isTauri: () => false, showsNativeTrafficLights: () => false, isWindowsPlatform: () => false, isMacPlatform: () => false }))
vi.mock("@/app/panels/EditorPanel", () => ({ EditorPanel: () => <input aria-label="Editor buffer" defaultValue="draft" /> }))
vi.mock("@/app/workbench/SpaceAgentSidebar", () => ({ SpaceAgentSidebar: () => null }))
vi.mock("@/app/workbench/WorkspaceToolsPanel", () => ({ WorkspaceToolsPanel: () => null }))
vi.mock("@/app/workbench/CommandPalette", () => ({ CommandPalette: () => null }))
vi.mock("@/app/workbench/ContextMenu", () => ({ ContextMenu: () => null }))
vi.mock("@/app/workbench/ProjectEditorPopover", () => ({ ProjectEditorPopover: () => null }))
vi.mock("@/app/workbench/StatusBar", () => ({ StatusBar: () => null }))
vi.mock("@/app/panels/GitPanel", () => {
    chunks.loaded("git")
    return { GitPanel: () => <div>Git surface</div> }
})
vi.mock("@/app/panels/DatabasePanel", () => {
    chunks.loaded("database")
    return { DatabasePanel: () => <div>Database surface</div> }
})
vi.mock("@/app/workbench/DatabaseNavContent", () => {
    chunks.loaded("database-nav")
    return { DatabaseNavContent: () => <div>Database navigation</div> }
})
vi.mock("@/app/workbench/SettingsDialog", () => {
    chunks.loaded("settings")
    return { SettingsDialog: ({ open, initialSection }: { open: boolean; initialSection?: string }) => open ? <div role="dialog">Settings {initialSection}</div> : null }
})
vi.mock("@/workbench/git/DiffModal", () => {
    chunks.loaded("diff")
    return { DiffModal: () => useDiffModalStore(s => s.open) ? <div role="dialog">Diff surface</div> : null }
})

afterEach(() => {
    cleanup()
    useUiStore.setState(uiInitialState)
    useDiffModalStore.getState().close()
})

it("keeps heavy surfaces out of initial ADE and opens them through the shared action stores", async () => {
    useUiStore.setState(uiInitialState)
    useDiffModalStore.getState().close()
    render(<AppShell />)
    const editor = screen.getByRole("textbox", { name: "Editor buffer" })
    expect(chunks.loaded).not.toHaveBeenCalled()

    // Shortcuts, palette and context menus all dispatch to these same stores.
    act(() => useUiStore.getState().openSettings("editor"))
    expect(await screen.findByText("Settings editor")).toBeInTheDocument()
    expect(chunks.loaded.mock.calls).toEqual([["settings"]])
    act(() => useUiStore.getState().setSettingsOpen(false))
    act(() => useDiffModalStore.getState().openText("a.ts", { kind: "full", content: "old" }, { kind: "full", content: "new" }))
    expect(await screen.findByText("Diff surface")).toBeInTheDocument()
    act(() => useDiffModalStore.getState().close())

    act(() => useUiStore.getState().setMode("database"))
    const database = await screen.findByText("Database surface")
    expect(await screen.findByText("Database navigation")).toBeInTheDocument()
    act(() => useUiStore.getState().setMode("git"))
    expect(await screen.findByText("Git surface")).toBeInTheDocument()
    act(() => useUiStore.getState().setMode("ade"))
    expect(screen.getByRole("textbox", { name: "Editor buffer" })).toBe(editor)
    expect(editor).toHaveValue("draft")
    expect(database.closest("[hidden]")).not.toBeNull()
    expect(chunks.loaded.mock.calls.flat().sort()).toEqual(["database", "database-nav", "diff", "git", "settings"])
})
