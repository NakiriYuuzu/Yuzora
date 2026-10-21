import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { I18nextProvider } from "react-i18next"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { i18n } from "../i18n"
import type { GitStatus, Snapshot } from "../lib/protocol"
import { navigate, restoreScreen, useCompanion } from "../store"

const client = vi.hoisted(() => ({ call: vi.fn() }))
vi.mock("../client", () => ({ call: client.call, retryNow: vi.fn(), refresh: vi.fn() }))
import { DiffScreen } from "./DiffScreen"
import { SpaceScreen } from "./SpaceScreen"

// git 2.50 with the host's diff flags: a UU file taken whole from one side prints a bare `--cc` header; once `git add` staged it, status no longer lists the file and the diff is empty.
const TAKEN_WHOLE = "diff --cc f.txt\nindex 85cb8e3,8948000..0000000\n--- a/f.txt\n+++ b/f.txt\n"
const AFTER_ADD = ""
const STILL_UNMERGED = /Unresolved conflict: this file is still unmerged/
const RESOLVED = "This file is no longer in conflict. Go back to refresh the list."

const snapshot: Snapshot = { session: "work", workspaces: [{ id: "w1", label: "Yuzora", status: "idle", git: true }], tabs: [], panes: [], agents: [] }
const statusWith = (conflicted: string[], staged: string[] = []): GitStatus => ({
  repo: { name: "yuzora" },
  status: {
    branch: "main", headOid: "a".repeat(40), detached: false, upstream: null, ahead: 0, behind: 0, untracked: [], unstaged: [], inProgress: "merge",
    staged: staged.map((path) => ({ path, origPath: null, status: "M" })),
    conflicted: conflicted.map((path) => ({ path, origPath: null, status: "UU" })),
  },
  truncated: false,
})
const uuStatus = statusWith(["f.txt"])
// `git add f.txt` ran on the desktop.
const stagedStatus = statusWith([], ["f.txt"])

/** Answers git.diff with the next text of the list and git.status with the next status (the last of each repeats). */
function routes(diffs: string[], statuses: GitStatus[] = [uuStatus]) {
  client.call.mockImplementation(async (method: string) => {
    if (method === "git.status") return statuses.length > 1 ? statuses.shift() : statuses[0]
    if (method === "git.diff") return { text: diffs.length > 1 ? diffs.shift() : diffs[0], truncated: false, binary: false }
    return new Promise(() => {})
  })
}
const statusReads = () => client.call.mock.calls.filter(([method]) => method === "git.status")

/** Mirrors App.tsx: the store's current screen decides what is rendered. */
function Screen() {
  const current = useCompanion((state) => state.screen)
  if (current.name === "space") return <SpaceScreen session={current.session} workspaceId={current.workspaceId} />
  if (current.name === "diff") return <DiffScreen key={JSON.stringify(current.source)} session={current.session} workspaceId={current.workspaceId} source={current.source} />
  return null
}

async function openConflictedRow(name = "f.txt") {
  render(<I18nextProvider i18n={i18n}><Screen /></I18nextProvider>)
  const group = (await screen.findByText("Conflicts")).closest("section")!
  fireEvent.click(within(group).getByText(name))
}

beforeEach(async () => {
  await i18n.changeLanguage("en")
  client.call.mockReset()
  navigate({ name: "space", session: "work", workspaceId: "w1" })
  useCompanion.setState({ socket: "ready", epoch: 1, git: {}, spaceView: { "work\u0000w1": { tab: "git" } } as never, notice: null, snapshots: { work: snapshot } })
  window.addEventListener("popstate", restoreScreen)
})
afterEach(() => {
  window.removeEventListener("popstate", restoreScreen)
  cleanup()
})

it("shows the unmerged notice when a Conflicted row's diff is git's bare --cc header", async () => {
  routes([TAKEN_WHOLE])
  await openConflictedRow()
  expect(await screen.findByText(STILL_UNMERGED)).toBeInTheDocument()
  expect(screen.queryByText(RESOLVED)).toBeNull()
  expect(client.call).toHaveBeenCalledWith("git.diff", { session: "work", workspaceId: "w1", kind: "worktree", path: "f.txt", untracked: false })
})

