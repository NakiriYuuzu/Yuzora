import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, expect, test } from "vitest"

import { useTextInputDialogStore } from "@/state/textInputDialogStore"
import { TextInputDialogHost } from "./TextInputDialogHost"

afterEach(() => {
    useTextInputDialogStore.setState({ pending: null })
})

test("uses the same compact non-resizable size contract as alert dialogs", async () => {
    render(<TextInputDialogHost />)
    act(() => {
        void useTextInputDialogStore.getState().request({
            title: "Rename tab",
            label: "Name",
            initialValue: "Old",
            confirmLabel: "Rename"
        })
    })

    const dialog = await screen.findByRole("dialog")
    expect(dialog).not.toHaveAttribute("data-dialog-size-id")
    expect(dialog.style.width).toBe("")
    expect(dialog.style.height).toBe("")
    expect(dialog.className).toMatch(/sm:max-w-\[420px\]/)
    expect(dialog.className).not.toMatch(/max-w-none/)
    expect(
        dialog.querySelectorAll('[data-slot="dialog-resize-handle"]').length
    ).toBe(0)
})

test("submits a trimmed name from the shared in-app text dialog", async () => {
    render(<TextInputDialogHost />)
    const result = useTextInputDialogStore.getState().request({
        title: "Rename tab",
        label: "Name",
        initialValue: "Old",
        confirmLabel: "Rename"
    })

    const input = await screen.findByLabelText("Name")
    expect(input).toHaveValue("Old")
    fireEvent.change(input, { target: { value: "  New name  " } })
    fireEvent.click(screen.getByRole("button", { name: "Rename" }))

    await expect(result).resolves.toBe("New name")
})

test("a same-shaped superseding request resets the input instead of retaining stale text", async () => {
    render(<TextInputDialogHost />)
    const first = useTextInputDialogStore.getState().request({
        title: "New file",
        label: "Name",
        confirmLabel: "Create"
    })
    const input = await screen.findByLabelText("Name")
    fireEvent.change(input, { target: { value: "stale.ts" } })

    const second = useTextInputDialogStore.getState().request({
        title: "New file",
        label: "Name",
        confirmLabel: "Create"
    })

    await expect(first).resolves.toBeNull()
    await waitFor(() => expect(screen.getByLabelText("Name")).toHaveValue(""))
    useTextInputDialogStore.getState().respond(null)
    await expect(second).resolves.toBeNull()
})

test("cancel resolves null without submitting a name", async () => {
    render(<TextInputDialogHost />)
    const result = useTextInputDialogStore.getState().request({
        title: "Rename tab",
        label: "Name",
        initialValue: "Old",
        confirmLabel: "Rename"
    })

    await waitFor(() => expect(screen.getByRole("dialog")).toBeVisible())
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))

    await expect(result).resolves.toBeNull()
})
