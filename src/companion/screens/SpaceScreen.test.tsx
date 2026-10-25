import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { I18nextProvider } from "react-i18next"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { i18n } from "../i18n"
import type { GitBranches, GitLog, GitLogCommit, GitStatus, Snapshot } from "../lib/protocol"
import { RpcError } from "../lib/rpc"
import { navigate, pruneGit, useCompanion } from "../store"

const client = vi.hoisted(() => ({ call: vi.fn() }))
vi.mock("../client", () => ({ call: client.call, retryNow: vi.fn(), refresh: vi.fn() }))
import { SpaceScreen } from "./SpaceScreen"

const snapshot = (git: boolean, withAgent = false, gitGeneration?: string): Snapshot => ({
  session: "work",
  workspaces: [{ id: "w1", label: "Yuzora", status: "idle", git, ...(gitGeneration ? { gitGeneration } : {}) }],
  tabs: withAgent ? [{ id: "w1:t", workspaceId: "w1", label: "main" }] : [],
  panes: withAgent ? [{ id: "w1:p", tabId: "w1:t", workspaceId: "w1", status: "working", label: null, layout: null }] : [],
  agents: withAgent ? [{ paneId: "w1:p", name: "pi", agent: "pi", status: "working" }] : [],
})
const emptyStatus = { branch: "main", headOid: "a".repeat(40), detached: false, upstream: "origin/main", ahead: 1, behind: 0, staged: [], unstaged: [], untracked: [], conflicted: [], inProgress: null }
const status = (change: Partial<GitStatus["status"]> = {}): GitStatus => ({ repo: { name: "yuzora" }, status: { ...emptyStatus, ...change }, truncated: false })
const commit = (hash: string, parents: string[], subject = `commit ${hash}`): GitLogCommit => ({
  hash, shortHash: hash, subject, authorName: "Sora", authorEmail: "sora@example.com", timestamp: 1_760_000_000, parents, refs: [],
})
const branches: GitBranches = {
  local: [
    { name: "feat/x", upstream: null, ahead: 0, behind: 0, isCurrent: false, gone: false },
    { name: "main", upstream: "origin/main", ahead: 2, behind: 1, isCurrent: true, gone: false },
  ],
  remote: ["origin/main", "origin/dev"],
  tags: [{ name: "v1.0.0", date: "2026-10-01T10:00:00+00:00" }],
  truncated: false,
}

/** Answers each Git read with the next value of its list; an `Error` rejects. */
function answer(replies: Record<string, unknown[]>) {
  client.call.mockImplementation(async (method: string) => {
    const next = replies[method]?.shift()
    if (next instanceof Error) throw next
    return next
  })
}
const calls = (method: string) => client.call.mock.calls.filter(([name]) => name === method)
const tab = (name: string) => screen.queryByRole("tab", { name })
// Radix tabs select on mouse down.
const select = (name: string) => fireEvent.mouseDown(screen.getByRole("tab", { name }), { button: 0 })

function renderSpace() {
  render(<I18nextProvider i18n={i18n}><SpaceScreen session="work" workspaceId="w1" /></I18nextProvider>)
}

beforeEach(async () => {
  await i18n.changeLanguage("en")
  client.call.mockReset()
  navigate({ name: "space", session: "work", workspaceId: "w1" })
  useCompanion.setState({ socket: "ready", epoch: 1, git: {}, spaceView: {}, notice: null, snapshots: { work: snapshot(true, true) } })
})
afterEach(cleanup)

