import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import i18n from "@/lib/i18n"
import type { GitStatus } from "@/lib/types"

const originalUserAgent = navigator.userAgent

function setUserAgent(userAgent: string) {
    Object.defineProperty(navigator, "userAgent", {
        configurable: true,
        value: userAgent,
    })
}

function makeStatus(over: Partial<GitStatus> = {}): GitStatus {
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
        inProgress: null,
        ...over
    }
}

function setViewportGeometry(viewport: HTMLElement, height = 600) {
    Object.defineProperty(viewport, "clientHeight", { configurable: true, value: height })
}

vi.mock("@/lib/ipc", () => ({
    gitStage: vi.fn(async () => undefined),
    gitUnstage: vi.fn(async () => undefined),
    gitDiscard: vi.fn(async () => undefined),
    gitCommit: vi.fn(async () => undefined),
    gitStatus: vi.fn(async () => makeStatus()),
    gitBranches: vi.fn(async () => ({ local: [], remote: [], tags: [] })),
    // BranchPopover pulls these in through the shared trigger.
    gitCheckout: vi.fn(async () => undefined),
    gitCheckoutDetached: vi.fn(async () => undefined),
    gitCreateBranch: vi.fn(async () => undefined),
    gitFetch: vi.fn(async () => undefined),
    gitPull: vi.fn(async () => undefined),
    gitPush: vi.fn(async () => undefined)
}))

vi.mock("@/features/logs/userAction", () => ({
    logUserAction: vi.fn(async () => undefined)
}))

const ipc = await import("@/lib/ipc")
const { GitNavContent } = await import("@/app/workbench/GitNavContent")
const { useGitStore, initialGitState, clearGitSnapshots } = await import("@/state/gitStore")
const { useDiffModalStore } = await import("@/state/diffModalStore")
const { useUiStore, uiInitialState } = await import("@/state/uiStore")
const { useWorkspaceStore } = await import("@/state/workspaceStore")
const { useContextMenuStore } = await import("@/state/contextMenuStore")
const { useAppDialogStore } = await import("@/state/appDialogStore")

const READY = { status: "ready", root: "/w", version: "2.50.1" } as const

function setReady(status: Partial<GitStatus> = {}) {
    useGitStore.setState({ environment: READY, status: makeStatus(status) })
}

function resetDiffModal() {
    useDiffModalStore.setState({ open: false, source: null, activeIndex: 0, mode: "unified" })
}

