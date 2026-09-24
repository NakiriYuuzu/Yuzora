import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render } from "@testing-library/react"
import { WorkbenchKeyboardBridge } from "./WorkbenchKeyboardBridge"
import { navigateWorkbenchTabs } from "@/lib/workbenchTabNavigation"
import { useKeyboardSettingsStore } from "@/state/keyboardSettingsStore"
import { uiInitialState, useUiStore } from "@/state/uiStore"

vi.mock("@/lib/workbenchTabNavigation", () => ({ navigateWorkbenchTabs: vi.fn(async () => {}) }))
vi.mock("@/terminal/openNewTerminalTab", () => ({ openNewTerminalTab: vi.fn(async () => {}) }))

beforeEach(() => { vi.clearAllMocks(); useKeyboardSettingsStore.setState({ overrides: {} }) })
afterEach(cleanup)

it("handles tab navigation before terminal/editor key handlers and removes its listener on unmount", () => {
    const childKeyDown = vi.fn()
    const view = render(<><WorkbenchKeyboardBridge /><div className="xterm"><textarea onKeyDown={childKeyDown} /></div></>)
    const input = view.getByRole("textbox")
    fireEvent.keyDown(input, { key: "Tab", ctrlKey: true })
    expect(navigateWorkbenchTabs).toHaveBeenCalledExactlyOnceWith({ direction: 1 })
    expect(childKeyDown).not.toHaveBeenCalled()
    view.unmount()
    fireEvent.keyDown(window, { key: "Tab", ctrlKey: true })
    expect(navigateWorkbenchTabs).toHaveBeenCalledOnce()
})

it("preserves dialog and IME input while honoring reassigned shortcuts", () => {
    useKeyboardSettingsStore.setState({ overrides: { nextTab: "Ctrl+J" } })
    const view = render(<><WorkbenchKeyboardBridge /><textarea /><div role="dialog"><input /></div></>)
    const [editor, dialog] = view.getAllByRole("textbox")
    fireEvent.keyDown(dialog, { key: "j", ctrlKey: true })
    fireEvent.keyDown(editor, { key: "j", ctrlKey: true, isComposing: true })
    fireEvent.keyDown(editor, { key: "Tab", ctrlKey: true })
    expect(navigateWorkbenchTabs).not.toHaveBeenCalled()
    fireEvent.keyDown(editor, { key: "j", ctrlKey: true })
    expect(navigateWorkbenchTabs).toHaveBeenCalledExactlyOnceWith({ direction: 1 })
})

it("switches the Spaces/Agents view with Ctrl+Shift+E even inside a Windows terminal", () => {
    vi.spyOn(navigator, "userAgent", "get").mockReturnValue("Windows")
    useUiStore.setState(uiInitialState)
    const childKeyDown = vi.fn()
    const view = render(<><WorkbenchKeyboardBridge /><div className="xterm"><textarea onKeyDown={childKeyDown} /></div></>)
    fireEvent.keyDown(view.getByRole("textbox"), { key: "E", code: "KeyE", ctrlKey: true, shiftKey: true })
    expect(useUiStore.getState().sidebarViewToggleRequest).toBe(1)
    expect(childKeyDown).not.toHaveBeenCalled()
})
