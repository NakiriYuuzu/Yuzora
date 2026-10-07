import { act, fireEvent, render, screen } from "@testing-library/react"
import { Toaster } from "sonner"
import { afterEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/ipc", () => ({ fsDeleteCancel: vi.fn(async () => undefined) }))

import { fsDeleteCancel } from "@/lib/ipc"
import type { DeleteProgress } from "@/lib/types"
import { withDeleteProgress } from "./deleteProgress"

const pause = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms))

function slowDelete() {
    const handle = {
        id: "",
        report: (_progress: DeleteProgress) => {},
        finish: () => {}
    }
    const run = (operation: { id: string; onProgress: (progress: DeleteProgress) => void }) => {
        handle.id = operation.id
        handle.report = operation.onProgress
        return new Promise<void>((resolve) => {
            handle.finish = resolve
        })
    }
    return { handle, run }
}

afterEach(() => vi.clearAllMocks())

describe("withDeleteProgress", () => {
    it("shows a progress toast for a slow local delete that the user can cancel", async () => {
        render(<Toaster />)
        const { handle, run } = slowDelete()
        const done = withDeleteProgress("node_modules", true, run)
        await act(() => pause(350))

        const bar = await screen.findByRole("progressbar", { name: "Delete progress" })
        expect(screen.getByText("Deleting “node_modules”")).toBeInTheDocument()
        act(() => handle.report({ phase: "counting", found: 1200 }))
        expect(bar).not.toHaveAttribute("aria-valuenow")
        act(() => handle.report({ phase: "deleting", removed: 250, total: 1000 }))
        expect(bar).toHaveAttribute("aria-valuenow", "25")
        expect(screen.getByText("Deleted 250 of 1,000 items")).toBeInTheDocument()

        fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
        expect(fsDeleteCancel).toHaveBeenCalledWith(handle.id)
        expect(screen.getByRole("button", { name: "Cancelling…" })).toBeDisabled()

        handle.finish()
        await act(() => done)
        await act(() => pause(500))
        expect(screen.queryByRole("progressbar")).not.toBeInTheDocument()
    })

    it("shows no toast for a quick delete and no Cancel for a remote one", async () => {
        render(<Toaster />)
        await withDeleteProgress("a.ts", true, async () => undefined)
        await act(() => pause(350))
        expect(screen.queryByRole("progressbar")).not.toBeInTheDocument()

        const { handle, run } = slowDelete()
        const done = withDeleteProgress("remote-dir", false, run)
        await act(() => pause(350))
        expect(await screen.findByRole("progressbar", { name: "Delete progress" })).toBeInTheDocument()
        expect(screen.getByText("Deleting remote items…")).toBeInTheDocument()
        expect(screen.queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument()
        handle.finish()
        await act(() => done)
    })
})