it("does not claim a stale Conflicted row is still unmerged once the desktop staged the file", async () => {
  // The Git tab only reloads on mount, reconnect or its refresh button: the list still shows UU f.txt. The diff screen's own status read finds it staged.
  routes([AFTER_ADD], [uuStatus, stagedStatus])
  await openConflictedRow()
  expect(await screen.findByText(RESOLVED)).toBeInTheDocument()
  expect(screen.queryByText(STILL_UNMERGED)).toBeNull()
  expect(screen.queryByText("No differences to show.")).toBeNull()
})

it("leaves the Git tab's list fresh behind the resolved hint, not re-read: going back shows the file as staged", async () => {
  // Only the first two status reads are answered, so the list after Back can only be the one the diff screen cached.
  client.call.mockImplementation(async (method: string) => {
    if (method === "git.status") return statusReads().length <= 2 ? (statusReads().length === 1 ? uuStatus : stagedStatus) : new Promise(() => {})
    return { text: AFTER_ADD, truncated: false, binary: false }
  })
  await openConflictedRow()
  expect(await screen.findByText(RESOLVED)).toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Back" }))
  const staged = (await screen.findByText("Staged")).closest("section")!
  expect(within(staged).getByText("f.txt")).toBeInTheDocument()
  expect(screen.queryByText("Conflicts")).toBeNull()
})

it("drops the unmerged notice when the diff screen reloads on a new epoch after the desktop staged the file", async () => {
  routes([TAKEN_WHOLE, AFTER_ADD], [uuStatus, stagedStatus])
  await openConflictedRow()
  expect(await screen.findByText(STILL_UNMERGED)).toBeInTheDocument()
  await act(async () => useCompanion.setState({ epoch: 2 }))
  expect(await screen.findByText(RESOLVED)).toBeInTheDocument()
  expect(client.call.mock.calls.filter(([method]) => method === "git.diff")).toHaveLength(2)
  expect(screen.queryByText(STILL_UNMERGED)).toBeNull()
})

// `caf\xE9.txt` on a Linux host: the list carries the lossy name, the pathspec built from it matches no index entry, and git prints nothing for a file that is still in conflict.
it("keeps the unmerged notice for a non-UTF-8 name git printed nothing for, and the list still shows the conflict after Back", async () => {
  const lossy = "caf\uFFFD.txt"
  routes([AFTER_ADD], [statusWith([lossy])])
  await openConflictedRow(lossy)
  expect(await screen.findByText(STILL_UNMERGED)).toBeInTheDocument()
  expect(screen.queryByText(RESOLVED)).toBeNull()
  expect(screen.queryByText("No differences to show.")).toBeNull()
  expect(client.call).toHaveBeenCalledWith("git.diff", { session: "work", workspaceId: "w1", kind: "worktree", path: lossy, untracked: false })
  fireEvent.click(screen.getByRole("button", { name: "Back" }))
  await waitFor(() => expect(statusReads()).toHaveLength(3))
  expect(within((await screen.findByText("Conflicts")).closest("section")!).getByText(lossy)).toBeInTheDocument()
})

// Resolved with `--ours`, `git add`, then `chmod +x`: the list is stale, and git prints a plain diff with only the mode lines.
it("shows the mode change of a stale Conflicted row that was resolved, staged and chmodded, without calling it unmerged", async () => {
  routes(["diff --git a/f.txt b/f.txt\nold mode 100644\nnew mode 100755\n"])
  await openConflictedRow()
  expect(await screen.findByText("Mode 100644 → 100755")).toBeInTheDocument()
  expect(screen.queryByText(STILL_UNMERGED)).toBeNull()
  expect(screen.queryByText(RESOLVED)).toBeNull()
  expect(screen.queryByText("No differences to show.")).toBeNull()
})
