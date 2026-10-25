import { useState } from "react"
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { CommandPalette } from "./CommandPalette"
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog"
import { WorkbenchFocusBridge } from "@/workbench/WorkbenchFocusBridge"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore"
import { uiInitialState, useUiStore } from "@/state/uiStore"
import type { SearchEvent } from "@/lib/types"

const mocks = vi.hoisted(() => ({ search: vi.fn(), terminal: vi.fn() }))
vi.mock("@/lib/ipc", async importOriginal => ({
    ...await importOriginal<typeof import("@/lib/ipc")>(),
    searchWorkspace: (...args: unknown[]) => mocks.search(...args),
}))
vi.mock("@/terminal/openNewTerminalTab", () => ({ openNewTerminalTab: () => mocks.terminal() }))

const owners: HTMLElement[] = []
beforeEach(() => {
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Windows")
    mocks.search.mockReset().mockResolvedValue(undefined)
    mocks.terminal.mockReset().mockResolvedValue(undefined)
    useUiStore.setState(uiInitialState)
    useHerdrStore.setState({ ...herdrInitialState, selectedSessionName: "default", selectedSpaceId: "space", attentionByKey: new Map(), attachments: new Map() })
    useWorkspaceStore.setState({ workspacePath: "/fixture", activeGroupIndex: 0, groups: [{ tabs: [], activePath: "/fixture/a.txt" }], pendingReveal: null })
})
afterEach(async () => {
    cleanup()
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)) })
    for (const owner of owners.splice(0)) owner.remove()
    vi.restoreAllMocks()
})

function Harness({ bridge = false }: { bridge?: boolean }) {
    const [open, setOpen] = useState(false)
    const [settings, setSettings] = useState(false)
    return <>
        {bridge && <WorkbenchFocusBridge />}
        <CommandPalette open={open} onOpenChange={setOpen} onSelectMode={mode => useUiStore.getState().setMode(mode)} onOpenSettings={() => setSettings(true)} />
        <Dialog open={settings} onOpenChange={setSettings}>
            <DialogContent><DialogTitle>Destination settings</DialogTitle><input aria-label="Settings destination" /></DialogContent>
        </Dialog>
    </>
}
function ownerElement(kind = "input") {
    const owner = document.createElement(kind === "editor" ? "div" : kind === "terminal" ? "textarea" : "input")
    if (kind === "editor") { owner.className = "cm-content"; owner.contentEditable = "true"; owner.tabIndex = 0 }
    if (kind === "terminal") owner.className = "xterm-helper-textarea"
    document.body.append(owner)
    owners.push(owner)
    return owner
}
const toggle = (target: HTMLElement) => fireEvent.keyDown(target, { key: "k", ctrlKey: true })
const drainClose = async () => { await act(async () => { await new Promise(resolve => setTimeout(resolve, 0)) }) }
async function openFrom(owner: HTMLElement) {
    owner.focus()
    expect(document.activeElement).toBe(owner)
    toggle(owner)
    const input = await screen.findByPlaceholderText("Search files, run a command…")
    await waitFor(() => expect(input).toHaveFocus())
    return input
}
async function dismiss(input: HTMLElement, method = "escape") {
    if (method === "toggle") toggle(input)
    else if (method === "outside") {
        const overlay = document.querySelector('[data-slot="dialog-overlay"]')!
        fireEvent.pointerDown(overlay, { button: 0, pointerType: "mouse", isPrimary: true })
        fireEvent.click(overlay)
    } else fireEvent.keyDown(input, { key: "Escape" })
    await waitFor(() => expect(screen.queryByPlaceholderText("Search files, run a command…")).not.toBeInTheDocument())
    await drainClose()
}

it.each(["input", "editor", "terminal"])("restores the %s owner after Escape with the real workbench focus bridge", async kind => {
    render(<Harness bridge />)
    const owner = ownerElement(kind)
    if (owner instanceof HTMLInputElement) { owner.value = "keep selection"; owner.setSelectionRange(2, 5) }
    const input = await openFrom(owner)
    await dismiss(input)
    expect(owner).toHaveFocus()
    if (owner instanceof HTMLInputElement) expect([owner.selectionStart, owner.selectionEnd]).toEqual([2, 5])
})

it("restores the owner after outside dismissal", async () => {
    render(<Harness />)
    const owner = ownerElement()
    const input = await openFrom(owner)
    await dismiss(input, "outside")
    expect(owner).toHaveFocus()
})

it("preserves modal shortcut isolation and restores after Escape", async () => {
    render(<Harness />)
    const owner = ownerElement()
    const input = await openFrom(owner)
    toggle(input)
    expect(input).toBeInTheDocument()
    expect(input).toHaveFocus()
    await dismiss(input)
    expect(owner).toHaveFocus()
})

