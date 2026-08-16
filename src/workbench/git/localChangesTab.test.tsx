import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react"

import type { GitStatus } from "../../lib/types"

function makeStatus(): GitStatus {
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

function setViewportGeometry(viewport: HTMLElement, height = 600) {
    Object.defineProperty(viewport, "clientHeight", { configurable: true, value: height })
}

vi.mock("../../lib/ipc", () => ({
    gitStage: vi.fn(async () => undefined),
    gitUnstage: vi.fn(async () => undefined),
    gitStatus: vi.fn(async () => makeStatus()),
    gitBranches: vi.fn(async () => ({ local: [], remote: [], tags: [] })),
    gitFetch: vi.fn(async () => undefined),
    gitRemoteProbe: vi.fn(async () => "no"),
    gitDetect: vi.fn(async () => ({ status: "ready", root: "/w", version: "2.50.1" })),
    gitDiffContent: vi.fn(async () => ({
        original: { kind: "full", content: "one\n" },
        modified: { kind: "full", content: "two\n" }
    }))
}))

vi.mock("@/features/logs/userAction", () => ({
    logUserAction: vi.fn(async () => undefined)
}))

// Imported after the mocks so the component and tests share the mocked modules.
const ipc = await import("../../lib/ipc")
const { LocalChangesTab } = await import("./LocalChangesTab")
const { useGitStore, initialGitState, clearGitSnapshots } = await import("../../state/gitStore")
const { useUiStore, uiInitialState } = await import("../../state/uiStore")
const { useContextMenuStore } = await import("../../state/contextMenuStore")

describe("LocalChangesTab", () => {
    beforeEach(() => {
        clearGitSnapshots()
        useGitStore.setState({
            ...initialGitState,
            environment: { status: "ready", root: "/w", version: "2.50.1" }
        })
        useUiStore.setState(uiInitialState)
        useContextMenuStore.setState({ request: null, x: 0, y: 0, availabilityRevision: 0 })
    })
    afterEach(() => {
        vi.clearAllMocks()
    })

    it("defaults to split mode on a fresh mount", () => {
        render(<LocalChangesTab />)
        expect(screen.getByRole("radio", { name: "Split" })).toHaveAttribute("aria-checked", "true")
        expect(screen.getByRole("radio", { name: "Unified" })).toHaveAttribute("aria-checked", "false")
    })

    it("keeps the chosen mode sticky across Local Changes remounts", () => {
        const { unmount } = render(<LocalChangesTab />)
        fireEvent.click(screen.getByRole("radio", { name: "Unified" }))
        expect(useUiStore.getState().gitDiffMode).toBe("unified")
        unmount()
        render(<LocalChangesTab />)
        expect(screen.getByRole("radio", { name: "Unified" })).toHaveAttribute("aria-checked", "true")
        expect(screen.getByRole("radio", { name: "Split" })).toHaveAttribute("aria-checked", "false")
    })

    it("renders grouped local rows in conflicts → staged → unstaged → untracked order", () => {
        useGitStore.setState({
            status: {
                ...makeStatus(),
                staged: [{ path: "z.ts", origPath: null, status: "M" }],
                unstaged: [{ path: "b.ts", origPath: null, status: "M" }],
                untracked: ["c.txt"],
                conflicted: [{ path: "conflict.ts", origPath: null, status: "UU" }]
            }
        })
        render(<LocalChangesTab />)
        expect(screen.getByText("Local changes")).toBeInTheDocument()
        expect(screen.getByText("Conflicts")).toBeInTheDocument()
        expect(screen.getByText("Staged")).toBeInTheDocument()
        expect(screen.getByText("Unstaged")).toBeInTheDocument()
        expect(screen.getByText("Untracked")).toBeInTheDocument()

        const names = screen.getAllByRole("option").map((n) => n.textContent)
        expect(names.join(" ")).toMatch(/conflict\.ts.*z\.ts.*b\.ts.*c\.txt/)
    })

    it("has no commit box (moved to the sidebar)", () => {
        useGitStore.setState({
            status: { ...makeStatus(), staged: [{ path: "a.ts", origPath: null, status: "M" }] }
        })
        render(<LocalChangesTab />)
        expect(screen.queryByPlaceholderText(/commit message/i)).not.toBeInTheDocument()
        expect(screen.queryByRole("button", { name: "Commit" })).not.toBeInTheDocument()
    })

    it("stage button handles keyboard activation without selecting its parent row", async () => {
        useGitStore.setState({
            status: { ...makeStatus(), unstaged: [{ path: "key.ts", origPath: null, status: "M" }] }
        })
        render(<LocalChangesTab />)
        const button = screen.getByRole("button", { name: "Stage key.ts" })
        // Sibling button inside a plain list item — not a nested option control.
        expect(button.closest("li")).not.toBeNull()
        expect(button.closest('[role="option"]')).toBeNull()
        button.focus()
        fireEvent.keyDown(button, { key: "Enter" })
        await waitFor(() => expect(ipc.gitStage).toHaveBeenCalledWith("/w", ["key.ts"]))
        expect(useUiStore.getState().gitChangeSelection).toEqual([])
        await waitFor(() => expect(useGitStore.getState().busy).toBeNull())

        // Space path on a fresh row (post-stage refresh empties the prior list).
        act(() => {
            useGitStore.setState({
                status: { ...makeStatus(), unstaged: [{ path: "space.ts", origPath: null, status: "M" }] },
                busy: null
            })
        })
        const spaceButton = screen.getByRole("button", { name: "Stage space.ts" })
        vi.mocked(ipc.gitStage).mockClear()
        spaceButton.focus()
        fireEvent.keyDown(spaceButton, { key: " " })
        await waitFor(() => expect(ipc.gitStage).toHaveBeenCalledWith("/w", ["space.ts"]))
        expect(useUiStore.getState().gitChangeSelection).toEqual([])
    })

    it("stage button forwards path and refreshes via runOp", async () => {
        useGitStore.setState({
            status: { ...makeStatus(), unstaged: [{ path: "b.ts", origPath: null, status: "M" }] }
        })
        render(<LocalChangesTab />)
        fireEvent.click(screen.getByRole("button", { name: "Stage b.ts" }))
        await waitFor(() => expect(ipc.gitStage).toHaveBeenCalledWith("/w", ["b.ts"]))
        await waitFor(() => expect(useGitStore.getState().busy).toBeNull())
    })

    it("staged row unstage button forwards path via runOp", async () => {
        useGitStore.setState({
            status: { ...makeStatus(), staged: [{ path: "a.ts", origPath: null, status: "M" }] }
        })
        render(<LocalChangesTab />)
        fireEvent.click(screen.getByRole("button", { name: "Unstage a.ts" }))
        await waitFor(() => expect(ipc.gitUnstage).toHaveBeenCalledWith("/w", ["a.ts"]))
        await waitFor(() => expect(useGitStore.getState().busy).toBeNull())
    })

    it("Stage all excludes unresolved conflicts", async () => {
        useGitStore.setState({
            status: {
                ...makeStatus(),
                unstaged: [{ path: "b.ts", origPath: null, status: "M" }],
                conflicted: [{ path: "conflict.ts", origPath: null, status: "UU" }]
            }
        })
        render(<LocalChangesTab />)
        expect(screen.getByRole("button", { name: "Stage conflict.ts" })).toBeDisabled()
        fireEvent.click(screen.getByRole("button", { name: "Stage all" }))
        await waitFor(() => expect(ipc.gitStage).toHaveBeenCalledWith("/w", ["b.ts"]))
    })

    it("Stage all forwards only the changed paths", async () => {
        useGitStore.setState({
            status: {
                ...makeStatus(),
                staged: [{ path: "a.ts", origPath: null, status: "M" }],
                unstaged: [{ path: "b.ts", origPath: null, status: "M" }],
                untracked: ["c.txt"]
            }
        })
        render(<LocalChangesTab />)
        fireEvent.click(screen.getByRole("button", { name: "Stage all" }))
        await waitFor(() => expect(ipc.gitStage).toHaveBeenCalledWith("/w", ["b.ts", "c.txt"]))
        await waitFor(() => expect(useGitStore.getState().busy).toBeNull())
    })

    it("row click loads diff for the selected side", async () => {
        useGitStore.setState({
            status: { ...makeStatus(), staged: [{ path: "a.ts", origPath: null, status: "M" }] }
        })
        render(<LocalChangesTab />)
        fireEvent.click(screen.getByText("a.ts"))
        await waitFor(() => expect(ipc.gitDiffContent).toHaveBeenCalledWith("/w", "a.ts", true, null))
    })

    it("reloads same-root same-path same-side diff when statusRevision advances", async () => {
        let resolveSecond: ((value: { original: { kind: "full"; content: string }; modified: { kind: "full"; content: string } }) => void) | null = null
        vi.mocked(ipc.gitDiffContent)
            .mockResolvedValueOnce({
                original: { kind: "full", content: "old one\n" },
                modified: { kind: "full", content: "new one\n" }
            })
            .mockImplementationOnce(() => new Promise((resolve) => { resolveSecond = resolve }))
        useGitStore.setState({
            status: { ...makeStatus(), unstaged: [{ path: "same.ts", origPath: null, status: "M" }] },
            statusRevision: 1
        })
        const { container } = render(<LocalChangesTab />)
        fireEvent.click(screen.getByText("same.ts"))
        await waitFor(() => expect(ipc.gitDiffContent).toHaveBeenNthCalledWith(1, "/w", "same.ts", false, null))
        const editorText = () => Array.from(container.querySelectorAll(".cm-content"), (node) => node.textContent).join("\n")
        await waitFor(() => expect(editorText()).toContain("new one"))

        act(() => useGitStore.setState({ statusRevision: 2 }))
        expect(editorText()).not.toContain("new one")
        expect(screen.getByText("Loading diff…")).toBeInTheDocument()
        expect(ipc.gitDiffContent).toHaveBeenNthCalledWith(2, "/w", "same.ts", false, null)

        await act(async () => {
            resolveSecond?.({
                original: { kind: "full", content: "old two\n" },
                modified: { kind: "full", content: "new two\n" }
            })
            await Promise.resolve()
        })
        await waitFor(() => expect(editorText()).toContain("new two"))
    })

    it("diff header shows the selected file's status badge (§2.5)", async () => {
        useGitStore.setState({
            status: { ...makeStatus(), staged: [{ path: "a.ts", origPath: null, status: "M" }] }
        })
        render(<LocalChangesTab />)
        fireEvent.click(screen.getByText("a.ts"))
        // Scope to the diff header (the container that holds the Unified toggle),
        // so the assertion targets the header badge, not the file row's badge.
        const header = screen.getByRole("radio", { name: "Unified" }).closest("[data-diff-header]")
        expect(header).not.toBeNull()
        await waitFor(() =>
            expect(within(header as HTMLElement).getByText("M")).toBeInTheDocument()
        )
    })

    it("diff header shows language label + +N/−N line stats (§2.5)", async () => {
        // Mocked gitDiffContent → original "one\n", modified "two\n": 1 add, 1 del.
        useGitStore.setState({
            status: { ...makeStatus(), staged: [{ path: "a.ts", origPath: null, status: "M" }] }
        })
        render(<LocalChangesTab />)
        fireEvent.click(screen.getByText("a.ts"))
        const header = screen.getByRole("radio", { name: "Unified" }).closest("[data-diff-header]")
        const h = header as HTMLElement
        await waitFor(() => expect(within(h).getByText("+1")).toBeInTheDocument())
        expect(within(h).getByText("−1")).toBeInTheDocument()
        expect(within(h).getByText("TypeScript")).toBeInTheDocument()
    })

    it("no stats before a file is selected", () => {
        useGitStore.setState({
            status: { ...makeStatus(), unstaged: [{ path: "b.ts", origPath: null, status: "M" }] }
        })
        render(<LocalChangesTab />)
        const header = screen.getByRole("radio", { name: "Unified" }).closest("[data-diff-header]")
        const h = header as HTMLElement
        expect(within(h).queryByText(/^[+−]\d/)).not.toBeInTheDocument()
    })

    it("plain, Ctrl-toggle and Shift-range use the shared flat order", () => {
        useGitStore.setState({
            status: {
                ...makeStatus(),
                staged: [{ path: "a.ts", origPath: null, status: "M" }],
                unstaged: [{ path: "b.ts", origPath: null, status: "M" }],
                untracked: ["c.txt"]
            }
        })
        render(<LocalChangesTab />)
        fireEvent.click(screen.getByText("a.ts"))
        fireEvent.click(screen.getByText("b.ts"), { ctrlKey: true })
        expect(useUiStore.getState().gitChangeSelection.map((row) => row.path)).toEqual([
            "a.ts",
            "b.ts"
        ])

        fireEvent.click(screen.getByText("c.txt"), { shiftKey: true })
        expect(useUiStore.getState().gitChangeSelection.map((row) => row.path)).toEqual([
            "b.ts",
            "c.txt"
        ])
        expect(useUiStore.getState().gitSelectedPath).toBe("c.txt")
    })

    it("partially-staged rows with the same path can both be selected", () => {
        useGitStore.setState({
            status: {
                ...makeStatus(),
                staged: [{ path: "partial.ts", origPath: null, status: "M" }],
                unstaged: [{ path: "partial.ts", origPath: null, status: "M" }]
            }
        })
        render(<LocalChangesTab />)
        const rows = screen.getAllByText("partial.ts")
        fireEvent.click(rows[0])
        fireEvent.click(rows[1], { ctrlKey: true })
        expect(useUiStore.getState().gitChangeSelection.map((row) => row.staged)).toEqual([
            true,
            false
        ])
    })

    it("virtualizes 1,598 options with full extent and reveals a far row", async () => {
        useGitStore.setState({
            status: {
                ...makeStatus(),
                unstaged: Array.from({ length: 1598 }, (_, index) => ({
                    path: `src/virtual-${String(index).padStart(4, "0")}.ts`,
                    origPath: null,
                    status: "M"
                }))
            }
        })
        render(<LocalChangesTab />)
        const root = screen.getByTestId("local-changes-scroll")
        const viewport = root.querySelector('[data-slot="scroll-area-viewport"]') as HTMLElement
        const spacer = screen.getByTestId("local-changes-spacer")
        setViewportGeometry(viewport)

        expect(root.querySelectorAll('[data-slot="scroll-area-viewport"]')).toHaveLength(1)
        expect(screen.getAllByRole("option").length).toBeLessThan(100)
        expect(spacer).toHaveAttribute("data-virtual-total-height", String(1598 * 32 + 30))
        expect(spacer).toHaveStyle({ height: `${1598 * 32 + 30}px` })
        expect(screen.queryByRole("option", { name: /virtual-1500\.ts/ })).toBeNull()

        fireEvent.scroll(viewport, { target: { scrollTop: 30 + 1500 * 32 } })
        await waitFor(() => {
            expect(screen.getByRole("option", { name: /virtual-1500\.ts/ })).toBeInTheDocument()
        })
        expect(screen.getAllByRole("option").length).toBeLessThan(100)
    })

    it("preserves the top visible row and within-row offset across a status snapshot replacement", async () => {
        const paths = Array.from({ length: 1598 }, (_, index) => ({
            path: `src/anchor-${String(index).padStart(4, "0")}.ts`,
            origPath: null,
            status: "M"
        }))
        useGitStore.setState({
            status: { ...makeStatus(), unstaged: paths }
        })
        render(<LocalChangesTab />)
        const root = screen.getByTestId("local-changes-scroll")
        const viewport = root.querySelector('[data-slot="scroll-area-viewport"]') as HTMLElement
        setViewportGeometry(viewport)
        const anchoredScrollTop = 30 + 1500 * 32 + 11
        fireEvent.scroll(viewport, { target: { scrollTop: anchoredScrollTop } })
        await waitFor(() => {
            expect(screen.getByRole("option", { name: /anchor-1500\.ts/ })).toBeInTheDocument()
        })

        act(() => {
            useGitStore.setState({
                status: {
                    ...makeStatus(),
                    unstaged: paths.map((entry) => ({ ...entry }))
                },
                statusRevision: useGitStore.getState().statusRevision + 1
            })
        })

        await waitFor(() => expect(viewport.scrollTop).toBe(anchoredScrollTop))
        expect(screen.getByRole("option", { name: /anchor-1500\.ts/ })).toBeInTheDocument()
    })

    it("End scrolls before focusing a far option and keeps aria-activedescendant mounted", async () => {
        useGitStore.setState({
            status: {
                ...makeStatus(),
                unstaged: Array.from({ length: 1598 }, (_, index) => ({
                    path: `src/key-${String(index).padStart(4, "0")}.ts`,
                    origPath: null,
                    status: "M"
                }))
            }
        })
        render(<LocalChangesTab />)
        const root = screen.getByTestId("local-changes-scroll")
        const viewport = root.querySelector('[data-slot="scroll-area-viewport"]') as HTMLElement
        setViewportGeometry(viewport)
        const first = screen.getByRole("option", { name: /key-0000\.ts/ })
        first.focus()
        fireEvent.keyDown(first, { key: "End" })

        const last = await screen.findByRole("option", { name: /key-1597\.ts/ })
        expect(last).toHaveFocus()
        expect(screen.getByRole("listbox")).toHaveAttribute(
            "aria-activedescendant",
            "local-file-c-src_2fkey-1597.ts"
        )
        expect(viewport.scrollTop).toBeGreaterThan(0)
    })

    it("shows a file filter only after 15 files and filters visible rows only", () => {
        useGitStore.setState({
            status: {
                ...makeStatus(),
                unstaged: Array.from({ length: 16 }, (_, i) => ({ path: `f${i}.ts`, origPath: null, status: "M" }))
            }
        })
        render(<LocalChangesTab />)
        const filter = screen.getByLabelText("Filter files")
        fireEvent.change(filter, { target: { value: "f15" } })
        expect(screen.getByRole("option", { name: /f15\.ts/ })).toBeInTheDocument()
        expect(screen.queryByRole("option", { name: /f0\.ts/ })).toBeNull()
        expect(useUiStore.getState().gitChangeSelection).toEqual([])
    })

    it("uses listbox keyboard navigation against uiStore selection", () => {
        useGitStore.setState({
            status: {
                ...makeStatus(),
                staged: [{ path: "a.ts", origPath: null, status: "M" }],
                unstaged: [{ path: "b.ts", origPath: null, status: "M" }]
            }
        })
        render(<LocalChangesTab />)
        const first = screen.getByRole("option", { name: /a\.ts/ })
        first.focus()
        fireEvent.keyDown(first, { key: "ArrowDown" })
        expect(useUiStore.getState().gitSelectedPath).toBe("b.ts")
        expect(screen.getByRole("listbox", { name: "Changed files" })).toBeInTheDocument()
    })

    it("keeps a mounted roving tabindex when the selected file is filtered out", () => {
        useGitStore.setState({
            status: {
                ...makeStatus(),
                unstaged: Array.from({ length: 16 }, (_, i) => ({ path: `f${i}.ts`, origPath: null, status: "M" }))
            }
        })
        render(<LocalChangesTab />)
        fireEvent.click(screen.getByRole("option", { name: /f1\.ts/ }))
        expect(useUiStore.getState().gitSelectedPath).toBe("f1.ts")
        fireEvent.change(screen.getByLabelText("Filter files"), { target: { value: "f0" } })
        const visible = screen.getByRole("option", { name: /f0\.ts/ })
        expect(visible).toHaveAttribute("tabindex", "0")
        expect(screen.getByRole("listbox")).toHaveAttribute("aria-activedescendant", "local-file-c-f0.ts")
        expect(useUiStore.getState().gitSelectedPath).toBe("f1.ts")
        expect(screen.queryByRole("option", { name: /f1\.ts/ })).toBeNull()
    })

    it("syncs collapsed chrome from the panel API, not only the toggle button", async () => {
        render(<LocalChangesTab />)
        const panel = screen.getByTestId("local-files") as HTMLElement & {
            __yzPanel?: { collapse: () => void; expand: () => void }
        }
        expect(panel.__yzPanel).toBeTruthy()
        expect(screen.getByTestId("local-changes-list").contains(screen.getByRole("button", { name: "Collapse file list" }))).toBe(false)
        act(() => panel.__yzPanel?.collapse())
        await waitFor(() => expect(screen.getByRole("button", { name: "Expand file list" })).toBeInTheDocument())
        expect(screen.getByTestId("local-changes-list").contains(screen.getByRole("button", { name: "Expand file list" }))).toBe(false)
        expect(panel).toHaveAttribute("data-files-collapsed", "true")
        act(() => panel.__yzPanel?.expand())
        await waitFor(() => expect(screen.getByRole("button", { name: "Collapse file list" })).toHaveAttribute("aria-expanded", "true"))
        expect(screen.getByTestId("local-files-handle")).toBeInTheDocument()
        expect(screen.getByTestId("local-files-handle")).not.toHaveAttribute("disabled")
        expect(screen.getByRole("button", { name: "Collapse file list" })).toHaveAttribute("aria-controls", "local-changes-list")
    })

    it("hides a collapsed large file list from accessibility and moves focus to expand", async () => {
        useGitStore.setState({
            status: {
                ...makeStatus(),
                unstaged: Array.from({ length: 16 }, (_, i) => ({ path: `f${i}.ts`, origPath: null, status: "M" }))
            }
        })
        render(<LocalChangesTab />)
        fireEvent.click(screen.getByRole("option", { name: /f3\.ts/ }))
        expect(useUiStore.getState().gitSelectedPath).toBe("f3.ts")
        const collapse = screen.getByRole("button", { name: "Collapse file list" })
        collapse.focus()
        fireEvent.click(collapse)
        const expand = await screen.findByRole("button", { name: "Expand file list" })
        const content = screen.getByTestId("local-changes-list")
        expect(content).toHaveAttribute("inert")
        expect(content).toHaveAttribute("aria-hidden", "true")
        expect(screen.queryByRole("option")).toBeNull()
        expect(screen.queryByRole("textbox", { name: "Filter files" })).toBeNull()
        expect(expand).toHaveFocus()
        expect(useUiStore.getState().gitSelectedPath).toBe("f3.ts")
        fireEvent.click(expand)
        await waitFor(() => {
            expect(screen.getByRole("button", { name: "Collapse file list" })).toHaveAttribute("aria-expanded", "true")
        })
        expect(screen.getByTestId("local-changes-list")).not.toHaveAttribute("aria-hidden")
        expect(screen.getByRole("option", { name: /f3\.ts/ })).toBeInTheDocument()
        expect(screen.getByLabelText("Filter files")).toBeInTheDocument()
        expect(useUiStore.getState().gitSelectedPath).toBe("f3.ts")
        expect(screen.getByRole("button", { name: "Collapse file list" })).toHaveAttribute("aria-controls", "local-changes-list")
    })

    it("uses a safe option id for paths with spaces", () => {
        useGitStore.setState({
            status: {
                ...makeStatus(),
                unstaged: [{ path: "my file.ts", origPath: null, status: "M" }]
            }
        })
        render(<LocalChangesTab />)
        fireEvent.click(screen.getByRole("option", { name: /my file\.ts/ }))
        expect(screen.getByRole("option", { name: /my file\.ts/ })).toHaveAttribute("id", "local-file-c-my_20file.ts")
        expect(screen.getByRole("listbox")).toHaveAttribute("aria-activedescendant", "local-file-c-my_20file.ts")
    })

    it("collapses the file handle sliver without leaving a focusable separator", async () => {
        render(<LocalChangesTab />)
        const handle = screen.getByTestId("local-files-handle")
        expect(handle.className).not.toMatch(/w-0/)
        act(() => {
            (screen.getByTestId("local-files") as HTMLElement & { __yzPanel?: { collapse: () => void } }).__yzPanel?.collapse()
        })
        await waitFor(() => expect(screen.getByTestId("local-files-handle")).toHaveAttribute("aria-disabled", "true"))
        expect(screen.getByTestId("local-files-handle")).toHaveAttribute("aria-hidden", "true")
        expect(screen.getByTestId("local-files-handle").className).toMatch(/w-0/)
        expect(screen.getByTestId("local-files-handle").className).toMatch(/pointer-events-none/)
    })

    it("right-click preserves selected multi rows and replaces it for an unselected row", () => {
        useGitStore.setState({
            environment: { status: "ready", root: "/w", version: "2.50.1" },
            status: { ...makeStatus(), untracked: ["a.ts", "b.ts", "c.ts"] }
        })
        render(<LocalChangesTab />)
        fireEvent.click(screen.getByText("a.ts"))
        fireEvent.click(screen.getByText("b.ts"), { ctrlKey: true })
        fireEvent.contextMenu(screen.getByText("a.ts"), { clientX: 10, clientY: 20 })
        expect(useUiStore.getState().gitChangeSelection).toHaveLength(2)
        expect(useContextMenuStore.getState().request).toMatchObject({
            kind: "gitChange",
            repositoryRoot: "/w",
            clicked: { path: "a.ts" },
            selected: [{ path: "a.ts" }, { path: "b.ts" }]
        })

        fireEvent.contextMenu(screen.getByText("c.ts"), { clientX: 30, clientY: 40 })
        expect(useUiStore.getState().gitChangeSelection.map((row) => row.path)).toEqual(["c.ts"])
        expect(useContextMenuStore.getState().request).toMatchObject({
            clicked: { path: "c.ts" },
            selected: [{ path: "c.ts" }]
        })
    })
})
