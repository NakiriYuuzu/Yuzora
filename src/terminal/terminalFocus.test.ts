import { afterEach, expect, it, vi } from "vitest"
import { focusActiveTerminal, registerTerminalFocusTarget, requestTerminalFocus } from "./terminalFocus"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { useTextInputDialogStore } from "@/state/textInputDialogStore"

afterEach(() => { vi.useRealTimers(); document.body.innerHTML = "" })
it("resolves the current terminal identity after a view is replaced", () => {
    const path = "yuzora://herdr/one"
    useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: path }], activeGroupIndex: 0 })
    const old = vi.fn(), current = vi.fn()
    const removeOld = registerTerminalFocusTarget("pane", { pagePath: path, active: () => true, focus: old })
    const removeCurrent = registerTerminalFocusTarget("pane", { pagePath: path, active: () => true, focus: current })
    removeOld()
    expect(focusActiveTerminal()).toBe(true)
    expect(old).not.toHaveBeenCalled()
    expect(current).toHaveBeenCalledOnce()
    removeCurrent()
})
it("waits for a new terminal and does not steal focus after switching tabs", () => {
    vi.useFakeTimers()
    const path = "yuzora://herdr/new"
    useTextInputDialogStore.setState({ pending: null })
    useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: path }], activeGroupIndex: 0 })
    requestTerminalFocus(path)
    vi.advanceTimersByTime(1)
    const focus = vi.fn()
    const release = registerTerminalFocusTarget("new", { pagePath: path, active: () => true, focus })
    vi.advanceTimersByTime(50)
    expect(focus).toHaveBeenCalledOnce()
    requestTerminalFocus(path)
    useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: "/file" }] })
    vi.runAllTimers()
    expect(focus).toHaveBeenCalledOnce()
    release()
})

it("focuses only the active split pane and preserves dialogs and other input fields", () => {
    vi.useFakeTimers()
    const path = "yuzora://herdr/split"
    useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: path }], activeGroupIndex: 0 })
    const inactive = vi.fn(), active = vi.fn()
    const removeInactive = registerTerminalFocusTarget("left", { pagePath: path, active: () => false, focus: inactive })
    const removeActive = registerTerminalFocusTarget("right", { pagePath: path, active: () => true, focus: active })
    expect(focusActiveTerminal()).toBe(true)
    expect(inactive).not.toHaveBeenCalled()
    expect(active).toHaveBeenCalledOnce()
    const input = document.body.appendChild(document.createElement("input"))
    input.focus()
    requestTerminalFocus(path)
    vi.runAllTimers()
    expect(active).toHaveBeenCalledOnce()
    const dialog = document.body.appendChild(document.createElement("div"))
    dialog.setAttribute("aria-modal", "true")
    expect(focusActiveTerminal()).toBe(false)
    removeInactive(); removeActive()
})