it.each(["removed", "disabled", "hidden", "inert"])("does not restore a %s owner", async condition => {
    render(<Harness />)
    const owner = ownerElement()
    const input = await openFrom(owner)
    const focus = vi.spyOn(owner, "focus")
    if (condition === "removed") owner.remove()
    if (condition === "disabled") (owner as HTMLInputElement).disabled = true
    if (condition === "hidden") owner.style.display = "none"
    if (condition === "inert") owner.setAttribute("inert", "")
    await dismiss(input)
    expect(focus).not.toHaveBeenCalled()
})

it.each(["workspace", "session", "space", "tab", "group", "mode"])("does not restore after the %s focus context changes", async change => {
    render(<Harness />)
    const owner = ownerElement()
    const input = await openFrom(owner)
    const focus = vi.spyOn(owner, "focus")
    act(() => {
        if (change === "workspace") useWorkspaceStore.setState({ workspacePath: "/other" })
        if (change === "session") useHerdrStore.setState({ selectedSessionName: "other" })
        if (change === "space") useHerdrStore.setState({ selectedSpaceId: "other" })
        if (change === "tab") useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: "/fixture/b.txt" }] })
        if (change === "group") useWorkspaceStore.setState({ activeGroupIndex: 1, groups: [{ tabs: [], activePath: "/fixture/a.txt" }, { tabs: [], activePath: "/fixture/a.txt" }] })
        if (change === "mode") useUiStore.getState().setMode("git")
    })
    await dismiss(input)
    expect(focus).not.toHaveBeenCalled()
})

it("leaves a Settings command's destination dialog focused", async () => {
    render(<Harness />)
    const owner = ownerElement()
    await openFrom(owner)
    const focus = vi.spyOn(owner, "focus")
    fireEvent.click(screen.getByRole("option", { name: /settings/i }))
    const destination = await screen.findByRole("textbox", { name: "Settings destination" })
    await drainClose()
    expect(destination).toHaveFocus()
    expect(focus).not.toHaveBeenCalled()
})

it("does not restore while a new terminal command is still opening its destination", async () => {
    let finish!: () => void
    mocks.terminal.mockReturnValue(new Promise<void>(resolve => { finish = resolve }))
    render(<Harness />)
    const owner = ownerElement("terminal")
    await openFrom(owner)
    const focus = vi.spyOn(owner, "focus")
    fireEvent.click(screen.getByRole("option", { name: /new terminal/i }))
    await drainClose()
    expect(focus).not.toHaveBeenCalled()
    expect(mocks.terminal).toHaveBeenCalledOnce()
    const destination = ownerElement("terminal")
    await act(async () => { finish(); destination.focus() })
    await drainClose()
    expect(destination).toHaveFocus()
})

it("lets a search result's later editor take focus without returning to the old input", async () => {
    mocks.search.mockImplementation((_root, query, _cs, onEvent: (event: SearchEvent) => void) => {
        if (query) {
            onEvent({ type: "match", path: "/fixture/found.txt", matches: [{ line: 3, col: 0, preview: "needle" }] })
            onEvent({ type: "done", fileCount: 1, truncated: false })
        }
        return Promise.resolve()
    })
    render(<Harness />)
    const owner = ownerElement()
    const input = await openFrom(owner)
    const focus = vi.spyOn(owner, "focus")
    fireEvent.change(input, { target: { value: "needle" } })
    fireEvent.click(await screen.findByText("needle", { selector: "mark" }))
    await drainClose()
    expect(useWorkspaceStore.getState().pendingReveal).toEqual({ path: "/fixture/found.txt", line: 3 })
    expect(focus).not.toHaveBeenCalled()
    const destination = ownerElement("editor")
    destination.focus()
    await drainClose()
    expect(destination).toHaveFocus()
})

it("does not let an old close callback steal a rapidly reopened palette's focus", async () => {
    render(<Harness />)
    const first = ownerElement()
    const input = await openFrom(first)
    fireEvent.keyDown(input, { key: "Escape" })
    const second = ownerElement()
    second.focus()
    toggle(second)
    const reopened = await screen.findByPlaceholderText("Search files, run a command…")
    await drainClose()
    expect(reopened).toHaveFocus()
    await dismiss(reopened)
    expect(second).toHaveFocus()
})

it("keeps a new input focused if it takes over before delayed close autofocus", async () => {
    render(<Harness />)
    const first = ownerElement()
    const input = await openFrom(first)
    fireEvent.keyDown(input, { key: "Escape" })
    const second = ownerElement()
    second.focus()
    await drainClose()
    expect(second).toHaveFocus()
})
