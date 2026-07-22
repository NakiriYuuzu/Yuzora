import { describe, expect, it } from "vitest"
import { fireEvent, render, screen, within } from "@testing-library/react"

import type { BlockEntry } from "@/agent/acpTypes"

import { ActivityChain } from "./ActivityChain"

function toolEntry(
    id: string,
    text: string,
    meta: Record<string, unknown>
): BlockEntry {
    return { id, kind: "tool", text, meta: JSON.stringify(meta) } as BlockEntry
}

describe("ActivityChain sub-agent rendering", () => {
    it("renders a spawn as a sub-agent card with type chip and expandable prompt", () => {
        const entries = [
            toolEntry("t-1", "Audit error paths", {
                toolCallId: "call-1",
                kind: "think",
                status: "in_progress",
                rawInput: {
                    description: "Audit error paths",
                    prompt: "Read git.rs and audit every error path in detail.",
                    subagent_type: "Explore"
                }
            })
        ]
        render(<ActivityChain entries={entries} live />)

        const step = screen.getByTestId("subagent-step")
        expect(within(step).getByTestId("subagent-chip")).toHaveTextContent("Explore")
        expect(step).toHaveTextContent("Audit error paths")

        fireEvent.click(within(step).getByRole("button", { expanded: false }))
        expect(step).toHaveTextContent("Read git.rs and audit every error path in detail.")
    })

    it("nests claude subagent-attributed tool calls under their spawn", () => {
        const entries = [
            toolEntry("t-1", "Scan the repo", {
                toolCallId: "spawn-1",
                status: "in_progress",
                rawInput: { description: "Scan the repo", prompt: "Scan everything.", subagent_type: "general-purpose" }
            }),
            toolEntry("t-2", "grep -r TODO", {
                toolCallId: "child-1",
                parentToolCallId: "spawn-1",
                kind: "search",
                status: "completed",
                rawInput: { pattern: "TODO" }
            }),
            toolEntry("t-3", "cargo check", {
                toolCallId: "top-1",
                kind: "execute",
                status: "completed",
                rawInput: { command: "cargo check" }
            })
        ]
        render(<ActivityChain entries={entries} live />)

        const children = screen.getByTestId("subagent-children")
        expect(within(children).getByText(/grep -r TODO/)).toBeInTheDocument()
        // 頂層工具不受影響；子工具不重複出現在頂層（僅嵌套內一份）。
        expect(screen.getAllByText(/grep -r TODO/)).toHaveLength(1)
        expect(screen.getByText(/cargo check/)).toBeInTheDocument()
    })

    it("keeps orphaned parent references at the top level", () => {
        const entries = [
            toolEntry("t-1", "read file", {
                toolCallId: "c-9",
                parentToolCallId: "not-in-this-chain",
                kind: "read",
                status: "completed",
                rawInput: { path: "/tmp/a" }
            })
        ]
        render(<ActivityChain entries={entries} live />)
        expect(screen.queryByTestId("subagent-children")).toBeNull()
        expect(screen.getByText(/read file/)).toBeInTheDocument()
    })
})