describe("Space screen tabs", () => {
  it("has no Git tab for a Space whose repository is not shared", () => {
    useCompanion.setState({ snapshots: { work: snapshot(false, true) } })
    renderSpace()
    expect(tab("Agents")).toBeInTheDocument()
    expect(tab("Git")).toBeNull()
    expect(screen.getByRole("button", { name: /pi/ })).toBeInTheDocument()
    expect(client.call).not.toHaveBeenCalled()
  })

  it("falls back to the agents when a remembered Git tab is no longer shared", () => {
    useCompanion.setState({ snapshots: { work: snapshot(false, true) }, spaceView: { "work\0w1": { tab: "git", view: "changes" } } })
    renderSpace()
    expect(screen.getByRole("tab", { name: "Agents" })).toHaveAttribute("aria-selected", "true")
    expect(client.call).not.toHaveBeenCalled()
  })

  it("offers Agents and Git, opening on the agents when there are some", async () => {
    answer({ "git.status": [status()] })
    renderSpace()
    expect(screen.getByRole("tab", { name: "Agents" })).toHaveAttribute("aria-selected", "true")
    expect(client.call).not.toHaveBeenCalled()
    select("Git")
    await screen.findByText("yuzora")
    expect(calls("git.status")).toEqual([["git.status", { session: "work", workspaceId: "w1" }]])
  })

  it("opens a Space without agents on its Git", async () => {
    useCompanion.setState({ snapshots: { work: snapshot(true) } })
    answer({ "git.status": [status()] })
    renderSpace()
    await screen.findByText("yuzora")
    expect(screen.getByRole("tab", { name: "Git" })).toHaveAttribute("aria-selected", "true")
  })

  it("says so when the Space is gone from the desktop", () => {
    useCompanion.setState({ snapshots: { work: { ...snapshot(true), workspaces: [] } } })
    renderSpace()
    expect(screen.getByText("This Space is no longer open on the desktop.")).toBeInTheDocument()
  })
})

describe("Git changes", () => {
  it("groups conflicts, staged, unstaged and untracked files and opens a file's diff", async () => {
    useCompanion.setState({ snapshots: { work: snapshot(true) } })
    answer({
      "git.status": [status({
        conflicted: [{ path: "src/merge.ts", origPath: null, status: "UU" }],
        staged: [{ path: "src/b.ts", origPath: null, status: "M" }, { path: "src/new.ts", origPath: "src/old.ts", status: "R" }],
        unstaged: [{ path: "c.ts", origPath: null, status: "M" }],
        untracked: ["notes/d.txt"],
      })],
    })
    renderSpace()
    await screen.findByText("Conflicts")
    expect(screen.getAllByRole("heading", { level: 3 }).map((heading) => heading.textContent)).toEqual(["Conflicts1", "Staged2", "Not staged1", "Untracked1"])
    const open = () => useCompanion.getState().screen

    fireEvent.click(screen.getByRole("button", { name: /^b\.ts/ }))
    expect(open()).toEqual({ name: "diff", session: "work", workspaceId: "w1", source: { kind: "staged", path: "src/b.ts", origPath: null } })
    fireEvent.click(screen.getByRole("button", { name: /^new\.ts/ }))
    expect(open()).toMatchObject({ source: { kind: "staged", path: "src/new.ts", origPath: "src/old.ts" } })
    fireEvent.click(screen.getByRole("button", { name: /^c\.ts/ }))
    expect(open()).toMatchObject({ source: { kind: "worktree", path: "c.ts", untracked: false } })
    expect(open()).not.toHaveProperty("source.conflicted")
    fireEvent.click(screen.getByRole("button", { name: /^d\.txt/ }))
    expect(open()).toMatchObject({ source: { kind: "worktree", path: "notes/d.txt", untracked: true } })
    expect(open()).not.toHaveProperty("source.conflicted")
    // Only the Conflicted group tells the diff screen that the file is still unmerged.
    fireEvent.click(screen.getByRole("button", { name: /^merge\.ts/ }))
    expect(open()).toEqual({ name: "diff", session: "work", workspaceId: "w1", source: { kind: "worktree", path: "src/merge.ts", untracked: false, conflicted: true } })
  })

  it("lists an untracked nested repository by its folder name and never opens a diff for it", async () => {
    useCompanion.setState({ snapshots: { work: snapshot(true) } })
    answer({ "git.status": [status({ untracked: ["notes/d.txt", "vendor/lib/", ".claude/worktrees/wt/"] })] })
    renderSpace()
    await screen.findByText("Untracked")
    expect(screen.getAllByRole("heading", { level: 3 }).map((heading) => heading.textContent)).toEqual(["Untracked3"])

    // The folder's own name, a label saying what it is, and nothing to tap.
    const lib = screen.getByText("lib").closest("li")!
    expect(within(lib).getByText("vendor/")).toBeInTheDocument()
    expect(within(lib).getByText("Nested repository")).toBeInTheDocument()
    expect(within(lib).queryByRole("button")).toBeNull()
    expect(within(screen.getByText("wt").closest("li")!).queryByRole("button")).toBeNull()
    expect(screen.getAllByText("Nested repository")).toHaveLength(2)
    expect(screen.getAllByRole("button", { name: /d\.txt/ })).toHaveLength(1)

    fireEvent.click(lib)
    fireEvent.click(screen.getByText("wt"))
    expect(useCompanion.getState().screen).toEqual({ name: "space", session: "work", workspaceId: "w1" })
  })

  it("shows a clean working tree and the branch it is on", async () => {
    useCompanion.setState({ snapshots: { work: snapshot(true) } })
    answer({ "git.status": [status({ ahead: 3, inProgress: "rebase" })] })
    renderSpace()
    expect(await screen.findByText("No changes. The working tree is clean.")).toBeInTheDocument()
    expect(screen.getByText("main")).toBeInTheDocument()
    expect(screen.getByText("Rebase in progress")).toBeInTheDocument()
  })
})

