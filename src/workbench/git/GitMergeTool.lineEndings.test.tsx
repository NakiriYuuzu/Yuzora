import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/ipc", async (importOriginal) => ({
    ...(await importOriginal<typeof import("@/lib/ipc")>()),
    gitConflictSides: vi.fn(),
    gitStage: vi.fn(async () => undefined),
    saveFile: vi.fn(async () => 0)
}))
vi.mock("@/features/logs/userAction", () => ({ logUserAction: vi.fn(async () => undefined) }))
vi.mock("@/lib/unsavedGuard", () => ({ dirtyTabPaths: vi.fn(() => []) }))

import * as ipc from "@/lib/ipc"
import type { GitConflictSides, GitStatus } from "@/lib/types"
import { useAppDialogStore } from "@/state/appDialogStore"
import { useGitConflictStore } from "@/state/gitConflictStore"
import { initialGitState, useGitStore } from "@/state/gitStore"
import { GitMergeTool } from "./GitMergeTool"
import fixtureCases from "./fixtures/mergeLineEndingCases.json"

const runOp = vi.fn(async (_name: string, operation: () => Promise<unknown>) => {
    await operation()
    return true
})

beforeEach(() => {
    useGitStore.setState({
        environment: { status: "ready", root: "/owned-merge-fixture", version: "2.50.1" },
        runOp
    })
})

afterEach(() => {
    cleanup()
    useGitStore.setState(initialGitState)
    useGitConflictStore.setState({ conflictsOpen: false, mergePath: null })
    vi.clearAllMocks()
})

async function openMerge(sides: GitConflictSides, changes: number, conflicts = changes) {
    vi.mocked(ipc.gitConflictSides).mockResolvedValue(sides)
    useGitConflictStore.setState({ mergePath: "f.txt" })
    render(<GitMergeTool />)
    await screen.findByText(`${changes} changes, ${conflicts} conflicts left`)
    return document.querySelector<HTMLElement>("[data-merge-pane='result']")!
}

async function saveResult(expected: string, manual = false) {
    await screen.findByText("0 changes, 0 conflicts left")
    fireEvent.click(screen.getByRole("button", { name: "Apply" }))
    if (manual) {
        await waitFor(() => expect(useAppDialogStore.getState().pending?.description).toMatch(/manual resolutions/))
        act(() => useAppDialogStore.getState().respond(true))
    }
    await waitFor(() => expect(ipc.saveFile).toHaveBeenCalledWith("/owned-merge-fixture/f.txt", expected))
    expect(ipc.gitStage).toHaveBeenCalledWith("/owned-merge-fixture", ["f.txt"])
    expect(useGitConflictStore.getState().mergePath).toBeNull()
}

const bothResults: Record<string, string> = {
    "crlf-five-lines": "a\r\nOb\r\nTb\r\nc\r\nd\r\ne\r\n",
    "crlf-short": "a\r\nOb\r\nTb\r\n",
    "crlf-multiple": "1\r\nO2\r\nT2\r\n3\r\n4\r\n5\r\nO6\r\nT6\r\n7\r\n8\r\n9\r\n",
    "lf-control": "a\nOb\nTb\nc\nd\ne\n",
    "crlf-no-final": "a\r\nOb\r\nTb\r\nc\r\nd\r\ne"
}
const actions = { ours: "« Yours", theirs: "Theirs »", both: "Both", ignore: "Ignore" } as const

