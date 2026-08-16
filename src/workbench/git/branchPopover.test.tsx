import { useState } from "react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"

import { BranchPopover } from "./BranchPopover"
import i18n from "@/lib/i18n"
import { initialGitState, useGitStore } from "@/state/gitStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import type { BranchList } from "@/lib/types"
import * as ipc from "@/lib/ipc"
import { writeText } from "@tauri-apps/plugin-clipboard-manager"
import { useAppDialogStore } from "@/state/appDialogStore"
import { useTextInputDialogStore } from "@/state/textInputDialogStore"

vi.mock("@/lib/ipc", () => ({
    gitCheckout: vi.fn(async () => undefined),
    gitCheckoutDetached: vi.fn(async () => undefined),
    gitCreateBranch: vi.fn(async () => undefined),
    gitFetch: vi.fn(async () => undefined),
    gitPull: vi.fn(async () => undefined),
    gitPush: vi.fn(async () => undefined),
    // runOp refreshes status + branches after a successful op; stub those too.
    gitStatus: vi.fn(async () => makeStatus()),
    gitBranches: vi.fn(async () => ({ local: [], remote: [], tags: [] }))
}))

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
    writeText: vi.fn(async () => undefined)
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
    return { local: [], remote: [], tags: [], ...over }
}

function searchField() {
    return screen.getByPlaceholderText(i18n.t("branchPopover.searchPlaceholder", { ns: "menus" }))
}

function setQuery(value: string) {
    fireEvent.change(searchField(), { target: { value } })
}

function selectTab(name: RegExp) {
    fireEvent.mouseDown(screen.getByRole("tab", { name }))
}

beforeEach(() => {
    // Merge (not replace) so the store keeps its actions (runOp/refresh/…);
    // initialGitState resets every data field.
    useGitStore.setState({ ...initialGitState, environment: { status: "ready", root: "/w", version: "2.50" } })
    useWorkspaceStore.setState(workspaceInitial, true)
    useAppDialogStore.setState({ pending: null })
    useTextInputDialogStore.setState({ pending: null })
    vi.clearAllMocks()
})

afterEach(() => cleanup())