describe("Git history", () => {
  const openHistory = () => {
    useCompanion.setState({ snapshots: { work: snapshot(true) }, spaceView: { "work\0w1": { tab: "git", view: "history" } } })
    renderSpace()
  }
  // Page one ends inside a merge; its second parent only shows up on page two.
  const first: GitLog = { commits: [commit("m", ["a", "b"], "Merge feature"), commit("a", ["c"])], hasMore: true, nextCursor: "next-1" }
  const second: GitLog = { commits: [commit("b", ["c"]), commit("c", [])], hasMore: false, nextCursor: null }
  const nodes = () => [...document.querySelectorAll("svg circle")].map((node) => node.getAttribute("cx"))

  it("lays the lanes out over every page loaded so far", async () => {
    answer({ "git.log": [first, second] })
    openHistory()
    await screen.findByText("Merge feature")
    // Alone, page one has the feature lane open but no commit on it yet.
    expect(nodes()).toEqual(["10", "10"])
    fireEvent.click(screen.getByRole("button", { name: "Load more" }))
    await screen.findByText("commit b")
    expect(calls("git.log")[1]).toEqual(["git.log", { session: "work", workspaceId: "w1", cursor: "next-1" }])
    // b sits on the second lane the merge opened; c is where both lanes meet.
    expect(nodes()).toEqual(["10", "10", "22", "10"])
    expect(screen.queryByRole("button", { name: "Load more" })).toBeNull()
  })

  it("opens a commit from its row", async () => {
    answer({ "git.log": [first] })
    openHistory()
    fireEvent.click(await screen.findByRole("button", { name: /Merge feature/ }))
    expect(useCompanion.getState().screen).toEqual({ name: "commit", session: "work", workspaceId: "w1", hash: "m" })
  })

  it("reloads the first page when the cursor has expired", async () => {
    const fresh: GitLog = { commits: [commit("n", ["m"], "Newest"), commit("m", ["a"], "Merge feature")], hasMore: false, nextCursor: null }
    answer({ "git.log": [first, new RpcError("git-cursor-invalid"), fresh] })
    openHistory()
    await screen.findByText("Merge feature")
    fireEvent.click(screen.getByRole("button", { name: "Load more" }))
    await screen.findByText("Newest")
    expect(calls("git.log").map(([, params]) => (params as { cursor?: string }).cursor)).toEqual([undefined, "next-1", undefined])
    expect(screen.queryByText("commit b")).toBeNull()
    expect(useCompanion.getState().notice).toMatchObject({ tone: "info", key: "git.historyReloaded" })
  })

  it("keeps the loaded pages across views and starts over only on refresh", async () => {
    const fresh: GitLog = { commits: [commit("n", ["m"], "Newest")], hasMore: false, nextCursor: null }
    answer({ "git.log": [first, fresh], "git.status": [status()] })
    openHistory()
    await screen.findByText("Merge feature")
    select("Changes")
    await screen.findByText("yuzora")
    select("History")
    expect(screen.getByText("Merge feature")).toBeInTheDocument()
    expect(calls("git.log")).toHaveLength(1)
    fireEvent.click(screen.getByRole("button", { name: "Refresh Git" }))
    await screen.findByText("Newest")
    expect(calls("git.log").map(([, params]) => (params as { cursor?: string }).cursor)).toEqual([undefined, undefined])
    expect(screen.queryByText("Merge feature")).toBeNull()
  })

  it("shows the new repository's history, not the cached one, when the desktop maps the Space elsewhere", async () => {
    const other: GitLog = { commits: [commit("z", [], "Other repository")], hasMore: false, nextCursor: null }
    answer({ "git.log": [first, other] })
    useCompanion.setState({ snapshots: { work: snapshot(true, false, "g1") }, spaceView: { "work\0w1": { tab: "git", view: "history" } } })
    renderSpace()
    await screen.findByText("Merge feature")
    // The next snapshot names another repository for the same Space.
    const next = snapshot(true, false, "g2")
    act(() => useCompanion.setState((state) => ({ snapshots: { work: next }, git: pruneGit(state.git, "work", next) })))
    await screen.findByText("Other repository")
    expect(screen.queryByText("Merge feature")).toBeNull()
    expect(calls("git.log")).toHaveLength(2)
  })

  it("keeps the loaded commits and offers the load again when a page fails", async () => {
    answer({ "git.log": [first, new RpcError("git-failed"), second] })
    openHistory()
    await screen.findByText("Merge feature")
    fireEvent.click(screen.getByRole("button", { name: "Load more" }))
    expect(await screen.findByText("Git could not read this repository. Try again shortly.")).toBeInTheDocument()
    expect(screen.getByText("Merge feature")).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Load more" }))
    await screen.findByText("commit b")
  })
})