// Raw index blobs captured from owned Git merge fixtures with `f.txt -text`.
// They include CRLF in all three sides, not only the conflict-marked worktree.
describe("merge tool line-ending coordinate domain", () => {
    for (const fixture of fixtureCases) {
        for (const action of ["ours", "theirs", "both", "ignore"] as const) {
            it(`preserves surrounding lines for ${action}: ${fixture.name}`, async () => {
                const result = await openMerge(fixture.sides as GitConflictSides, fixture.regions)
                for (let remaining = fixture.regions; remaining > 0; remaining--) {
                    fireEvent.click(within(result).getAllByRole("button", { name: actions[action] })[0])
                    await waitFor(() => expect(within(result).queryAllByRole("button", { name: actions[action] })).toHaveLength(remaining - 1))
                }
                // Git coalesces nearby conflicts into one marker block, so the
                // existing manual-resolution guard asks for consent here.
                await saveResult(action === "both" ? bothResults[fixture.name] : fixture.expected[action], fixture.regions > 1)
            })
        }
    }

    for (const action of ["ours", "theirs"] as const) {
        it(`preserves rebased side identity for ${action} with CRLF index blobs`, async () => {
            useGitStore.setState({ status: {
                branch: "fixture", headOid: "0".repeat(40), detached: true, upstream: null, ahead: 0, behind: 0,
                staged: [], unstaged: [], untracked: [], conflicted: [], inProgress: "rebase"
            } satisfies GitStatus })
            const fixture = fixtureCases[0]
            const result = await openMerge(fixture.sides as GitConflictSides, 1)
            fireEvent.click(within(result).getByRole("button", { name: actions[action] }))
            await saveResult(fixture.expected[action === "ours" ? "theirs" : "ours"])
        })
    }

    for (const [name, base, ours, theirs, worktree, expected] of [
        ["mixed", "a\r\nb\nc\r", "a\nOb\r\nc\r", "a\rTb\nc\r\n", "a\n<<<<<<< ours\r\n", "a\r\nTb\r\nc\r\n"],
        ["bare CR", "a\rb\rc\r", "a\rOb\rc\r", "a\rTb\rc\r", "a\r<<<<<<< ours\r", "a\nTb\nc\n"],
        ["CRLF index with LF worktree", "a\r\nb\r\nc\r\n", "a\r\nOb\r\nc\r\n", "a\r\nTb\r\nc\r\n", "a\n<<<<<<< ours\n", "a\nTb\nc\n"]
    ]) {
        it(`uses logical coordinates and the worktree save policy for ${name}`, async () => {
            const result = await openMerge({ code: "UU", base: { kind: "full", content: base },
                ours: { kind: "full", content: ours }, theirs: { kind: "full", content: theirs },
                worktree: { kind: "full", content: worktree }, lossyEncoding: false, symlink: false }, 1)
            fireEvent.click(within(result).getByRole("button", { name: actions.theirs }))
            await saveResult(expected)
        })
    }

    it("resolves simple character conflicts using logical insertion lengths", async () => {
        await openMerge({ code: "UU", base: { kind: "full", content: "let a = 1, b = 2\r\n" },
            ours: { kind: "full", content: "let a = 10, b = 2\r\n" },
            theirs: { kind: "full", content: "let a = 1, b = 20\r\n" },
            worktree: { kind: "full", content: "<<<<<<< ours\r\n" }, lossyEncoding: false, symlink: false }, 1)
        fireEvent.click(screen.getByRole("button", { name: "Resolve simple conflicts" }))
        await saveResult("let a = 10, b = 20\r\n")
    })

    for (const scope of ["left", "right", "all"] as const) {
        it(`applies non-conflicting ${scope} changes with CRLF coordinates`, async () => {
            const result = await openMerge({ code: "UU", base: { kind: "full", content: "1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n7\r\n" },
                ours: { kind: "full", content: "1\r\nO2\r\n3\r\n4\r\n5\r\nO6\r\n7\r\n" },
                theirs: { kind: "full", content: "1\r\n2\r\n3\r\nT4\r\n5\r\nT6\r\n7\r\n" },
                worktree: { kind: "full", content: "<<<<<<< ours\r\n" }, lossyEncoding: false, symlink: false }, 3, 1)
            fireEvent.click(screen.getByRole("button", { name: scope === "all" ? "Apply all non-conflicting" : `Apply non-conflicting from ${scope}` }))
            if (scope !== "all") {
                await screen.findByText("2 changes, 1 conflicts left")
                fireEvent.click(within(result).getAllByRole("button", { name: "Ignore" })[0])
            }
            fireEvent.click(within(result).getByRole("button", { name: actions.theirs }))
            await saveResult(scope === "left" ? "1\r\nO2\r\n3\r\n4\r\n5\r\nT6\r\n7\r\n"
                : scope === "right" ? "1\r\n2\r\n3\r\nT4\r\n5\r\nT6\r\n7\r\n"
                    : "1\r\nO2\r\n3\r\nT4\r\n5\r\nT6\r\n7\r\n")
        })
    }

    it("still detects a raw worktree change that changes only line-ending bytes", async () => {
        const fixture = fixtureCases[0]
        const result = await openMerge(fixture.sides as GitConflictSides, 1)
        fireEvent.click(within(result).getByRole("button", { name: actions.theirs }))
        vi.mocked(ipc.gitConflictSides).mockResolvedValue({ ...fixture.sides,
            worktree: { kind: "full", content: fixture.sides.worktree.content.replaceAll("\r\n", "\n") }
        } as GitConflictSides)
        fireEvent.click(screen.getByRole("button", { name: "Apply" }))
        await waitFor(() => expect(useAppDialogStore.getState().pending?.description).toMatch(/manual resolutions/))
        act(() => useAppDialogStore.getState().respond(false))
        await waitFor(() => expect(useAppDialogStore.getState().pending).toBeNull())
        expect(ipc.saveFile).not.toHaveBeenCalled()
        expect(ipc.gitStage).not.toHaveBeenCalled()
        await saveResult(fixture.expected.theirs, true)
    })
})
