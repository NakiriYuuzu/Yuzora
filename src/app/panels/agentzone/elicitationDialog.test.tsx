import { beforeEach, describe, expect, it, vi } from "vitest"
import { fireEvent, render, screen, waitFor } from "@testing-library/react"

import type { ElicitationRequest } from "@/agent/acpConnection"
import { useAgentStore, type PendingElicitation } from "@/state/agentStore"

import { ElicitationDialog } from "./ElicitationDialog"

function pendingOf(
    id: string,
    request: ElicitationRequest,
    respond: PendingElicitation["respond"] = () => {}
): PendingElicitation {
    return { id, request, respond }
}

function setQueue(queue: PendingElicitation[]) {
    useAgentStore.setState({
        pendingElicitations: new Map(queue.length > 0 ? [["s-1", queue]] : [])
    })
}

describe("ElicitationDialog", () => {
    beforeEach(() => {
        setQueue([])
    })

    it("renders nothing without a pending elicitation", () => {
        render(<ElicitationDialog sessionId="s-1" />)
        expect(screen.queryByTestId("agent-elicitation-dialog")).toBeNull()
    })

    it("solo enum: clicking an option accepts with that value and closes", async () => {
        const respond = vi.fn()
        setQueue([pendingOf("el-1", {
            message: "Pick a color",
            fields: [{
                key: "choice",
                type: "string",
                required: true,
                options: [{ value: "red", label: "Red" }, { value: "blue", label: "Blue", description: "cool" }]
            }]
        }, respond)])

        render(<ElicitationDialog sessionId="s-1" />)
        expect(screen.getByTestId("agent-elicitation-dialog")).toBeInTheDocument()
        expect(screen.getByText("Pick a color")).toBeInTheDocument()
        fireEvent.click(screen.getByRole("option", { name: /Red/ }))

        expect(respond).toHaveBeenCalledExactlyOnceWith({ action: "accept", content: { choice: "red" } })
        await waitFor(() => expect(screen.queryByTestId("agent-elicitation-dialog")).toBeNull())
    })

    it("solo boolean: yes/no buttons accept true/false", () => {
        const respond = vi.fn()
        setQueue([pendingOf("el-1", {
            message: "Overwrite the file?",
            fields: [{ key: "confirmed", type: "boolean", required: true }]
        }, respond)])

        render(<ElicitationDialog sessionId="s-1" />)
        fireEvent.click(screen.getByRole("button", { name: "Yes" }))
        expect(respond).toHaveBeenCalledExactlyOnceWith({ action: "accept", content: { confirmed: true } })
    })

    it("generic form: submit stays disabled until required text is filled", () => {
        const respond = vi.fn()
        setQueue([pendingOf("el-1", {
            message: "Name the branch",
            fields: [{ key: "value", type: "string", title: "Branch", required: true }]
        }, respond)])

        render(<ElicitationDialog sessionId="s-1" />)
        const submit = screen.getByRole("button", { name: "Submit" })
        expect(submit).toBeDisabled()
        fireEvent.change(screen.getByRole("textbox"), { target: { value: "feat/x" } })
        expect(submit).toBeEnabled()
        fireEvent.click(submit)
        expect(respond).toHaveBeenCalledExactlyOnceWith({ action: "accept", content: { value: "feat/x" } })
    })

    it("multiline fields render a textarea seeded with the default", () => {
        setQueue([pendingOf("el-1", {
            message: "Edit the note",
            fields: [{ key: "value", type: "string", required: true, multiline: true, defaultValue: "seed" }]
        })])

        render(<ElicitationDialog sessionId="s-1" />)
        const textarea = screen.getByRole("textbox")
        expect(textarea.tagName).toBe("TEXTAREA")
        expect(textarea).toHaveValue("seed")
    })

    it("escape closes the dialog as a single cancel", async () => {
        const respond = vi.fn()
        setQueue([pendingOf("el-1", {
            message: "Pick",
            fields: [{ key: "value", type: "string", required: true }]
        }, respond)])

        render(<ElicitationDialog sessionId="s-1" />)
        fireEvent.keyDown(screen.getByTestId("agent-elicitation-dialog"), { key: "Escape" })
        await waitFor(() => expect(respond).toHaveBeenCalledExactlyOnceWith({ action: "cancel" }))
    })

    // P4：array multiselect——單一 array 欄不走 soloEnum 捷徑（點選是 toggle、
    // 不是送出），required 需至少一項，提交內容為 string[]。
    it("array multiselect: toggles options and submits a string[] answer", () => {
        const respond = vi.fn()
        setQueue([pendingOf("el-1", {
            message: "Pick colors",
            fields: [{
                key: "colors",
                type: "array",
                title: "Colors",
                required: true,
                options: [
                    { value: "red", label: "Red" },
                    { value: "blue", label: "Blue" },
                    { value: "green", label: "Green" }
                ]
            }, { key: "note", type: "string", title: "Note", required: false }]
        }, respond)])

        render(<ElicitationDialog sessionId="s-1" />)
        const submit = screen.getByRole("button", { name: "Submit" })
        expect(submit).toBeDisabled()

        fireEvent.click(screen.getByRole("option", { name: "Red" }))
        expect(respond).not.toHaveBeenCalled()
        expect(screen.getByRole("option", { name: "Red" })).toHaveAttribute("aria-selected", "true")
        expect(submit).toBeEnabled()

        fireEvent.click(screen.getByRole("option", { name: "Blue" }))
        fireEvent.click(screen.getByRole("option", { name: "Red" }))
        expect(screen.getByRole("option", { name: "Red" })).toHaveAttribute("aria-selected", "false")

        fireEvent.change(screen.getByRole("textbox"), { target: { value: "extra" } })
        fireEvent.click(submit)
        expect(respond).toHaveBeenCalledExactlyOnceWith({
            action: "accept",
            content: { colors: ["blue"], note: "extra" }
        })
    })

    it("shows the next queued elicitation after answering the head", async () => {
        const first = vi.fn()
        const second = vi.fn()
        setQueue([
            pendingOf("el-1", {
                message: "First question",
                fields: [{ key: "choice", type: "string", required: true, options: [{ value: "a", label: "A" }] }]
            }, first),
            pendingOf("el-2", {
                message: "Second question",
                fields: [{ key: "confirmed", type: "boolean", required: true }]
            }, second)
        ])

        render(<ElicitationDialog sessionId="s-1" />)
        fireEvent.click(screen.getByRole("option", { name: "A" }))
        expect(first).toHaveBeenCalledOnce()
        await waitFor(() => expect(screen.getByText("Second question")).toBeInTheDocument())
        fireEvent.click(screen.getByRole("button", { name: "No" }))
        expect(second).toHaveBeenCalledExactlyOnceWith({ action: "accept", content: { confirmed: false } })
    })
})