describe("BranchPopover", () => {
    it("renders local branches with current marker and checkout on others", () => {
        useGitStore.setState({
            branches: branches({
                local: [
                    { name: "main", upstream: "origin/main", ahead: 2, behind: 0, isCurrent: true, gone: false },
                    { name: "feature/x", upstream: null, ahead: 0, behind: 0, isCurrent: false, gone: false }
                ],
                remote: ["origin/main"],
                tags: [{ name: "v1.0.0", date: "2026-08-01T12:00:00Z" }]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        expect(screen.getByRole("option", { name: "main" })).toBeInTheDocument()
        const other = screen.getByRole("option", { name: "feature/x" })
        expect(other).toBeInTheDocument()
        expect(screen.getByText(/current/i)).toBeInTheDocument()
        expect(within(other).getByText("Checkout")).toBeInTheDocument()
        expect(within(other).queryByRole("button")).not.toBeInTheDocument()
    })

    it("renders Git Branches title and Local / Remote sections", () => {
        useGitStore.setState({
            branches: branches({
                local: [{ name: "main", upstream: "origin/main", ahead: 0, behind: 0, isCurrent: true, gone: false }],
                remote: ["origin/main", "origin/dev"]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        expect(screen.getByText("Git Branches")).toBeInTheDocument()
        expect(screen.getByText("Local")).toBeInTheDocument()
        expect(screen.getByText("Remote")).toBeInTheDocument()
        selectTab(/remote/i)
        expect(screen.getByRole("option", { name: "origin/dev" })).toBeInTheDocument()
    })

    it("shows ahead / behind badges for local branches", () => {
        useGitStore.setState({
            branches: branches({
                local: [{ name: "main", upstream: "origin/main", ahead: 3, behind: 1, isCurrent: true, gone: false }]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        expect(screen.getByText("↑3")).toBeInTheDocument()
        expect(screen.getByText("↓1")).toBeInTheDocument()
    })

    it("checks out a non-current branch when no dirty tabs", async () => {
        useGitStore.setState({
            branches: branches({
                local: [{ name: "dev", upstream: null, ahead: 0, behind: 0, isCurrent: false, gone: false }]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        fireEvent.click(screen.getByRole("option", { name: "dev" }))
        await waitFor(() => expect(ipc.gitCheckout).toHaveBeenCalledWith("/w", "dev"))
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
                local: [{ name: "dev", upstream: null, ahead: 0, behind: 0, isCurrent: false, gone: false }]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        fireEvent.click(screen.getByRole("option", { name: "dev" }))
        expect(await screen.findByText(/unsaved changes/)).toBeInTheDocument()
        expect(ipc.gitCheckout).not.toHaveBeenCalled()
    })

    it("creates a new branch on Enter", async () => {
        render(<BranchPopover open onOpenChange={() => {}} />)
        fireEvent.click(screen.getByText(/New branch/i))
        const input = screen.getByPlaceholderText(i18n.t("branchNamePlaceholder", { ns: "git" }))
        fireEvent.change(input, { target: { value: "feature/y" } })
        fireEvent.keyDown(input, { key: "Enter" })
        await waitFor(() => expect(ipc.gitCreateBranch).toHaveBeenCalledWith("/w", "feature/y"))
        expect(vi.mocked(ipc.gitCreateBranch).mock.calls[0]).toHaveLength(2)
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
        expect(screen.getByText(/Remote check paused/)).toBeInTheDocument()
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
                local: [{ name: "dev", upstream: null, ahead: 0, behind: 0, isCurrent: false, gone: false }]
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
        fireEvent.click(screen.getByRole("option", { name: "dev" }))
        expect(await screen.findByText(/unsaved changes/)).toBeInTheDocument()
        // Close then reopen: the stale notice must not linger.
        fireEvent.click(screen.getByText("close-pop"))
        fireEvent.click(screen.getByText("open-pop"))
        expect(screen.queryByText(/unsaved changes/)).not.toBeInTheDocument()
    })

    it("exposes a Command search field that autofocuses on open", () => {
        render(<BranchPopover open onOpenChange={() => {}} />)
        const input = searchField()
        expect(input).toBeInTheDocument()
        expect(input).toHaveFocus()
    })

    it("shows Local / Remote / Tags tab counts", () => {
        useGitStore.setState({
            branches: branches({
                local: [
                    { name: "main", upstream: "origin/main", ahead: 0, behind: 0, isCurrent: true, gone: false },
                    { name: "feature/x", upstream: null, ahead: 0, behind: 0, isCurrent: false, gone: false }
                ],
                remote: ["origin/main"],
                tags: [{ name: "v1.0.0", date: "2026-08-01T12:00:00Z" }]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        expect(screen.getByRole("tab", { name: /local 2/i })).toBeInTheDocument()
        expect(screen.getByRole("tab", { name: /remote 1/i })).toBeInTheDocument()
        expect(screen.getByRole("tab", { name: /tags 1/i })).toBeInTheDocument()
    })

    it("shows the leaf first while keeping the full ref in title and ARIA", () => {
        useGitStore.setState({
            branches: branches({
                local: [{
                    name: "feature/repository-bound-git-history-pagination",
                    upstream: "origin/feature/repository-bound-git-history-pagination",
                    ahead: 0,
                    behind: 0,
                    isCurrent: false, gone: false
                }]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        const option = screen.getByRole("option", { name: "feature/repository-bound-git-history-pagination" })
        expect(option).toHaveAttribute("title", "feature/repository-bound-git-history-pagination")
        expect(within(option).getByText("repository-bound-git-history-pagination")).toBeInTheDocument()
        expect(within(option).queryByText("feature/repository-bound-git-history-pagination")).not.toBeInTheDocument()
    })

    it("groups empty-query rows by the first prefix", () => {
        useGitStore.setState({
            branches: branches({
                local: [
                    { name: "feature/a", upstream: null, ahead: 0, behind: 0, isCurrent: false, gone: false },
                    { name: "feature/b", upstream: null, ahead: 0, behind: 0, isCurrent: false, gone: false },
                    { name: "main", upstream: "origin/main", ahead: 0, behind: 0, isCurrent: true, gone: false }
                ]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        expect(screen.getByText("feature/")).toBeInTheDocument()
        expect(screen.getByText("Other")).toBeInTheDocument()
        expect(screen.queryByText("Search results")).not.toBeInTheDocument()
    })

    it("flattens full-ref matches into Search results and scopes them to the active tab", () => {
        useGitStore.setState({
            branches: branches({
                local: [
                    { name: "feature/x", upstream: null, ahead: 0, behind: 0, isCurrent: false, gone: false },
                    { name: "main", upstream: "origin/main", ahead: 0, behind: 0, isCurrent: true, gone: false }
                ],
                remote: ["origin/feature/x", "origin/main"]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        setQuery("feature")
        expect(screen.getByText("Search results")).toBeInTheDocument()
        expect(screen.getByRole("option", { name: "feature/x" })).toBeInTheDocument()
        expect(screen.queryByRole("option", { name: "origin/feature/x" })).not.toBeInTheDocument()
        expect(screen.queryByText("feature/")).not.toBeInTheDocument()
        expect(document.querySelector("mark")).toHaveTextContent("feature")

        setQuery("origin")
        expect(screen.getByText("No matching refs")).toBeInTheDocument()
        selectTab(/remote/i)
        expect(screen.getByRole("option", { name: "origin/feature/x" })).toBeInTheDocument()
        expect(screen.getByRole("option", { name: "origin/main" })).toBeInTheDocument()
        expect(screen.queryByRole("option", { name: "feature/x" })).not.toBeInTheDocument()
    })

    it("marks unpublished local branches as Local and renders authoritative Gone without sync badges", () => {
        useGitStore.setState({
            branches: branches({
                local: [
                    { name: "main", upstream: "origin/main", ahead: 0, behind: 0, isCurrent: true, gone: false },
                    { name: "wip", upstream: null, ahead: 0, behind: 0, isCurrent: false, gone: false },
                    { name: "release/old", upstream: "origin/release/old", ahead: 4, behind: 2, isCurrent: false, gone: true }
                ]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        expect(within(screen.getByRole("option", { name: "wip" })).getByText("Local")).toBeInTheDocument()
        const gone = screen.getByRole("option", { name: "release/old" })
        expect(within(gone).getByText("Gone")).toBeInTheDocument()
        expect(within(gone).queryByText("↑4")).not.toBeInTheDocument()
        expect(within(gone).queryByText("↓2")).not.toBeInTheDocument()
    })

    it("derives Tracked when a remote name exactly matches a local upstream", () => {
        useGitStore.setState({
            branches: branches({
                local: [{ name: "main", upstream: "origin/main", ahead: 0, behind: 0, isCurrent: true, gone: false }],
                remote: ["origin/main", "origin/dev"]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        selectTab(/remote/i)
        expect(within(screen.getByRole("option", { name: "origin/main" })).getByText("Tracked")).toBeInTheDocument()
        expect(within(screen.getByRole("option", { name: "origin/dev" })).queryByText("Tracked")).not.toBeInTheDocument()
    })

    it("shows Diverged together with both ahead and behind counts", () => {
        useGitStore.setState({
            branches: branches({
                local: [{ name: "main", upstream: "origin/main", ahead: 3, behind: 1, isCurrent: true, gone: false }]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        expect(screen.getByText("Diverged")).toBeInTheDocument()
        expect(screen.getByText("↑3")).toBeInTheDocument()
        expect(screen.getByText("↓1")).toBeInTheDocument()
    })

    it("does not mutate when the current branch is activated", async () => {
        useGitStore.setState({
            branches: branches({
                local: [
                    { name: "main", upstream: "origin/main", ahead: 0, behind: 0, isCurrent: true, gone: false },
                    { name: "dev", upstream: null, ahead: 0, behind: 0, isCurrent: false, gone: false }
                ]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        fireEvent.click(screen.getByRole("option", { name: "main" }))
        await waitFor(() => {
            expect(ipc.gitCheckout).not.toHaveBeenCalled()
        })
        expect(within(screen.getByRole("option", { name: "main" })).queryByRole("button", { name: /checkout/i })).not.toBeInTheDocument()
    })

    it("checks out using the exact full local name from Command selection", async () => {
        const onOpenChange = vi.fn()
        useGitStore.setState({
            branches: branches({
                local: [{ name: "feature/x", upstream: null, ahead: 0, behind: 0, isCurrent: false, gone: false }]
            })
        })
        render(<BranchPopover open onOpenChange={onOpenChange} />)
        fireEvent.click(screen.getByRole("option", { name: "feature/x" }))
        await waitFor(() => expect(ipc.gitCheckout).toHaveBeenCalledWith("/w", "feature/x"))
        expect(ipc.gitCheckout).not.toHaveBeenCalledWith("/w", "x")
        await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false))
    })

    it("checks out an untracked remote as a correctly named local branch from the exact full ref", async () => {
        useGitStore.setState({
            branches: branches({
                remote: ["origin/feature/x"]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        selectTab(/remote/i)
        fireEvent.click(screen.getByRole("option", { name: "origin/feature/x" }))
        await waitFor(() => expect(ipc.gitCreateBranch).toHaveBeenCalledWith(
            "/w",
            "feature/x",
            "refs/remotes/origin/feature/x"
        ))
        expect(ipc.gitCheckout).not.toHaveBeenCalled()
    })

    it("keeps search, tabs, and copy usable while mutations are blocked", async () => {
        useGitStore.setState({
            busy: "fetch",
            snapshotStale: false,
            branches: branches({
                local: [{ name: "feature/x", upstream: null, ahead: 0, behind: 0, isCurrent: false, gone: false }],
                remote: ["origin/feature/x"]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        expect(screen.getByText(/browse and copy names/i)).toBeInTheDocument()
        expect(screen.getByRole("button", { name: /^pull$/i })).toBeDisabled()
        expect(screen.getByRole("button", { name: /new branch/i })).toBeDisabled()
        expect(within(screen.getByRole("option", { name: "feature/x" })).queryByRole("button")).not.toBeInTheDocument()

        setQuery("feature")
        expect(screen.getByRole("option", { name: "feature/x" })).toBeInTheDocument()
        fireEvent.click(screen.getByRole("option", { name: "feature/x" }))
        expect(ipc.gitCheckout).not.toHaveBeenCalled()

        selectTab(/remote/i)
        fireEvent.click(screen.getByRole("button", { name: "Copy origin/feature/x" }))
        await waitFor(() => expect(writeText).toHaveBeenCalledWith("origin/feature/x"))
        expect(ipc.gitCreateBranch).not.toHaveBeenCalled()
    })

    it("shows browse-only behavior when the snapshot is stale", async () => {
        useGitStore.setState({
            snapshotStale: true,
            branches: branches({
                local: [{ name: "dev", upstream: null, ahead: 0, behind: 0, isCurrent: false, gone: false }]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        expect(screen.getByText(/browse and copy names/i)).toBeInTheDocument()
        expect(screen.getByRole("button", { name: /^fetch$/i })).toBeDisabled()
        fireEvent.click(screen.getByRole("option", { name: "dev" }))
        expect(ipc.gitCheckout).not.toHaveBeenCalled()
        expect(searchField()).not.toBeDisabled()
    })

    it("clears a non-empty search on Escape without a Git mutation", () => {
        useGitStore.setState({
            branches: branches({
                local: [{ name: "feature/x", upstream: null, ahead: 0, behind: 0, isCurrent: false, gone: false }]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        setQuery("feature")
        expect(searchField()).toHaveValue("feature")
        fireEvent.keyDown(searchField(), { key: "Escape" })
        expect(searchField()).toHaveValue("")
        expect(screen.getByText("Git Branches")).toBeInTheDocument()
        expect(ipc.gitCheckout).not.toHaveBeenCalled()
        expect(ipc.gitFetch).not.toHaveBeenCalled()
        expect(ipc.gitCreateBranch).not.toHaveBeenCalled()
    })

    it("uses the prototype first-slash leaf for deep local and remote names", () => {
        useGitStore.setState({
            branches: branches({
                local: [{ name: "feature/team/topic", upstream: null, ahead: 0, behind: 0, isCurrent: false, gone: false }],
                remote: ["origin/feature/x"]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        const local = screen.getByRole("option", { name: "feature/team/topic" })
        expect(local).toHaveAttribute("title", "feature/team/topic")
        expect(within(local).getByText("team/topic")).toBeInTheDocument()
        expect(within(local).queryByText("feature/team/topic")).not.toBeInTheDocument()
        expect(screen.getByText("feature/")).toBeInTheDocument()

        selectTab(/remote/i)
        const remote = screen.getByRole("option", { name: "origin/feature/x" })
        expect(remote).toHaveAttribute("title", "origin/feature/x")
        expect(within(remote).getByText("feature/x")).toBeInTheDocument()
        expect(within(remote).queryByText("origin/feature/x")).not.toBeInTheDocument()
        expect(screen.getByText("origin/")).toBeInTheDocument()
    })

    it("reserves no idle action width and pads the selected/hover row instead", () => {
        useGitStore.setState({
            branches: branches({
                local: [{ name: "feature/x", upstream: null, ahead: 0, behind: 0, isCurrent: false, gone: false }]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        const option = screen.getByRole("option", { name: "feature/x" })
        expect(option).toHaveAttribute("data-has-action", "true")
        expect(option.className).toMatch(/hover:pr-\[4\.5rem\]/)
        expect(option.className).toMatch(/data-selected:pr-\[4\.5rem\]/)
        expect(option.className).not.toMatch(/(?:^|\s)pr-\[4\.5rem\](?:\s|$)/)
        const hint = option.querySelector("[data-row-action]")
        expect(hint).not.toBeNull()
        expect(hint).toHaveAttribute("aria-hidden", "true")
        expect(hint?.className).toMatch(/hidden/)
        expect(hint?.className).toMatch(/group-hover\/command-item:flex/)
        expect(within(option).queryByRole("button")).not.toBeInTheDocument()
    })

    it("labels the Command listbox from the active tab and has no nested tab stops", () => {
        useGitStore.setState({
            branches: branches({
                local: [{ name: "main", upstream: "origin/main", ahead: 0, behind: 0, isCurrent: true, gone: false }],
                remote: ["origin/main"]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        expect(screen.getByRole("listbox", { name: /local branches/i })).toBeInTheDocument()
        expect(within(screen.getByRole("option", { name: "main" })).queryByRole("button")).not.toBeInTheDocument()
        selectTab(/remote/i)
        expect(screen.getByRole("listbox", { name: /remote branches/i })).toBeInTheDocument()
        expect(within(screen.getByRole("option", { name: "origin/main" })).queryByRole("button")).not.toBeInTheDocument()
    })

    it("shows an inline notice when copying a remote name fails", async () => {
        vi.mocked(writeText).mockRejectedValueOnce(new Error("clipboard denied"))
        useGitStore.setState({
            branches: branches({
                remote: ["origin/feature/x"]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        selectTab(/remote/i)
        fireEvent.click(screen.getByRole("button", { name: "Copy origin/feature/x" }))
        expect(await screen.findByText(i18n.t("branchPopover.copyFailed", { ns: "menus" }))).toBeInTheDocument()
    })

    it("checks out the owning local branch for an exactly tracked remote", async () => {
        useGitStore.setState({
            branches: branches({
                local: [{ name: "work/topic", upstream: "upstream/feature/topic", ahead: 0, behind: 0, isCurrent: false, gone: false }],
                remote: ["upstream/feature/topic"]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        selectTab(/remote/i)
        fireEvent.click(screen.getByRole("option", { name: "upstream/feature/topic" }))
        await waitFor(() => expect(ipc.gitCheckout).toHaveBeenCalledWith("/w", "work/topic"))
        expect(ipc.gitCreateBranch).not.toHaveBeenCalled()
    })

    it("lists tag dates, groups by prefix, and searches full tag names only in Tags", () => {
        useGitStore.setState({
            branches: branches({
                local: [{ name: "release/tag-search", upstream: null, ahead: 0, behind: 0, isCurrent: false, gone: false }],
                tags: [
                    { name: "release/desktop-preview", date: "2026-08-01T12:00:00Z" },
                    { name: "v1.0.0", date: "2026-07-01T12:00:00Z" }
                ]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        selectTab(/tags/i)
        expect(screen.getByText("release/")).toBeInTheDocument()
        const tag = screen.getByRole("option", { name: "release/desktop-preview" })
        expect(tag).toHaveAttribute("title", "release/desktop-preview")
        expect(within(tag).getByText("desktop-preview")).toBeInTheDocument()
        expect(within(tag).getByText("2026-08-01T12:00:00Z")).toBeInTheDocument()
        setQuery("release/desk")
        expect(screen.getByRole("option", { name: "release/desktop-preview" })).toHaveTextContent("release/desktop-preview")
        expect(document.querySelector("mark")).toHaveTextContent("release/desk")
        expect(screen.queryByRole("option", { name: "release/tag-search" })).not.toBeInTheDocument()
    })

    it("uses the app-owned confirmation before exact detached tag checkout", async () => {
        useGitStore.setState({
            branches: branches({ tags: [{ name: "v1.0.0", date: "2026-08-01T12:00:00Z" }] })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        selectTab(/tags/i)
        fireEvent.click(screen.getByRole("option", { name: "v1.0.0" }))
        expect(useAppDialogStore.getState().pending).toMatchObject({
            type: "confirm",
            title: "Checkout tag?"
        })
        expect(ipc.gitCheckoutDetached).not.toHaveBeenCalled()
        act(() => useAppDialogStore.getState().respond(true))
        await waitFor(() => expect(ipc.gitCheckoutDetached).toHaveBeenCalledWith("/w", "refs/tags/v1.0.0"))
        expect(ipc.gitCheckout).not.toHaveBeenCalledWith("/w", "v1.0.0")
    })

    it("creates a branch from the exact selected tag through app-owned text input", async () => {
        useGitStore.setState({
            branches: branches({ tags: [{ name: "release/v1.0.0", date: "2026-08-01T12:00:00Z" }] })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        selectTab(/tags/i)
        fireEvent.click(screen.getByRole("button", { name: "Create branch from release/v1.0.0" }))
        expect(useTextInputDialogStore.getState().pending).toMatchObject({
            title: "Create branch from tag",
            description: "Create a new branch starting exactly at release/v1.0.0.",
            initialValue: "release/release/v1.0.0"
        })
        act(() => useTextInputDialogStore.getState().respond("release/1.0.0"))
        await waitFor(() => expect(ipc.gitCreateBranch).toHaveBeenCalledWith(
            "/w",
            "release/1.0.0",
            "refs/tags/release/v1.0.0"
        ))
    })

    it("drops a create-from-tag response after repository authority changes", async () => {
        useGitStore.setState({
            branches: branches({ tags: [{ name: "v1", date: "2026-08-01T12:00:00Z" }] })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        selectTab(/tags/i)
        fireEvent.click(screen.getByRole("button", { name: "Create branch from v1" }))
        act(() => {
            useGitStore.setState({ environment: { status: "ready", root: "/other", version: "2.50" } })
            useTextInputDialogStore.getState().respond("release/v1")
        })
        await waitFor(() => expect(useTextInputDialogStore.getState().pending).toBeNull())
        expect(ipc.gitCreateBranch).not.toHaveBeenCalled()
    })

    it("copies the exact selected remote and tag names from non-nested controls", async () => {
        useGitStore.setState({
            branches: branches({
                remote: ["origin/feature/x"],
                tags: [{ name: "release/v1", date: "2026-08-01T12:00:00Z" }]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        selectTab(/remote/i)
        const remoteCopy = screen.getByRole("button", { name: "Copy origin/feature/x" })
        expect(remoteCopy.closest('[role="option"]')).toBeNull()
        fireEvent.click(remoteCopy)
        await waitFor(() => expect(writeText).toHaveBeenCalledWith("origin/feature/x"))
        selectTab(/tags/i)
        const tagCopy = screen.getByRole("button", { name: "Copy release/v1" })
        expect(tagCopy.closest('[role="option"]')).toBeNull()
        fireEvent.click(tagCopy)
        await waitFor(() => expect(writeText).toHaveBeenCalledWith("release/v1"))
    })

    it("blocks remote, tag, and branch-creation mutations for dirty tabs", async () => {
        useWorkspaceStore.setState({
            groups: [{
                tabs: [{ path: "/w/a.ts", name: "a.ts", dirty: true, externallyModified: false }],
                activePath: "/w/a.ts"
            }],
            activeGroupIndex: 0
        })
        useGitStore.setState({
            branches: branches({
                remote: ["origin/feature/x"],
                tags: [{ name: "v1", date: "2026-08-01T12:00:00Z" }]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        selectTab(/remote/i)
        fireEvent.click(screen.getByRole("option", { name: "origin/feature/x" }))
        expect(await screen.findByText(/unsaved changes/i)).toBeInTheDocument()
        expect(ipc.gitCreateBranch).not.toHaveBeenCalled()
        selectTab(/tags/i)
        fireEvent.click(screen.getByRole("option", { name: "v1" }))
        expect(ipc.gitCheckoutDetached).not.toHaveBeenCalled()
        expect(useAppDialogStore.getState().pending).toBeNull()
        fireEvent.click(screen.getByRole("button", { name: "Create branch from v1" }))
        expect(useTextInputDialogStore.getState().pending).toBeNull()
        fireEvent.click(screen.getByRole("button", { name: /new branch/i }))
        const input = screen.getByPlaceholderText(i18n.t("branchNamePlaceholder", { ns: "git" }))
        fireEvent.change(input, { target: { value: "dirty/new" } })
        fireEvent.keyDown(input, { key: "Enter" })
        expect(ipc.gitCreateBranch).not.toHaveBeenCalled()
    })

    it("keeps copy and navigation available without repository authority", async () => {
        useGitStore.setState({
            environment: null,
            branches: branches({ remote: ["origin/main"] })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        expect(screen.getByText(/browse and copy names/i)).toBeInTheDocument()
        selectTab(/remote/i)
        expect(screen.getByRole("option", { name: "origin/main" })).toBeInTheDocument()
        fireEvent.click(screen.getByRole("button", { name: "Copy origin/main" }))
        await waitFor(() => expect(writeText).toHaveBeenCalledWith("origin/main"))
        expect(ipc.gitCheckout).not.toHaveBeenCalled()
        expect(ipc.gitCreateBranch).not.toHaveBeenCalled()
    })

    it("keeps exact geometry and closes on Escape only after search is clear", () => {
        const onOpenChange = vi.fn()
        useGitStore.setState({
            branches: branches({ local: [{ name: "main", upstream: null, ahead: 0, behind: 0, isCurrent: true, gone: false }] })
        })
        render(<BranchPopover open onOpenChange={onOpenChange} />)
        const content = document.querySelector(".yz-pop")
        expect(content).toHaveClass("w-[340px]", "max-w-[92vw]", "h-[min(68vh,520px)]")
        setQuery("main")
        fireEvent.keyDown(searchField(), { key: "Escape" })
        expect(searchField()).toHaveValue("")
        expect(onOpenChange).not.toHaveBeenCalledWith(false)
        fireEvent.keyDown(searchField(), { key: "Escape" })
        expect(onOpenChange).toHaveBeenCalledWith(false)
    })

    it("clears search and notice when switching tabs", async () => {
        useWorkspaceStore.setState({
            groups: [{
                tabs: [{ path: "/w/a.ts", name: "a.ts", dirty: true, externallyModified: false }],
                activePath: "/w/a.ts"
            }],
            activeGroupIndex: 0
        })
        useGitStore.setState({
            branches: branches({
                local: [{ name: "dev", upstream: null, ahead: 0, behind: 0, isCurrent: false, gone: false }],
                remote: ["origin/dev"]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        setQuery("dev")
        fireEvent.click(screen.getByRole("option", { name: "dev" }))
        expect(await screen.findByText(/unsaved changes/)).toBeInTheDocument()
        expect(searchField()).toHaveValue("dev")
        selectTab(/remote/i)
        expect(searchField()).toHaveValue("")
        expect(screen.queryByText(/unsaved changes/)).not.toBeInTheDocument()
        expect(screen.getByRole("tabpanel")).toBeInTheDocument()
        expect(screen.getByRole("listbox", { name: /remote branches/i })).toBeInTheDocument()
    })

    it("routes declining detached checkout into create-from-tag with a suggested release name", async () => {
        useGitStore.setState({
            branches: branches({ tags: [{ name: "v1.0.0", date: "2026-08-01T12:00:00Z" }] })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        selectTab(/tags/i)
        fireEvent.click(screen.getByRole("option", { name: "v1.0.0" }))
        expect(useAppDialogStore.getState().pending).toMatchObject({ type: "confirm" })
        act(() => useAppDialogStore.getState().respond(false))
        await waitFor(() => expect(useTextInputDialogStore.getState().pending).toMatchObject({
            title: "Create branch from tag",
            initialValue: "release/1.0.0"
        }))
        expect(ipc.gitCheckoutDetached).not.toHaveBeenCalled()
        act(() => useTextInputDialogStore.getState().respond("release/1.0.0"))
        await waitFor(() => expect(ipc.gitCreateBranch).toHaveBeenCalledWith(
            "/w",
            "release/1.0.0",
            "refs/tags/v1.0.0"
        ))
    })

    it("requires Local-tab selection when several locals track one remote", async () => {
        useGitStore.setState({
            branches: branches({
                local: [
                    { name: "topic-a", upstream: "origin/topic", ahead: 0, behind: 0, isCurrent: false, gone: false },
                    { name: "topic-b", upstream: "origin/topic", ahead: 0, behind: 0, isCurrent: false, gone: false }
                ],
                remote: ["origin/topic"]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        selectTab(/remote/i)
        fireEvent.click(screen.getByRole("option", { name: "origin/topic" }))
        expect(await screen.findByText(/multiple local branches track origin\/topic/i)).toBeInTheDocument()
        expect(ipc.gitCheckout).not.toHaveBeenCalled()
        expect(ipc.gitCreateBranch).not.toHaveBeenCalled()
    })

    it("prompts for an alternate local name when the derived remote name already exists", async () => {
        useGitStore.setState({
            branches: branches({
                local: [{ name: "main", upstream: null, ahead: 0, behind: 0, isCurrent: true, gone: false }],
                remote: ["origin/main"]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        selectTab(/remote/i)
        fireEvent.click(screen.getByRole("option", { name: "origin/main" }))
        expect(useTextInputDialogStore.getState().pending).toMatchObject({
            title: "Choose a local branch name",
            description: "main already exists. Enter a different local name for origin/main.",
            initialValue: "main"
        })
        act(() => useTextInputDialogStore.getState().respond("from-origin"))
        await waitFor(() => expect(ipc.gitCreateBranch).toHaveBeenCalledWith(
            "/w",
            "from-origin",
            "refs/remotes/origin/main"
        ))
    })

    it("rechecks a colliding derived name after the alternate-name dialog", async () => {
        useGitStore.setState({
            branches: branches({
                local: [{ name: "main", upstream: null, ahead: 0, behind: 0, isCurrent: true, gone: false }],
                remote: ["origin/main"]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        selectTab(/remote/i)
        fireEvent.click(screen.getByRole("option", { name: "origin/main" }))
        act(() => useTextInputDialogStore.getState().respond("main"))
        expect(await screen.findByText(/main already exists/i)).toBeInTheDocument()
        expect(ipc.gitCreateBranch).not.toHaveBeenCalled()
    })

    it("cancelling create-from-tag does not start a branch", async () => {
        useGitStore.setState({
            branches: branches({ tags: [{ name: "v1.0.0", date: "2026-08-01T12:00:00Z" }] })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        selectTab(/tags/i)
        fireEvent.click(screen.getByRole("button", { name: "Create branch from v1.0.0" }))
        expect(useTextInputDialogStore.getState().pending?.initialValue).toBe("release/1.0.0")
        act(() => useTextInputDialogStore.getState().respond(null))
        await waitFor(() => expect(useTextInputDialogStore.getState().pending).toBeNull())
        expect(ipc.gitCreateBranch).not.toHaveBeenCalled()
        expect(ipc.gitCheckoutDetached).not.toHaveBeenCalled()
    })

    it("associates tabpanels and supports tab arrows plus result Home/End/Enter", async () => {
        useGitStore.setState({
            branches: branches({
                local: [
                    { name: "alpha", upstream: null, ahead: 0, behind: 0, isCurrent: true, gone: false },
                    { name: "beta", upstream: null, ahead: 0, behind: 0, isCurrent: false, gone: false },
                    { name: "gamma", upstream: null, ahead: 0, behind: 0, isCurrent: false, gone: false }
                ],
                remote: ["origin/alpha"],
                tags: [{ name: "v1", date: "2026-08-01T12:00:00Z" }]
            })
        })
        render(<BranchPopover open onOpenChange={() => {}} />)
        expect(screen.getByRole("tabpanel")).toBeInTheDocument()
        const localTab = screen.getByRole("tab", { name: /local 3/i })
        expect(localTab).toHaveAttribute("aria-selected", "true")
        localTab.focus()
        fireEvent.keyDown(localTab, { key: "ArrowRight" })
        await waitFor(() => expect(screen.getByRole("tab", { name: /remote 1/i })).toHaveAttribute("aria-selected", "true"))
        expect(screen.getByRole("listbox", { name: /remote branches/i })).toBeInTheDocument()
        fireEvent.keyDown(screen.getByRole("tab", { name: /remote 1/i }), { key: "Home" })
        await waitFor(() => expect(screen.getByRole("tab", { name: /local 3/i })).toHaveAttribute("aria-selected", "true"))
        fireEvent.keyDown(screen.getByRole("tab", { name: /local 3/i }), { key: "End" })
        await waitFor(() => expect(screen.getByRole("tab", { name: /tags 1/i })).toHaveAttribute("aria-selected", "true"))
        selectTab(/local/i)
        const list = screen.getByRole("listbox", { name: /local branches/i })
        fireEvent.keyDown(list, { key: "End" })
        fireEvent.keyDown(list, { key: "Enter" })
        await waitFor(() => expect(ipc.gitCheckout).toHaveBeenCalledWith("/w", "gamma"))
        fireEvent.keyDown(list, { key: "Home" })
        fireEvent.keyDown(list, { key: "Enter" })
        expect(ipc.gitCheckout).not.toHaveBeenCalledWith("/w", "alpha")
    })

})
