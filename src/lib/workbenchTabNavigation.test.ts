import { afterEach, expect, it, vi } from "vitest"
import { useWorkspaceStore, type TabInfo } from "@/state/workspaceStore"
import { useHerdrStore } from "@/state/herdrStore"
import { useUiStore } from "@/state/uiStore"
import type { HerdrSnapshot, HerdrTabInfo } from "./herdrTypes"
import { activateWorkbenchTab, navigateWorkbenchTabs, tabNavigationIndex, visibleWorkbenchTabs } from "./workbenchTabNavigation"
import { registerTerminalFocusTarget } from "@/terminal/terminalFocus"

const originalRuntime = useHerdrStore.getState()
afterEach(() => { useHerdrStore.setState(originalRuntime, true); vi.restoreAllMocks(); vi.useRealTimers(); document.body.innerHTML = "" })
const file = (path: string, pinned = false): TabInfo => ({ path, name: path, pinned, dirty: false, externallyModified: false })

it("selects numbered tabs and wraps adjacent navigation", () => {
    expect(tabNavigationIndex(0, 0, { direction: 1 })).toBeNull()
    expect(tabNavigationIndex(0, 1, { direction: 1 })).toBe(0)
    expect(tabNavigationIndex(0, 8, { index: 8 })).toBeNull()
    expect(tabNavigationIndex(0, 12, { index: 8 })).toBe(8)
    expect(tabNavigationIndex(8, 9, { direction: 1 })).toBe(0)
    expect(tabNavigationIndex(0, 9, { direction: -1 })).toBe(8)
})

it.each(["existing", "mounting"])("focuses the %s Terminal after selecting its already-active tab", async (readiness) => {
    vi.useFakeTimers()
    const path = "yuzora://herdr/selected"
    const tab: TabInfo = { ...file(path), kind: "herdr-terminal", herdrTabId: "selected" }
    const runtimeTab = { id: "selected" } as HerdrTabInfo
    useWorkspaceStore.setState({ workspacePath: "/w", activeGroupIndex: 0, groups: [{ activePath: path, tabs: [tab] }] })
    vi.spyOn(useHerdrStore.getState(), "activateTab").mockResolvedValue({ ok: true })
    const button = document.body.appendChild(document.createElement("button"))
    const input = document.body.appendChild(document.createElement("textarea"))
    input.className = "xterm-helper-textarea"
    button.focus()
    let ready = readiness === "existing"
    const release = registerTerminalFocusTarget("selected", { pagePath: path, active: () => ready, focus: () => input.focus() })
    try {
        await activateWorkbenchTab(0, tab, runtimeTab)
        vi.advanceTimersByTime(1)
        ready = true
        vi.runAllTimers()
        expect(document.activeElement).toBe(input)
    } finally { release() }
})

it("does not focus a late successful activation after newer tab selection", async () => {
    vi.useFakeTimers()
    const path = "yuzora://herdr/late"
    const tab: TabInfo = { ...file(path), kind: "herdr-terminal", herdrTabId: "late" }
    useWorkspaceStore.setState({ workspacePath: "/w", activeGroupIndex: 0, groups: [{ activePath: "/file", tabs: [file("/file"), tab] }] })
    let finish!: (result: Awaited<ReturnType<typeof originalRuntime.activateTab>>) => void
    vi.spyOn(useHerdrStore.getState(), "activateTab").mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
    const focus = vi.fn()
    const release = registerTerminalFocusTarget("late", { pagePath: path, active: () => true, focus })
    try {
        const pending = activateWorkbenchTab(0, tab, { id: "late" } as HerdrTabInfo)
        await activateWorkbenchTab(0, file("/file"))
        finish({ ok: true })
        await pending
        vi.runAllTimers()
        expect(focus).not.toHaveBeenCalled()
    } finally { release() }
})

it("uses pinned-first visible order only in the active split group", async () => {
    useUiStore.setState({ mode: "files" })
    useWorkspaceStore.setState({ workspacePath: "/w", activeGroupIndex: 1, groups: [
        { activePath: "left", tabs: [file("left")] },
        { activePath: "a", tabs: [file("a"), file("pinned", true), ...Array.from({ length: 9 }, (_, i) => file(`file${i}`))] },
    ] })
    await navigateWorkbenchTabs({ index: 0 })
    expect(useWorkspaceStore.getState().groups.map(group => group.activePath)).toEqual(["left", "pinned"])
    await navigateWorkbenchTabs({ index: 8 })
    expect(useWorkspaceStore.getState().groups[1].activePath).toBe("file6")
    await navigateWorkbenchTabs({ direction: 1 })
    expect(useWorkspaceStore.getState().groups[1].activePath).toBe("file7")
})

it("filters other HERDR sessions/spaces and rolls back only the failed current activation", async () => {
    const runtimeTab: HerdrTabInfo = { id: "tab", workspaceId: "space", label: "Terminal", order: 0, paneCount: 1, status: "idle", active: false, focused: false }
    const terminal: TabInfo = { ...file("terminal"), kind: "herdr-terminal", herdrSessionId: "session", herdrWorkspaceId: "space", herdrTabId: "tab" }
    useWorkspaceStore.setState({ workspacePath: "/w", activeGroupIndex: 0, groups: [{ activePath: "file", tabs: [file("file"), terminal, { ...terminal, path: "other-session", herdrSessionId: "other" }, { ...terminal, path: "other-space", herdrWorkspaceId: "other" }] }] })
    useHerdrStore.setState({ selectedSessionName: "session", selectedSpaceId: "space", snapshot: { tabs: [runtimeTab] } as HerdrSnapshot })
    expect(visibleWorkbenchTabs(0).map(tab => tab.path)).toEqual(["file", "terminal"])
    const activate = vi.spyOn(useHerdrStore.getState(), "activateTab").mockResolvedValue({ ok: false, error: "unsupported" })
    await activateWorkbenchTab(0, terminal)
    expect(activate).toHaveBeenCalledWith(runtimeTab)
    expect(useWorkspaceStore.getState().groups[0].activePath).toBe("file")
    let finish!: (result: Awaited<ReturnType<typeof originalRuntime.activateTab>>) => void
    activate.mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
    const pending = activateWorkbenchTab(0, terminal)
    await activateWorkbenchTab(0, file("file"))
    finish({ ok: false, error: "unsupported" })
    await pending
    expect(useWorkspaceStore.getState().groups[0].activePath).toBe("file")
})
