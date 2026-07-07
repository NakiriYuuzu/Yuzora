import { afterEach, expect, test } from "vitest"
import { render, screen, fireEvent, waitFor } from "@testing-library/react"

import { ConfirmDialogHost } from "./ConfirmDialogHost"
import { useConfirmDialogStore } from "@/state/confirmDialogStore"

afterEach(() => useConfirmDialogStore.setState({ pending: null }))

test("pending 為 null 時不渲染 dialog", () => {
    render(<ConfirmDialogHost />)
    expect(screen.queryByRole("dialog")).toBeNull()
})

test("按 Save → promise resolve 'save' 並關閉 dialog", async () => {
    render(<ConfirmDialogHost />)
    const decision = useConfirmDialogStore.getState().requestUnsavedDecision({
        title: "T",
        description: "D",
        saveLabel: "Save"
    })
    expect(await screen.findByRole("dialog")).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Save" }))
    expect(await decision).toBe("save")
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull())
})

test("按 Discard → promise resolve 'discard'", async () => {
    render(<ConfirmDialogHost />)
    const decision = useConfirmDialogStore.getState().requestUnsavedDecision({
        title: "T",
        description: "D",
        saveLabel: "Save"
    })
    expect(await screen.findByRole("dialog")).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Discard" }))
    expect(await decision).toBe("discard")
})

test("按 Cancel → promise resolve 'cancel'", async () => {
    render(<ConfirmDialogHost />)
    const decision = useConfirmDialogStore.getState().requestUnsavedDecision({
        title: "T",
        description: "D",
        saveLabel: "Save"
    })
    expect(await screen.findByRole("dialog")).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    expect(await decision).toBe("cancel")
})

test("saveLabel 由請求控制（workspace 切換用 'Save all'）", async () => {
    render(<ConfirmDialogHost />)
    void useConfirmDialogStore.getState().requestUnsavedDecision({
        title: "T",
        description: "D",
        saveLabel: "Save all"
    })
    expect(await screen.findByRole("dialog")).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Save all" })).toBeInTheDocument()
})
