import { useState } from "react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import { BranchPopover } from "./BranchPopover"
import { strings } from "@/lib/i18n"
import { initialGitState, useGitStore } from "@/state/gitStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import type { BranchList } from "@/lib/types"
import * as ipc from "@/lib/ipc"

vi.mock("@/lib/ipc", () => ({
    gitCheckout: vi.fn(async () => undefined),
    gitCreateBranch: vi.fn(async () => undefined),
    gitFetch: vi.fn(async () => undefined),
    gitPull: vi.fn(async () => undefined),
    gitPush: vi.fn(async () => undefined),
    // runOp refreshes status + branches after a successful op; stub those too.
    gitStatus: vi.fn(async () => makeStatus()),
    gitBranches: vi.fn(async () => ({ local: [], remote: [] }))
}))

function makeStatus() {
    return {
        branch: "main",
        headOid: "0".repeat(40),
        detached: false,
        upstream: null,
        ahead: 0,
        behind: 0,
        staged: [],
        unstaged: [],
        untracked: [],
        conflicted: [],
        inProgress: null
    }
}

const workspaceInitial = useWorkspaceStore.getState()

function branches(over: Partial<BranchList> = {}): BranchList {
    return { local: [], remote: [], ...over }
}

beforeEach(() => {
    // Merge (not replace) so the store keeps its actions (runOp/refresh/…);
    // initialGitState resets every data field.
    useGitStore.setState(initialGitState)
    useWorkspaceStore.setState(workspaceInitial, true)
    vi.clearAllMocks()
})

afterEach(() => cleanup())