describe("GitNavContent — ready state (E1)", () => {
    beforeEach(async () => {
        await i18n.changeLanguage("en")
        setUserAgent(originalUserAgent)
        clearGitSnapshots()
        useGitStore.setState(initialGitState)
        useUiStore.setState(uiInitialState)
        useContextMenuStore.setState({ request: null, x: 0, y: 0, availabilityRevision: 0 })
        resetDiffModal()
    })
    afterEach(() => {
        cleanup()
        setUserAgent(originalUserAgent)
        void i18n.changeLanguage("en")
        vi.mocked(ipc.gitStatus).mockReset()
        vi.mocked(ipc.gitStage).mockReset()
        vi.mocked(ipc.gitStatus).mockImplementation(async () => makeStatus())
        vi.mocked(ipc.gitStage).mockImplementation(async () => undefined)
        vi.clearAllMocks()
    })

    it("missing → guided setup; not-ready → empty state (unchanged)", () => {
        useGitStore.setState({ environment: { status: "missing", reason: "git not found" } })
        const { rerender } = render(<GitNavContent />)
        expect(screen.getByText("Git not detected")).toBeInTheDocument()

        useGitStore.setState({ environment: null })
        rerender(<GitNavContent />)
        expect(screen.getByText("No repository status")).toBeInTheDocument()
    })

    it("localizes missing-Git guidance without displaying raw backend diagnostics", async () => {
        const reason = "git executable was not found on PATH"
        const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
        await i18n.changeLanguage("zh-TW")
        setUserAgent("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
        useGitStore.setState({ environment: { status: "missing", reason } })

        render(<GitNavContent />)

        expect(screen.getByText("未偵測到 Git")).toBeInTheDocument()
        expect(screen.getByText("Git 目前無法使用。安裝完成後，請重新偵測。")).toBeInTheDocument()
        expect(screen.queryByText(reason)).not.toBeInTheDocument()
        expect(screen.queryByText("xcode-select --install")).not.toBeInTheDocument()
        expect(screen.queryByText("brew install git")).not.toBeInTheDocument()
        expect(screen.getByText("https://git-scm.com/downloads")).toBeInTheDocument()
        expect(warn).toHaveBeenCalledWith("git executable unavailable:", reason)
    })

    it.each([
        {
            locale: "en",
            title: "Git is too old",
            description: "Yuzora requires Git 2.24 or newer. Upgrade Git, then re-detect.",
            hint: "Install or upgrade to Git 2.24+, then click Re-detect.",
        },
        {
            locale: "zh-TW",
            title: "Git 版本過舊",
            description: "Yuzora 需要 Git 2.24 或更新版本。請升級 Git 後重新偵測。",
            hint: "請安裝或升級至 Git 2.24+，然後按「重新偵測」。",
        },
    ])("renders unsupported-version guidance in $locale with interpolated 2.24", async ({
        locale,
        title,
        description,
        hint,
    }) => {
        const reason =
            "git version below 2.24 (requires git switch and --end-of-options): git version 2.23.0"
        const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
        await i18n.changeLanguage(locale)
        useGitStore.setState({
            environment: {
                status: "missing",
                reason,
                kind: "unsupportedVersion",
                minimumVersion: "2.24",
            },
        })

        render(<GitNavContent />)

        expect(screen.getByText(title)).toBeInTheDocument()
        expect(screen.getByText(description)).toBeInTheDocument()
        expect(screen.getByText(hint)).toBeInTheDocument()
        expect(screen.queryByText(reason)).not.toBeInTheDocument()
        expect(screen.queryByText(/\{version\}/)).not.toBeInTheDocument()
        expect(screen.queryByText("https://git-scm.com/downloads")).not.toBeInTheDocument()
        expect(warn).toHaveBeenCalledWith("git executable unavailable:", reason)
    })

    it("shows macOS Git commands only on macOS", () => {
        vi.spyOn(console, "warn").mockImplementation(() => {})
        setUserAgent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)")
        useGitStore.setState({ environment: { status: "missing", reason: "missing" } })

        render(<GitNavContent />)

        expect(screen.getByText("xcode-select --install")).toBeInTheDocument()
        expect(screen.getByText("brew install git")).toBeInTheDocument()
    })

    it("ready with null status shows dedicated localized loading/error/retry states", async () => {
        useWorkspaceStore.setState({ workspacePath: "/w" })
        useGitStore.setState({ environment: READY, status: null, lastError: null })
        const { rerender } = render(<GitNavContent />)
        expect(screen.getByText("Loading repository status…")).toBeInTheDocument()
        expect(screen.getByText("Waiting for the latest changes and branch information.")).toBeInTheDocument()
        expect(screen.queryByText("Working tree clean")).not.toBeInTheDocument()

        useGitStore.setState({ lastError: "status timed out" })
        rerender(<GitNavContent />)
        expect(screen.getByText("Repository status unavailable")).toBeInTheDocument()
        expect(screen.getByText("status timed out")).toBeInTheDocument()
        expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument()

        await i18n.changeLanguage("zh-TW")
        rerender(<GitNavContent />)
        expect(screen.getByText("無法取得儲存庫狀態")).toBeInTheDocument()
    })

    it("renders the commit card: branch pill, ahead/behind, changed pill", () => {
        setReady({
            ahead: 2,
            behind: 1,
            unstaged: [{ path: "b.ts", origPath: null, status: "M" }],
            untracked: ["c.txt"]
        })
        render(<GitNavContent />)
        expect(screen.getByRole("button", { name: "Branches" })).toBeInTheDocument()
        expect(screen.getByText("main")).toBeInTheDocument()
        expect(screen.getByText("↑2")).toBeInTheDocument()
        expect(screen.getByText("↓1")).toBeInTheDocument()
        expect(screen.getByText("U 1")).toBeInTheDocument()
        expect(screen.getByText("? 1")).toBeInTheDocument()
    })

    it("uses the 52px expanded-aware branch anchor and detached presentation", () => {
        setReady({
            detached: true,
            headOid: "abcdef0123456789",
            branch: null,
            upstream: null,
            ahead: 4,
            behind: 3
        })
        render(<GitNavContent />)
        const trigger = screen.getByRole("button", { name: "Branches" })
        expect(trigger).toHaveClass("h-[52px]", "w-full")
        expect(trigger).toHaveAttribute("aria-expanded", "false")
        expect(trigger).toHaveAttribute("title", "detached @ abcdef0 · Detached HEAD")
        expect(screen.getByText("Detached HEAD")).toBeInTheDocument()
        expect(screen.queryByText("↑4")).not.toBeInTheDocument()
        expect(screen.queryByText("↓3")).not.toBeInTheDocument()
        fireEvent.click(trigger)
        expect(trigger).toHaveAttribute("aria-expanded", "true")
    })

    it("owns a single ScrollArea viewport for the ready git nav body", () => {
        setReady({
            staged: [{ path: "a.ts", origPath: null, status: "M" }],
            unstaged: [{ path: "b.ts", origPath: null, status: "M" }],
            untracked: ["c.txt"],
            ahead: 1,
        })
        render(<GitNavContent />)
        const root = screen.getByTestId("git-nav-scroll")
        expect(root).toHaveAttribute("data-slot", "scroll-area")

        const viewports = root.querySelectorAll('[data-slot="scroll-area-viewport"]')
        expect(viewports).toHaveLength(1)
        const viewport = viewports[0] as HTMLElement

        // Only change sections scroll; summary and commit composer remain fixed.
        expect(viewport.contains(screen.getByText("main"))).toBe(false)
        expect(viewport.contains(screen.getByText("↑1"))).toBe(false)
        expect(viewport.contains(screen.getByText("a.ts"))).toBe(true)
        expect(viewport.contains(screen.getByText("b.ts"))).toBe(true)
        expect(viewport.contains(screen.getByText("c.txt"))).toBe(true)
        expect(viewport.contains(screen.getByRole("button", { name: "Commit" }))).toBe(false)
        expect(viewport.contains(screen.getByRole("button", { name: "Review diff" }))).toBe(false)
        expect(screen.getByTestId("git-nav-summary")).toHaveClass("shrink-0")
        expect(screen.getByTestId("git-nav-composer")).toHaveClass("shrink-0")
    })

    it("virtualizes 1,598 sidebar rows with full extent and reveals a far row", async () => {
        setReady({
            unstaged: Array.from({ length: 1598 }, (_, index) => ({
                path: `src/nav-${String(index).padStart(4, "0")}.ts`,
                origPath: null,
                status: "M"
            }))
        })
        render(<GitNavContent />)
        const root = screen.getByTestId("git-nav-scroll")
        const viewport = root.querySelector('[data-slot="scroll-area-viewport"]') as HTMLElement
        const spacer = screen.getByTestId("git-nav-spacer")
        setViewportGeometry(viewport)

        expect(root.querySelectorAll('[data-slot="scroll-area-viewport"]')).toHaveLength(1)
        expect(root.querySelectorAll('button[title^="src/nav-"]').length).toBeLessThan(100)
        expect(spacer).toHaveAttribute("data-virtual-total-height", String(1598 * 32 + 30))
        expect(spacer).toHaveStyle({ height: `${1598 * 32 + 30}px` })
        expect(screen.queryByText("nav-1500.ts")).toBeNull()

        fireEvent.scroll(viewport, { target: { scrollTop: 30 + 1500 * 32 } })
        await waitFor(() => expect(screen.getByText("nav-1500.ts")).toBeInTheDocument())
        expect(root.querySelectorAll('button[title^="src/nav-"]').length).toBeLessThan(100)
    })

    it("section tri-state includes virtualized offscreen rows", () => {
        setReady({
            unstaged: Array.from({ length: 1598 }, (_, index) => ({
                path: `src/select-${String(index).padStart(4, "0")}.ts`,
                origPath: null,
                status: "M"
            }))
        })
        render(<GitNavContent />)
        fireEvent.click(screen.getByLabelText("Select Unstaged"))
        expect(useUiStore.getState().gitChangeSelection).toHaveLength(1598)
        expect(screen.getByLabelText("Select Unstaged")).toHaveAttribute("aria-checked", "true")
        expect(screen.getByText("1598 selected")).toBeInTheDocument()
        expect(screen.queryByText("select-1500.ts")).toBeNull()
    })

    it("hides ahead/behind and changed pill when zero", () => {
        setReady()
        render(<GitNavContent />)
        expect(screen.queryByText(/changed$/)).not.toBeInTheDocument()
        expect(screen.queryByText(/^[↑↓]/)).not.toBeInTheDocument()
    })

    it("Commit enabled only with staged files AND a non-empty message", () => {
        setReady({ staged: [{ path: "a.ts", origPath: null, status: "M" }] })
        render(<GitNavContent />)
        const btn = screen.getByRole("button", { name: "Commit" })
        expect(btn).toBeDisabled()

        fireEvent.change(screen.getByPlaceholderText(/commit message/i), {
            target: { value: "feat: x" }
        })
        expect(btn).toBeEnabled()
    })

    it("commit calls gitCommit and clears the shared message", async () => {
        setReady({ staged: [{ path: "a.ts", origPath: null, status: "M" }] })
        render(<GitNavContent />)
        fireEvent.change(screen.getByPlaceholderText(/commit message/i), {
            target: { value: "feat: x" }
        })
        fireEvent.click(screen.getByRole("button", { name: "Commit" }))
        await waitFor(() => expect(ipc.gitCommit).toHaveBeenCalledWith("/w", "feat: x"))
        await waitFor(() => expect(useGitStore.getState().commitMessage).toBe(""))
    })

    it("Review diff opens the worktree modal with the flattened files", () => {
        setReady({
            staged: [{ path: "a.ts", origPath: null, status: "M" }],
            unstaged: [{ path: "b.ts", origPath: null, status: "M" }]
        })
        render(<GitNavContent />)
        fireEvent.click(screen.getByRole("button", { name: "Review diff" }))
        const s = useDiffModalStore.getState()
        expect(s.open).toBe(true)
        expect(s.source?.type).toBe("worktree")
        expect(s.source?.type === "worktree" && s.source.files.map((f) => f.path)).toEqual([
            "a.ts",
            "b.ts"
        ])
    })

    it("Review diff is disabled with no changes at all", () => {
        setReady()
        render(<GitNavContent />)
        expect(screen.getByRole("button", { name: "Review diff" })).toBeDisabled()
    })

    // Note: after a runOp the mocked gitStatus refresh empties the list, so each
    // test exercises a single per-file/bulk action (a second click would target
    // a now-removed row). Real refresh returns the true status and rows persist.
    it("renders STAGED and UNSTAGED lists", () => {
        setReady({
            staged: [{ path: "a.ts", origPath: null, status: "M" }],
            unstaged: [{ path: "b.ts", origPath: null, status: "M" }]
        })
        render(<GitNavContent />)
        expect(screen.getByText("Staged")).toBeInTheDocument()
        expect(screen.getByText("Unstaged")).toBeInTheDocument()
        expect(screen.getByText("a.ts")).toBeInTheDocument()
        expect(screen.getByText("b.ts")).toBeInTheDocument()
    })

    it("selecting a file then Stage operates only that path", async () => {
        setReady({ unstaged: [{ path: "b.ts", origPath: null, status: "M" }] })
        render(<GitNavContent />)
        fireEvent.click(screen.getByText("b.ts"))
        fireEvent.click(screen.getByRole("button", { name: /^Stage$/ }))
        await waitFor(() => expect(ipc.gitStage).toHaveBeenCalledWith("/w", ["b.ts"]))
    })

    it("selecting a file then Unstage operates only that path", async () => {
        setReady({ staged: [{ path: "a.ts", origPath: null, status: "M" }] })
        render(<GitNavContent />)
        fireEvent.click(screen.getByText("a.ts"))
        fireEvent.click(screen.getByRole("button", { name: /^Unstage$/ }))
        await waitFor(() => expect(ipc.gitUnstage).toHaveBeenCalledWith("/w", ["a.ts"]))
    })

    it("Unstaged Stage all forwards only unstaged paths", async () => {
        setReady({
            staged: [{ path: "a.ts", origPath: null, status: "M" }],
            unstaged: [{ path: "b.ts", origPath: null, status: "M" }],
            untracked: ["c.txt"]
        })
        render(<GitNavContent />)
        fireEvent.click(screen.getByRole("button", { name: /^Stage all$/ }))
        await waitFor(() => expect(ipc.gitStage).toHaveBeenCalledWith("/w", ["b.ts"]))
    })

    it("Untracked Stage all forwards only untracked paths", async () => {
        setReady({
            unstaged: [{ path: "b.ts", origPath: null, status: "M" }],
            untracked: ["c.txt"]
        })
        render(<GitNavContent />)
        fireEvent.click(screen.getByRole("button", { name: "Stage all untracked" }))
        await waitFor(() => expect(ipc.gitStage).toHaveBeenCalledWith("/w", ["c.txt"]))
    })

    it("Unstage all forwards the staged bucket", async () => {
        setReady({
            staged: [
                { path: "a.ts", origPath: null, status: "M" },
                { path: "d.ts", origPath: null, status: "M" }
            ]
        })
        render(<GitNavContent />)
        fireEvent.click(screen.getByRole("button", { name: "Unstage all" }))
        await waitFor(() => expect(ipc.gitUnstage).toHaveBeenCalledWith("/w", ["a.ts", "d.ts"]))
    })

    it("clicking a file row selects it without opening Diff or mutating", () => {
        setReady({
            staged: [{ path: "a.ts", origPath: null, status: "M" }],
            unstaged: [{ path: "b.ts", origPath: null, status: "M" }]
        })
        render(<GitNavContent />)
        fireEvent.click(screen.getByText("b.ts"))
        expect(useDiffModalStore.getState().open).toBe(false)
        expect(ipc.gitStage).not.toHaveBeenCalled()
        expect(useUiStore.getState().gitChangeSelection.map((row) => row.path)).toEqual(["b.ts"])
    })

    it("double-click and Enter open the Diff on the exact side", () => {
        setReady({
            staged: [{ path: "a.ts", origPath: null, status: "M" }],
            unstaged: [{ path: "b.ts", origPath: null, status: "M" }]
        })
        render(<GitNavContent />)
        fireEvent.doubleClick(screen.getByText("b.ts"))
        expect(useDiffModalStore.getState().open).toBe(true)
        expect(useDiffModalStore.getState().activeIndex).toBe(1)
        resetDiffModal()
        fireEvent.keyDown(screen.getByText("a.ts"), { key: "Enter" })
        expect(useDiffModalStore.getState().open).toBe(true)
        expect(useDiffModalStore.getState().activeIndex).toBe(0)
    })

    it("staging a file keeps the Local-changes selection following it (T15)", async () => {
        // The panel had this file selected on the unstaged (changes) side.
        useUiStore.getState().selectGitFile("b.ts", false)
        setReady({ unstaged: [{ path: "b.ts", origPath: null, status: "M" }] })
        vi.mocked(ipc.gitStatus).mockResolvedValueOnce(makeStatus({
            staged: [{ path: "b.ts", origPath: null, status: "M" }]
        }))
        render(<GitNavContent />)
        fireEvent.click(screen.getByRole("button", { name: /^Stage$/ }))
        await waitFor(() => expect(ipc.gitStage).toHaveBeenCalledWith("/w", ["b.ts"]))
        // Selection follows the row to the staged side so the diff re-resolves.
        await waitFor(() => expect(useUiStore.getState().gitSelectedStaged).toBe(true))
        expect(useUiStore.getState().gitSelectedPath).toBe("b.ts")
    })

    it("unstaging a file keeps the Local-changes selection following it (T15)", async () => {
        useUiStore.getState().selectGitFile("a.ts", true)
        setReady({ staged: [{ path: "a.ts", origPath: null, status: "M" }] })
        vi.mocked(ipc.gitStatus).mockResolvedValueOnce(makeStatus({
            unstaged: [{ path: "a.ts", origPath: null, status: "M" }]
        }))
        render(<GitNavContent />)
        fireEvent.click(screen.getByRole("button", { name: /^Unstage$/ }))
        await waitFor(() => expect(ipc.gitUnstage).toHaveBeenCalledWith("/w", ["a.ts"]))
        await waitFor(() => expect(useUiStore.getState().gitSelectedStaged).toBe(false))
        expect(useUiStore.getState().gitSelectedPath).toBe("a.ts")
    })

    it("in-flight refresh during pending stage does not switch MM side; failed stage keeps original", async () => {
        const mm = {
            staged: [{ path: "mm.ts", origPath: null, status: "M" }],
            unstaged: [{ path: "mm.ts", origPath: null, status: "M" }]
        }
        setReady(mm)
        useUiStore.getState().selectGitFile("mm.ts", false)
        render(<GitNavContent />)
        expect(useUiStore.getState().gitSelectedStaged).toBe(false)

        let rejectStage: (error: Error) => void = () => {}
        vi.mocked(ipc.gitStage).mockImplementationOnce(() => new Promise((_resolve, reject) => {
            rejectStage = reject
        }))

        fireEvent.click(screen.getByRole("button", { name: /^Stage$/ }))
        await waitFor(() => expect(ipc.gitStage).toHaveBeenCalledWith("/w", ["mm.ts"]))
        expect(useUiStore.getState().gitSelectedStaged).toBe(false)
        expect(useUiStore.getState().gitSelectedPath).toBe("mm.ts")

        act(() => {
            useGitStore.setState({ status: makeStatus(mm) })
        })
        expect(useUiStore.getState().gitSelectedStaged).toBe(false)
        expect(useUiStore.getState().gitSelectedPath).toBe("mm.ts")

        rejectStage(new Error("stage failed"))
        expect(useUiStore.getState().gitSelectedStaged).toBe(false)
        expect(useUiStore.getState().gitSelectedPath).toBe("mm.ts")
        await waitFor(() => expect(useGitStore.getState().busy).toBeNull())
        expect(useUiStore.getState().gitSelectedStaged).toBe(false)
    })

    it("successful stage remaps MM side before mutation refresh publishes", async () => {
        const mm = {
            staged: [{ path: "mm.ts", origPath: null, status: "M" }],
            unstaged: [{ path: "mm.ts", origPath: null, status: "M" }]
        }
        setReady(mm)
        useUiStore.getState().selectGitFile("mm.ts", false)
        render(<GitNavContent />)

        let resolveStage: () => void = () => {}
        vi.mocked(ipc.gitStage).mockImplementationOnce(() => new Promise((resolve) => {
            resolveStage = () => resolve(undefined)
        }))
        let publishedRefresh = false
        vi.mocked(ipc.gitStatus).mockImplementation(async () => {
            publishedRefresh = true
            return makeStatus({ staged: [{ path: "mm.ts", origPath: null, status: "M" }] })
        })

        fireEvent.click(screen.getByRole("button", { name: /^Stage$/ }))
        await waitFor(() => expect(ipc.gitStage).toHaveBeenCalledWith("/w", ["mm.ts"]))
        expect(useUiStore.getState().gitSelectedStaged).toBe(false)

        resolveStage()
        await waitFor(() => expect(useUiStore.getState().gitSelectedStaged).toBe(true))
        expect(publishedRefresh).toBe(false)
        expect(useUiStore.getState().gitSelectedPath).toBe("mm.ts")
        await waitFor(() => expect(publishedRefresh).toBe(true))
    })

    it("busy second action cannot publish another side move", async () => {
        setReady({ unstaged: [{ path: "b.ts", origPath: null, status: "M" }] })
        useUiStore.getState().selectGitFile("b.ts", false)
        render(<GitNavContent />)
        vi.mocked(ipc.gitStage).mockImplementationOnce(() => new Promise(() => {}))
        fireEvent.click(screen.getByRole("button", { name: /^Stage$/ }))
        await waitFor(() => expect(useGitStore.getState().busy).toBe("stage"))
        const secondHook = vi.fn()
        expect(await useGitStore.getState().runOp("stage", async () => {}, {
            afterMutationBeforeRefresh: secondHook
        })).toBe(false)
        expect(secondHook).not.toHaveBeenCalled()
        expect(useUiStore.getState().gitSelectedStaged).toBe(false)
    })

    it("root switch during a pending stage never remaps the new repository selection", async () => {
        setReady({
            staged: [{ path: "mm.ts", origPath: null, status: "M" }],
            unstaged: [{ path: "mm.ts", origPath: null, status: "M" }]
        })
        useUiStore.getState().selectGitFile("mm.ts", false)
        render(<GitNavContent />)
        let resolveStage: () => void = () => {}
        vi.mocked(ipc.gitStage).mockImplementationOnce(() => new Promise((resolve) => {
            resolveStage = () => resolve(undefined)
        }))
        vi.mocked(ipc.gitStatus).mockImplementation(async () => makeStatus({
            unstaged: [{ path: "mm.ts", origPath: null, status: "M" }]
        }))
        fireEvent.click(screen.getByRole("button", { name: /^Stage$/ }))
        await waitFor(() => expect(ipc.gitStage).toHaveBeenCalled())
        act(() => {
            useGitStore.setState({
                environment: { status: "ready", root: "/other", version: "2.50.1" },
                status: makeStatus({ unstaged: [{ path: "mm.ts", origPath: null, status: "M" }] })
            })
            useUiStore.getState().resetGitRepositoryUi()
            useUiStore.getState().selectGitFile("mm.ts", false)
        })
        resolveStage()
        await waitFor(() => expect(useGitStore.getState().busy).toBeNull())
        expect(useUiStore.getState().gitSelectedPath).toBe("mm.ts")
        expect(useUiStore.getState().gitSelectedStaged).toBe(false)
    })

    it("clicking the CHANGED row of an MM file opens the UNSTAGED side (F2)", () => {
        // A partially-staged file (M staged AND M unstaged) has two rows with the
        // same path. worktreeFilesFrom lists the staged row first, so a path-only
        // active would always land on the staged side. The CHANGED row must pass
        // { path, staged:false } so it opens on the unstaged (index 1) side.
        setReady({
            staged: [{ path: "mm.ts", origPath: null, status: "M" }],
            unstaged: [{ path: "mm.ts", origPath: null, status: "M" }]
        })
        render(<GitNavContent />)
        // Two "mm.ts" rows render (STAGED + CHANGED); the CHANGED one is the last.
        const rows = screen.getAllByText("mm.ts")
        fireEvent.doubleClick(rows[rows.length - 1])
        const s = useDiffModalStore.getState()
        expect(s.open).toBe(true)
        expect(s.activeIndex).toBe(1)

        fireEvent.doubleClick(rows[0])
        expect(useDiffModalStore.getState().activeIndex).toBe(0)
    })

    it("shares plain/Ctrl/Shift selection and right-click preservation with Local Changes", () => {
        setReady({
            staged: [{ path: "a.ts", origPath: null, status: "M" }],
            unstaged: [{ path: "b.ts", origPath: null, status: "M" }],
            untracked: ["c.ts"]
        })
        render(<GitNavContent />)
        fireEvent.click(screen.getByText("a.ts"))
        fireEvent.click(screen.getByText("b.ts"), { ctrlKey: true })
        fireEvent.click(screen.getByText("c.ts"), { shiftKey: true })
        expect(useUiStore.getState().gitChangeSelection.map((row) => row.path)).toEqual([
            "b.ts",
            "c.ts"
        ])
        expect(useDiffModalStore.getState().open).toBe(false)

        fireEvent.contextMenu(screen.getByText("b.ts"), { clientX: 11, clientY: 12 })
        expect(useContextMenuStore.getState().request).toMatchObject({
            kind: "gitChange",
            selected: [{ path: "b.ts" }, { path: "c.ts" }]
        })
        fireEvent.contextMenu(screen.getByText("a.ts"), { clientX: 21, clientY: 22 })
        expect(useUiStore.getState().gitChangeSelection.map((row) => row.path)).toEqual(["a.ts"])
    })

    it("Space toggles the focused row and Cmd+A selects visible rows", () => {
        setReady({
            unstaged: [{ path: "b.ts", origPath: null, status: "M" }],
            untracked: ["c.ts"]
        })
        render(<GitNavContent />)
        fireEvent.click(screen.getByText("b.ts"))
        fireEvent.keyDown(screen.getByText("c.ts"), { key: " " })
        expect(useUiStore.getState().gitChangeSelection.map((row) => row.path)).toEqual(["b.ts", "c.ts"])
        fireEvent.keyDown(screen.getByText("b.ts"), { key: "a", metaKey: true })
        expect(useUiStore.getState().gitChangeSelection.map((row) => row.path)).toEqual(["b.ts", "c.ts"])
    })

    it("section checkbox reports mixed when only some rows are selected", () => {
        setReady({
            unstaged: [
                { path: "b.ts", origPath: null, status: "M" },
                { path: "d.ts", origPath: null, status: "M" }
            ]
        })
        render(<GitNavContent />)
        fireEvent.click(screen.getByText("b.ts"))
        const unstaged = screen.getByLabelText("Select Unstaged")
        expect(unstaged).toHaveAttribute("aria-checked", "mixed")
        expect(unstaged.querySelector("svg.lucide-minus")).not.toBeNull()
    })

    it("section tri-state only changes selection and mixed bulk excludes conflicts", async () => {
        setReady({
            staged: [{ path: "a.ts", origPath: null, status: "M" }],
            unstaged: [{ path: "b.ts", origPath: null, status: "M" }],
            conflicted: [{ path: "conf.ts", origPath: null, status: "U" }]
        })
        render(<GitNavContent />)
        fireEvent.click(screen.getByLabelText("Select Unstaged"))
        expect(useUiStore.getState().gitChangeSelection.map((row) => row.path)).toEqual(["b.ts"])
        expect(ipc.gitStage).not.toHaveBeenCalled()
        fireEvent.click(screen.getByLabelText("Select Conflicts"))
        expect(screen.getByText("2 selected")).toBeInTheDocument()
        expect(screen.getByText("1 conflicts excluded")).toBeInTheDocument()
        fireEvent.click(screen.getByRole("button", { name: /^Stage$/ }))
        await waitFor(() => expect(ipc.gitStage).toHaveBeenCalledWith("/w", ["b.ts"]))
        fireEvent.click(screen.getByRole("button", { name: "Clear selection" }))
        expect(useUiStore.getState().gitChangeSelection).toEqual([])
        expect(screen.queryByTestId("git-nav-bulk")).toBeNull()
    })

    it("busy and stale disable mutations while selection and Diff remain", () => {
        setReady({ unstaged: [{ path: "b.ts", origPath: null, status: "M" }] })
        useGitStore.setState({ busy: "fetch" })
        render(<GitNavContent />)
        fireEvent.click(screen.getByText("b.ts"))
        expect(useUiStore.getState().gitChangeSelection).toHaveLength(1)
        expect(screen.getByRole("button", { name: /^Stage$/ })).toBeDisabled()
        fireEvent.doubleClick(screen.getByText("b.ts"))
        expect(useDiffModalStore.getState().open).toBe(true)
        expect(screen.queryByRole("button", { name: "+" })).toBeNull()
        expect(screen.queryByRole("button", { name: "−" })).toBeNull()
    })

    it("keeps the branch trigger reachable while Git is busy so browse/search stays openable", () => {
        setReady()
        useGitStore.setState({ busy: "fetch" })
        render(<GitNavContent />)
        const trigger = screen.getByRole("button", { name: "Branches" })
        expect(trigger).toBeEnabled()
        expect(trigger).toHaveAttribute("aria-busy", "true")
        fireEvent.click(trigger)
        expect(screen.getByPlaceholderText(i18n.t("branchPopover.searchPlaceholder", { ns: "menus" }))).toBeInTheDocument()
    })

    it("keeps the branch trigger reachable when the snapshot is stale", () => {
        setReady()
        useGitStore.setState({ snapshotStale: true })
        render(<GitNavContent />)
        const trigger = screen.getByRole("button", { name: "Branches" })
        expect(trigger).toBeEnabled()
        fireEvent.click(trigger)
        expect(screen.getByPlaceholderText(i18n.t("branchPopover.searchPlaceholder", { ns: "menus" }))).toBeInTheDocument()
    })

    it("Shift range follows visible conflicts → staged → unstaged → untracked order", () => {
        setReady({
            staged: [{ path: "staged.ts", origPath: null, status: "M" }],
            unstaged: [{ path: "unstaged.ts", origPath: null, status: "M" }],
            untracked: ["new.ts"],
            conflicted: [{ path: "conf.ts", origPath: null, status: "U" }]
        })
        render(<GitNavContent />)
        fireEvent.click(screen.getByText("conf.ts"))
        fireEvent.click(screen.getByText("new.ts"), { shiftKey: true })
        expect(useUiStore.getState().gitChangeSelection.map((row) => row.path)).toEqual([
            "conf.ts",
            "staged.ts",
            "unstaged.ts",
            "new.ts"
        ])
    })

    it("Cmd+A and Shift range skip collapsed section buckets", () => {
        setReady({
            staged: [{ path: "staged.ts", origPath: null, status: "M" }],
            unstaged: [{ path: "unstaged.ts", origPath: null, status: "M" }],
            untracked: ["hidden.ts"]
        })
        render(<GitNavContent />)
        fireEvent.click(screen.getByTestId("git-section-toggle-untracked"))
        fireEvent.click(screen.getByText("staged.ts"))
        fireEvent.keyDown(screen.getByText("staged.ts"), { key: "a", metaKey: true })
        expect(useUiStore.getState().gitChangeSelection.map((row) => row.path)).toEqual([
            "staged.ts",
            "unstaged.ts"
        ])
        fireEvent.click(screen.getByText("staged.ts"))
        fireEvent.click(screen.getByText("unstaged.ts"), { shiftKey: true })
        expect(useUiStore.getState().gitChangeSelection.map((row) => row.path)).toEqual([
            "staged.ts",
            "unstaged.ts"
        ])
    })

    it("pointer checkbox activation leaves focus on the row button", () => {
        setReady({ unstaged: [{ path: "b.ts", origPath: null, status: "M" }] })
        render(<GitNavContent />)
        const row = screen.getByTitle("b.ts")
        const checkbox = row.parentElement?.querySelector("[data-slot=checkbox]") as HTMLElement
        fireEvent.pointerDown(checkbox)
        fireEvent.click(checkbox)
        expect(row).toHaveFocus()
        expect(useUiStore.getState().gitChangeSelection.map((item) => item.path)).toEqual(["b.ts"])
    })
})