describe("Git branches", () => {
  it("lists the current branch first with its tracking, and folds remotes and tags away", async () => {
    useCompanion.setState({ snapshots: { work: snapshot(true) }, spaceView: { "work\0w1": { tab: "git", view: "branches" } } })
    answer({ "git.branches": [branches] })
    renderSpace()
    await screen.findByText("origin/main", { selector: "span" })
    const rows = within(screen.getByRole("heading", { name: /Local branches/ }).closest("section")!).getAllByRole("listitem")
    expect(rows.map((row) => row.textContent)).toEqual(["mainorigin/main21", "feat/xNo upstream"])
    expect(screen.queryByText("origin/dev")).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: /Remote branches/ }))
    expect(screen.getByText("origin/dev")).toBeInTheDocument()
    expect(screen.queryByText("v1.0.0")).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: /Tags/ }))
    expect(screen.getByText("v1.0.0")).toBeInTheDocument()
  })
})

describe("Git errors", () => {
  beforeEach(() => {
    useCompanion.setState({ snapshots: { work: snapshot(true) } })
  })

  it("asks to trust the workspace on the desktop, then reads again on retry", async () => {
    answer({ "git.status": [new RpcError("git-untrusted"), status()] })
    renderSpace()
    expect(await screen.findByText("Trust this workspace on the desktop first.")).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Try again" }))
    expect(await screen.findByText("yuzora")).toBeInTheDocument()
    expect(screen.queryByText("Trust this workspace on the desktop first.")).toBeNull()
  })

  it("asks in Traditional Chinese on a Chinese phone", async () => {
    await i18n.changeLanguage("zh-TW")
    answer({ "git.status": [new RpcError("git-untrusted")] })
    renderSpace()
    expect(await screen.findByText("請在桌面信任此工作區")).toBeInTheDocument()
  })

  it.each([
    ["git-unavailable", /does not share this Space's folder/],
    ["git-not-repository", /not a Git repository/],
    ["git-failed", /Git could not read this repository/],
    ["busy", /The desktop is busy/],
  ])("explains %s", async (code, message) => {
    answer({ "git.status": [new RpcError(code)] })
    renderSpace()
    expect(await screen.findByText(message)).toBeInTheDocument()
  })

  it("keeps showing what was read when a later read fails", async () => {
    answer({ "git.status": [status()] })
    renderSpace()
    await screen.findByText("yuzora")
    answer({ "git.status": [new RpcError("git-failed")] })
    fireEvent.click(screen.getByRole("button", { name: "Refresh Git" }))
    await waitFor(() => expect(screen.getByText(/Git could not read this repository/)).toBeInTheDocument())
    expect(screen.getByText("yuzora")).toBeInTheDocument()
  })
})