describe("BranchPopover", () => {
    it("renders local branches with current marker and checkout on others", () => {
        useGitStore.setState({
            branches: branches({
                local: [
                    { name: "main", upstream: "origin/main", ahead: 2, behind: 0, isCurrent: true },
                    { name: "feature/x", upstream: null, ahead: 0, behind: 0, isCurrent: false }
                ],
                remote: ["origin/main"]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        expect(screen.getByText("main")).toBeInTheDocument()
        expect(screen.getByText("feature/x")).toBeInTheDocument()
        expect(screen.getByText("current")).toBeInTheDocument()
        expect(screen.getByRole("button", { name: /checkout/i })).toBeInTheDocument()
    })

    it("renders Git Branches title and Local / Remote sections", () => {
        useGitStore.setState({
            branches: branches({
                local: [{ name: "main", upstream: "origin/main", ahead: 0, behind: 0, isCurrent: true }],
                remote: ["origin/main", "origin/dev"]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        expect(screen.getByText("Git Branches")).toBeInTheDocument()
        expect(screen.getByText("Local")).toBeInTheDocument()
        expect(screen.getByText("Remote")).toBeInTheDocument()
        expect(screen.getByText("origin/dev")).toBeInTheDocument()
    })

    it("shows ahead / behind badges for local branches", () => {
        useGitStore.setState({
            branches: branches({
                local: [{ name: "main", upstream: "origin/main", ahead: 3, behind: 1, isCurrent: true }]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        expect(screen.getByText("↑3")).toBeInTheDocument()
        expect(screen.getByText("↓1")).toBeInTheDocument()
    })

    it("checks out a non-current branch when no dirty tabs", async () => {
        useGitStore.setState({
            branches: branches({
                local: [{ name: "dev", upstream: null, ahead: 0, behind: 0, isCurrent: false }]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        fireEvent.click(screen.getByRole("button", { name: /checkout/i }))
        await waitFor(() => expect(ipc.gitCheckout).toHaveBeenCalledWith("dev"))
    })

    it("checkout blocked when dirty tabs exist", async () => {
        useWorkspaceStore.setState({
            groups: [
                {
                    tabs: [{ path: "/w/a.ts", name: "a.ts", dirty: true, externallyModified: false }],
                    activePath: "/w/a.ts"
                }
            ],
            activeGroupIndex: 0
        })
        useGitStore.setState({
            branches: branches({
                local: [{ name: "dev", upstream: null, ahead: 0, behind: 0, isCurrent: false }]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        fireEvent.click(screen.getByRole("button", { name: /checkout/i }))
        expect(await screen.findByText(/未儲存的變更/)).toBeInTheDocument()
        expect(ipc.gitCheckout).not.toHaveBeenCalled()
    })

    it("creates a new branch on Enter", async () => {
        render(<BranchPopover open onOpenChange={() => {}} />)
        fireEvent.click(screen.getByText(/New branch/i))
        const input = screen.getByPlaceholderText(strings.git.branchNamePlaceholder)
        fireEvent.change(input, { target: { value: "feature/y" } })
        fireEvent.keyDown(input, { key: "Enter" })
        await waitFor(() => expect(ipc.gitCreateBranch).toHaveBeenCalledWith("feature/y"))
    })

    // Each op is exercised in isolation: runOp holds `busy` (and disables the
    // whole row) until its trailing refresh settles, so back-to-back clicks in a
    // single render would race the busy latch.
    it("runs fetch through runOp", async () => {
        render(<BranchPopover open onOpenChange={() => {}} />)
        fireEvent.click(screen.getByRole("button", { name: /^fetch$/i }))
        await waitFor(() => expect(ipc.gitFetch).toHaveBeenCalled())
    })

    it("runs pull through runOp", async () => {
        render(<BranchPopover open onOpenChange={() => {}} />)
        fireEvent.click(screen.getByRole("button", { name: /^pull$/i }))
        await waitFor(() => expect(ipc.gitPull).toHaveBeenCalled())
    })

    it("runs push through runOp", async () => {
        render(<BranchPopover open onOpenChange={() => {}} />)
        fireEvent.click(screen.getByRole("button", { name: /^push$/i }))
        await waitFor(() => expect(ipc.gitPush).toHaveBeenCalled())
    })

    it("disables the action row while an op is in flight", () => {
        // busy === "fetch": the fetch button shows a spinner label, the other two
        // keep their plain names but are disabled.
        useGitStore.setState({ busy: "fetch" })
        render(<BranchPopover open onOpenChange={() => {}} />)
        expect(screen.getByRole("button", { name: /^pull$/i })).toBeDisabled()
        expect(screen.getByRole("button", { name: /^push$/i })).toBeDisabled()
    })

    it("shows a paused-auth notice when remotePaused", () => {
        useGitStore.setState({ remotePaused: true })
        render(<BranchPopover open onOpenChange={() => {}} />)
        expect(screen.getByText(/遠端檢查已暫停/)).toBeInTheDocument()
    })

    it("resets the checkout-blocked notice when the popover closes (T14)", async () => {
        useWorkspaceStore.setState({
            groups: [
                {
                    tabs: [{ path: "/w/a.ts", name: "a.ts", dirty: true, externallyModified: false }],
                    activePath: "/w/a.ts"
                }
            ],
            activeGroupIndex: 0
        })
        useGitStore.setState({
            branches: branches({
                local: [{ name: "dev", upstream: null, ahead: 0, behind: 0, isCurrent: false }]
            })
        })

        function Harness() {
            const [open, setOpen] = useState(true)
            return (
                <>
                    <button onClick={() => setOpen(false)}>close-pop</button>
                    <button onClick={() => setOpen(true)}>open-pop</button>
                    <BranchPopover open={open} onOpenChange={setOpen} />
                </>
            )
        }
        render(<Harness />)
        fireEvent.click(screen.getByRole("button", { name: /checkout/i }))
        expect(await screen.findByText(/未儲存的變更/)).toBeInTheDocument()
        // Close then reopen: the stale notice must not linger.
        fireEvent.click(screen.getByText("close-pop"))
        fireEvent.click(screen.getByText("open-pop"))
        expect(screen.queryByText(/未儲存的變更/)).not.toBeInTheDocument()
    })
})