describe("GitNavContent — accessible discard confirmation", () => {
    beforeEach(() => {
        clearGitSnapshots()
        useGitStore.setState(initialGitState)
        useAppDialogStore.setState({ pending: null })
        resetDiffModal()
    })
    afterEach(() => {
        cleanup()
        vi.clearAllMocks()
        useAppDialogStore.setState({ pending: null })
    })

    it("discards tracked/untracked only after app-owned confirmation", async () => {
        setReady({
            unstaged: [{ path: "b.ts", origPath: null, status: "M" }],
            untracked: ["c.txt"],
            conflicted: [{ path: "conf.ts", origPath: null, status: "U" }]
        })
        render(<GitNavContent />)
        fireEvent.click(screen.getByRole("button", { name: "Discard all working changes" }))
        expect(useAppDialogStore.getState().pending).toMatchObject({
            type: "confirm",
            destructive: true,
            title: "Discard all working changes?"
        })
        await act(async () => useAppDialogStore.getState().respond(true))
        await waitFor(() => expect(ipc.gitDiscard).toHaveBeenCalledWith("/w", ["b.ts"], ["c.txt"]))
    })

    it("cancel leaves working changes untouched", async () => {
        setReady({ unstaged: [{ path: "b.ts", origPath: null, status: "M" }] })
        render(<GitNavContent />)
        fireEvent.click(screen.getByRole("button", { name: "Discard all working changes" }))
        await act(async () => useAppDialogStore.getState().respond(false))
        expect(ipc.gitDiscard).not.toHaveBeenCalled()
    })

    it("is disabled when there are no discardable working changes", () => {
        setReady({ staged: [{ path: "a.ts", origPath: null, status: "M" }] })
        render(<GitNavContent />)
        expect(screen.getByRole("button", { name: "Discard all working changes" })).toBeDisabled()
    })

    it("cancels discard when status revision/targets change while confirmation is open", async () => {
        setReady({ unstaged: [{ path: "b.ts", origPath: null, status: "M" }] })
        useGitStore.setState({ statusRevision: 1 })
        render(<GitNavContent />)
        fireEvent.click(screen.getByRole("button", { name: "Discard all working changes" }))
        expect(useAppDialogStore.getState().pending?.type).toBe("confirm")
        // Status moves to a different eligible set during the confirmation gap.
        act(() => {
            useGitStore.setState({
                statusRevision: 2,
                status: makeStatus({
                    unstaged: [{ path: "c.ts", origPath: null, status: "M" }],
                }),
            })
        })
        await act(async () => useAppDialogStore.getState().respond(true))
        expect(ipc.gitDiscard).not.toHaveBeenCalled()
    })

    it("confirmed selected discard ignores selection order", async () => {
        setReady({
            unstaged: [
                { path: "a.ts", origPath: null, status: "M" },
                { path: "b.ts", origPath: null, status: "M" }
            ]
        })
        render(<GitNavContent />)
        fireEvent.click(screen.getByText("b.ts"))
        fireEvent.click(screen.getByText("a.ts"), { ctrlKey: true })
        fireEvent.click(screen.getByRole("button", { name: /^Discard$/ }))
        await act(async () => useAppDialogStore.getState().respond(true))
        await waitFor(() => expect(ipc.gitDiscard).toHaveBeenCalledWith("/w", ["b.ts", "a.ts"], []))
    })
})
