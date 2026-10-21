import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { I18nextProvider } from "react-i18next"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { i18n } from "../i18n"
import { spaceKey } from "../lib/git"
import type { DiffSource, GitDiff, GitStatus } from "../lib/protocol"
import { RpcError } from "../lib/rpc"
import { useCompanion } from "../store"

const client = vi.hoisted(() => ({ call: vi.fn() }))
vi.mock("../client", () => ({ call: client.call, retryNow: vi.fn() }))
import { DiffScreen } from "./DiffScreen"

const text = [
  "diff --git a/src/a.ts b/src/a.ts",
  "--- a/src/a.ts",
  "+++ b/src/a.ts",
  "@@ -7,3 +7,3 @@ export function a() {",
  " keep",
  "-const old = 1",
  "+const next = 2",
  "\\ No newline at end of file",
  "",
].join("\n")
const worktree: DiffSource = { kind: "worktree", path: "src/a.ts", untracked: false }

/** git.status as the Git tab reads it, listing `conflicted` as UU paths. */
const statusOf = (...conflicted: string[]): GitStatus => ({
  repo: { name: "yuzora" },
  status: {
    branch: "main", headOid: "a".repeat(40), detached: false, upstream: null, ahead: 0, behind: 0,
    staged: [], unstaged: [], untracked: [], conflicted: conflicted.map((path) => ({ path, origPath: null, status: "UU" })), inProgress: "merge",
  },
  truncated: false,
})
const statusReads = () => client.call.mock.calls.filter(([method]) => method === "git.status")

/** `status` answers git.status (a Conflicted row's blank diff asks); by default the desktop has no conflict left. */
function renderDiff(diff: GitDiff | Error, source: DiffSource = worktree, status: GitStatus | Error | Promise<GitStatus> = statusOf()) {
  client.call.mockImplementation(async (method: string) => {
    const reply = method === "git.status" ? await status : diff
    if (reply instanceof Error) throw reply
    return reply
  })
  render(<I18nextProvider i18n={i18n}><DiffScreen session="work" workspaceId="w1" source={source} /></I18nextProvider>)
}

beforeEach(async () => {
  await i18n.changeLanguage("en")
  client.call.mockReset()
  useCompanion.setState({ socket: "ready", epoch: 1, git: {} })
})
afterEach(cleanup)

it("shows hunks with both line numbers and colours the changes", async () => {
  renderDiff({ text, truncated: false, binary: false })
  expect(await screen.findByText("@@ -7,3 +7,3 @@ export function a() {")).toBeInTheDocument()
  expect(client.call).toHaveBeenCalledWith("git.diff", { session: "work", workspaceId: "w1", kind: "worktree", path: "src/a.ts", untracked: false })
  const row = (content: string) => screen.getByText(content).parentElement!
  expect(row("const old = 1").textContent).toBe("8-const old = 1")
  expect(row("const next = 2").textContent).toBe("8+const next = 2")
  expect(row("keep").textContent).toBe("77 keep")
  expect(row("const old = 1").className).toContain("--destructive")
  expect(row("const next = 2").className).toContain("--term-ok")
  expect(screen.getByText("\\ No newline at end of file")).toBeInTheDocument()
  expect(screen.getByText("Working tree changes")).toBeInTheDocument()
})

it("says that a big diff was cut short", async () => {
  renderDiff({ text, truncated: true, binary: false })
  expect(await screen.findByText(/Truncated/)).toBeInTheDocument()
  expect(screen.getByText("const next = 2")).toBeInTheDocument()
})

it("explains a binary file instead of showing bytes", async () => {
  renderDiff({ text: "diff --git a/x.png b/x.png\nBinary files a/x.png and b/x.png differ\n", truncated: false, binary: true })
  expect(await screen.findByText("Binary file: there is no text diff to show.")).toBeInTheDocument()
})

it("says there is nothing to show for an empty diff", async () => {
  renderDiff({ text: "", truncated: false, binary: false })
  expect(await screen.findByText("No differences to show.")).toBeInTheDocument()
})

it("names a rename that changes no content", async () => {
  renderDiff({ text: "diff --git a/old.ts b/new.ts\nsimilarity index 100%\nrename from old.ts\nrename to new.ts\n", truncated: false, binary: false }, { kind: "staged", path: "new.ts", origPath: "old.ts" })
  expect(await screen.findByText("old.ts → new.ts")).toBeInTheDocument()
  expect(screen.getByText("Renamed")).toBeInTheDocument()
  expect(screen.getByText("No content changes.")).toBeInTheDocument()
  expect(client.call).toHaveBeenCalledWith("git.diff", { session: "work", workspaceId: "w1", kind: "staged", path: "new.ts", origPath: "old.ts" })
})

it("says a delete/modify conflict is unresolved instead of claiming nothing changed", async () => {
  renderDiff({ text: "* Unmerged path g.txt\n", truncated: false, binary: false }, { kind: "worktree", path: "g.txt", untracked: false })
  expect(await screen.findByText(/Unresolved conflict: one side deleted or added this file/)).toBeInTheDocument()
  expect(screen.queryByText("No content changes.")).toBeNull()
  expect(screen.queryByText("No differences to show.")).toBeNull()
})

it("says so in Traditional Chinese", async () => {
  await i18n.changeLanguage("zh-TW")
  renderDiff({ text: "* Unmerged path g.txt\n", truncated: false, binary: false }, { kind: "worktree", path: "g.txt", untracked: false })
  expect(await screen.findByText(/未解決的衝突/)).toBeInTheDocument()
  expect(screen.queryByText("內容沒有變更。")).toBeNull()
})

it("shows the file's own diff below the conflict notice when it was edited afterwards", async () => {
  renderDiff({ text: `* Unmerged path src/a.ts\n${text}`, truncated: false, binary: false })
  expect(await screen.findByText(/Unresolved conflict/)).toBeInTheDocument()
  expect(screen.getByText("const next = 2")).toBeInTheDocument()
  expect(screen.queryByText("No content changes.")).toBeNull()
})

// `git diff -- <path>` as the host prints it for a path that `git status` still lists as unmerged (git 2.50, -U3, --no-color).
const conflicted = (path: string): DiffSource => ({ kind: "worktree", path, untracked: false, conflicted: true })
const lines = (...rows: string[]) => `${rows.join("\n")}\n`
// After `git checkout --ours|--theirs f.txt`, or an editor's accept-current/incoming for every hunk: `--cc` hides what matches a parent.
const takenWhole = lines("diff --cc f.txt", "index daf31e1,594dc4f..0000000", "--- a/f.txt", "+++ b/f.txt")
const bothAdded = lines("diff --cc n.txt", "index b19a1e9,950b81b..0000000", "--- a/n.txt", "+++ b/n.txt")
const combinedHeader = lines("diff --combined f.txt", "index daf31e1,594dc4f..0000000", "--- a/f.txt", "+++ b/f.txt")
const deletedByHand = lines("diff --cc f.txt", "index daf31e1,594dc4f..0000000", "deleted file mode 100644,100644", "--- a/f.txt", "+++ /dev/null")
const withMarkers = lines(
  "diff --cc f.txt", "index daf31e1,594dc4f..0000000", "--- a/f.txt", "+++ b/f.txt", "@@@ -1,3 -1,3 +1,7 @@@",
  "  one", "++<<<<<<< HEAD", " +OURS", "++=======", "+ THEIRS", "++>>>>>>> other", "  three",
)
const MATCHED = /Unresolved conflict: this file is still unmerged and the working tree may already match one side/

it.each([
  ["a UU file taken whole from one side", takenWhole, "f.txt"],
  ["an AA file taken whole from one side", bothAdded, "n.txt"],
  ["a UU file deleted without git rm", deletedByHand, "f.txt"],
  ["a file with a bare `--combined` header", combinedHeader, "f.txt"],
])("says %s is still unmerged instead of claiming nothing changed", async (_name, diff, path) => {
  renderDiff({ text: diff, truncated: false, binary: false }, conflicted(path))
  expect(await screen.findByText(MATCHED)).toBeInTheDocument()
  expect(screen.getByText(/finish it on the desktop/i)).toBeInTheDocument()
  expect(screen.queryByText("No content changes.")).toBeNull()
  expect(screen.queryByText("No differences to show.")).toBeNull()
  // The chip of a file deleted by hand would contradict the notice.
  expect(screen.queryByText("Deleted")).toBeNull()
  // The flag is for the screen only: the gateway is asked the same question as for any worktree file.
  expect(client.call).toHaveBeenCalledWith("git.diff", { session: "work", workspaceId: "w1", kind: "worktree", path, untracked: false })
  // The header itself proves the path unmerged: no second read.
  expect(statusReads()).toHaveLength(0)
})

const RESOLVED = "This file is no longer in conflict. Go back to refresh the list."
const blankDiff: GitDiff = { text: "", truncated: false, binary: false }
/** Answers in turn, the last answer repeating: a reload gets the next one. */
const inTurn = <T,>(answers: T[]) => () => (answers.length > 1 ? answers.shift()! : answers[0])

// A blank diff is ambiguous: git prints a header for every path it can match that is still unmerged, but a path that never reached git as it is spelled prints nothing either. git.status says which.
it("tells a Conflicted row with a blank diff that the conflict is gone once git.status no longer lists the file", async () => {
  renderDiff(blankDiff, conflicted("f.txt"), statusOf())
  expect(await screen.findByText(RESOLVED)).toBeInTheDocument()
  expect(screen.queryByText(/Unresolved conflict/)).toBeNull()
  expect(screen.queryByText("No differences to show.")).toBeNull()
  expect(screen.queryByText("No content changes.")).toBeNull()
  // The Git tab's own read, and the list it caches is now the fresh one.
  expect(statusReads()).toEqual([["git.status", { session: "work", workspaceId: "w1" }]])
  expect(useCompanion.getState().git[spaceKey("work", "w1")]?.status?.status.conflicted).toEqual([])
})

it("says so in Traditional Chinese when the conflict is already resolved", async () => {
  await i18n.changeLanguage("zh-TW")
  renderDiff(blankDiff, conflicted("f.txt"), statusOf())
  expect(await screen.findByText("這個檔案已不再衝突。返回上一頁即可重新整理清單。")).toBeInTheDocument()
  expect(screen.queryByText(/未解決的衝突/)).toBeNull()
  expect(screen.queryByText("沒有差異可顯示。")).toBeNull()
})

it("keeps saying a Conflicted row is unmerged when git.status still lists the file, whatever the blank diff", async () => {
  renderDiff(blankDiff, conflicted("f.txt"), statusOf("f.txt", "g.txt"))
  expect(await screen.findByText(MATCHED)).toBeInTheDocument()
  expect(screen.getByText(/finish it on the desktop/i)).toBeInTheDocument()
  expect(screen.queryByText(RESOLVED)).toBeNull()
  expect(screen.queryByText("No differences to show.")).toBeNull()
  expect(screen.queryByText("No content changes.")).toBeNull()
})

it("says so in Traditional Chinese when git.status still lists the file", async () => {
  await i18n.changeLanguage("zh-TW")
  renderDiff(blankDiff, conflicted("f.txt"), statusOf("f.txt"))
  expect(await screen.findByText(/這個檔案仍是未合併狀態，工作樹的內容可能已與其中一方相同/)).toBeInTheDocument()
  expect(screen.queryByText("這個檔案已不再衝突。返回上一頁即可重新整理清單。")).toBeNull()
  expect(screen.queryByText("沒有差異可顯示。")).toBeNull()
})

// The Latin-1 name `caf\xE9.txt`: the host lists it lossily, the literal pathspec built from that listing matches no index entry, git prints nothing, and the list still shows the conflict.
it("does not call a conflict resolved because git printed nothing for a path the list spells lossily", async () => {
  renderDiff(blankDiff, conflicted("caf�.txt"), statusOf("caf�.txt"))
  expect(await screen.findByText(MATCHED)).toBeInTheDocument()
  expect(screen.queryByText(RESOLVED)).toBeNull()
  expect(screen.queryByText("No differences to show.")).toBeNull()
})

it("takes the file for resolved only when its own path is gone from git.status, not any other conflict", async () => {
  renderDiff(blankDiff, conflicted("f.txt"), statusOf("other.txt"))
  expect(await screen.findByText(RESOLVED)).toBeInTheDocument()
  expect(screen.queryByText(MATCHED)).toBeNull()
})

// Whitespace-only text is no output at all: it is never parsed into a header-less file, so the status read decides it like an empty one.
it.each([["a newline", "\n"], ["blank lines and a tab", " \n\t\n\n"]])("treats %s as a blank diff for a Conflicted row", async (_name, blank) => {
  renderDiff({ text: blank, truncated: false, binary: false }, conflicted("f.txt"), statusOf())
  expect(await screen.findByText(RESOLVED)).toBeInTheDocument()
  expect(screen.queryByText(/Unresolved conflict/)).toBeNull()
  expect(screen.queryByText("No differences to show.")).toBeNull()
  expect(screen.queryByText("No content changes.")).toBeNull()
  cleanup()
  renderDiff({ text: blank, truncated: false, binary: false }, conflicted("f.txt"), statusOf("f.txt"))
  expect(await screen.findByText(MATCHED)).toBeInTheDocument()
  expect(screen.queryByText(RESOLVED)).toBeNull()
  expect(screen.queryByText("No differences to show.")).toBeNull()
  expect(screen.queryByText("No content changes.")).toBeNull()
})

it("shows only the resolved hint in Traditional Chinese for a whitespace-only diff of a Conflicted row", async () => {
  await i18n.changeLanguage("zh-TW")
  renderDiff({ text: "\n", truncated: false, binary: false }, conflicted("f.txt"), statusOf())
  expect(await screen.findByText("這個檔案已不再衝突。返回上一頁即可重新整理清單。")).toBeInTheDocument()
  expect(screen.queryByText(/未解決的衝突/)).toBeNull()
  expect(screen.queryByText("內容沒有變更。")).toBeNull()
})

it("keeps saying nothing differs for an empty diff of a file that is not a conflict, without reading git.status", async () => {
  renderDiff(blankDiff)
  expect(await screen.findByText("No differences to show.")).toBeInTheDocument()
  expect(screen.queryByText(/no longer in conflict/)).toBeNull()
  expect(statusReads()).toHaveLength(0)
})

it("says nothing differs for a whitespace-only diff of a file that is not a conflict", async () => {
  renderDiff({ text: "\n", truncated: false, binary: false })
  expect(await screen.findByText("No differences to show.")).toBeInTheDocument()
  expect(screen.queryByText("No content changes.")).toBeNull()
  expect(screen.queryByText(/no longer in conflict/)).toBeNull()
})

it("shows neither message while the status read is in flight, then the one it decides", async () => {
  let answer: (status: GitStatus) => void = () => {}
  renderDiff(blankDiff, conflicted("f.txt"), new Promise<GitStatus>((resolve) => { answer = resolve }))
  await waitFor(() => expect(statusReads()).toHaveLength(1))
  await act(async () => {})
  expect(screen.getByText("Loading…")).toBeInTheDocument()
  expect(screen.queryByText(RESOLVED)).toBeNull()
  expect(screen.queryByText(MATCHED)).toBeNull()
  expect(screen.queryByText("No differences to show.")).toBeNull()
  await act(async () => answer(statusOf("f.txt")))
  expect(await screen.findByText(MATCHED)).toBeInTheDocument()
  expect(screen.queryByText("Loading…")).toBeNull()
  expect(screen.queryByText(RESOLVED)).toBeNull()
})

it("shows the load error and neither message when the status read fails, and decides on retry", async () => {
  renderDiff(blankDiff, conflicted("f.txt"), new RpcError("git-failed"))
  expect(await screen.findByText(/Git could not read this repository/)).toBeInTheDocument()
  expect(screen.queryByText(RESOLVED)).toBeNull()
  expect(screen.queryByText(MATCHED)).toBeNull()
  expect(screen.queryByText("No differences to show.")).toBeNull()
  expect(screen.queryByText("Loading…")).toBeNull()
  client.call.mockImplementation(async (method: string) => (method === "git.status" ? statusOf() : blankDiff))
  fireEvent.click(screen.getByRole("button", { name: "Try again" }))
  expect(await screen.findByText(RESOLVED)).toBeInTheDocument()
  expect(screen.queryByText(/Git could not read this repository/)).toBeNull()
})

it.each([
  ["resolved hint", statusOf(), RESOLVED],
  ["unmerged notice", statusOf("f.txt"), MATCHED],
])("drops the %s for the load error when a reload's status read fails", async (_name, status, message) => {
  renderDiff(blankDiff, conflicted("f.txt"), status)
  expect(await screen.findByText(message)).toBeInTheDocument()
  client.call.mockImplementation(async (method: string) => {
    if (method === "git.status") throw new RpcError("git-failed")
    return blankDiff
  })
  await act(async () => useCompanion.setState({ epoch: 2 }))
  expect(await screen.findByText(/Git could not read this repository/)).toBeInTheDocument()
  expect(screen.queryByText(RESOLVED)).toBeNull()
  expect(screen.queryByText(MATCHED)).toBeNull()
  expect(screen.queryByText("No differences to show.")).toBeNull()
})

const unmergedPath = lines("* Unmerged path f.txt")
const ERROR = /Git could not read this repository/
const noVerdict = () => {
  for (const message of [MATCHED, /Unresolved conflict: one side deleted or added this file/, RESOLVED, "No differences to show.", "No content changes."]) expect(screen.queryByText(message)).toBeNull()
}

// What the first diff said is not what the file is now: a reload that fails shows the error and nothing the earlier diff concluded.
it.each([
  ["a bare `--cc` header", takenWhole, MATCHED],
  ["an `* Unmerged path` line", unmergedPath, /Unresolved conflict: one side deleted or added this file/],
])("shows only the load error when %s was shown and the reload finds a blank diff whose status read fails", async (_name, first, message) => {
  client.call.mockImplementation(async (method: string) => (method === "git.status" ? statusOf("f.txt") : { text: first, truncated: false, binary: false }))
  render(<I18nextProvider i18n={i18n}><DiffScreen session="work" workspaceId="w1" source={conflicted("f.txt")} /></I18nextProvider>)
  expect(await screen.findByText(message)).toBeInTheDocument()
  // The desktop ran `git add f.txt`, the socket came back, and the new status read fails.
  client.call.mockImplementation(async (method: string) => {
    if (method === "git.status") throw new RpcError("git-failed")
    return blankDiff
  })
  await act(async () => useCompanion.setState({ epoch: 2 }))
  expect(await screen.findByText(ERROR)).toBeInTheDocument()
  noVerdict()
})

it.each([
  ["a bare `--cc` header", takenWhole, MATCHED],
  ["an `* Unmerged path` line", unmergedPath, /Unresolved conflict: one side deleted or added this file/],
])("shows only the load error when %s was shown and the reload's diff read fails", async (_name, first, message) => {
  const diffs = inTurn<GitDiff | Error>([{ text: first, truncated: false, binary: false }, new RpcError("git-failed")])
  client.call.mockImplementation(async () => {
    const reply = diffs()
    if (reply instanceof Error) throw reply
    return reply
  })
  render(<I18nextProvider i18n={i18n}><DiffScreen session="work" workspaceId="w1" source={conflicted("f.txt")} /></I18nextProvider>)
  expect(await screen.findByText(message)).toBeInTheDocument()
  await act(async () => useCompanion.setState({ epoch: 2 }))
  expect(await screen.findByText(ERROR)).toBeInTheDocument()
  noVerdict()
})

it.each([
  ["a bare `--cc` header", takenWhole, MATCHED],
  ["an `* Unmerged path` line", unmergedPath, /Unresolved conflict: one side deleted or added this file/],
  ["a blank diff of a file still conflicted", "", MATCHED],
  ["a blank diff of a file no longer conflicted", "", RESOLVED],
])("shows no verdict of the earlier read while a reload is in flight after %s", async (name, first, message) => {
  const status = name.includes("no longer") ? statusOf() : statusOf("f.txt")
  client.call.mockImplementation(async (method: string) => (method === "git.status" ? status : { text: first, truncated: false, binary: false }))
  render(<I18nextProvider i18n={i18n}><DiffScreen session="work" workspaceId="w1" source={conflicted("f.txt")} /></I18nextProvider>)
  expect(await screen.findByText(message)).toBeInTheDocument()
  // The reconnect reload: its diff read is still out.
  let answer: (diff: GitDiff) => void = () => {}
  client.call.mockImplementation((method: string) => (method === "git.status" ? Promise.resolve(status) : new Promise<GitDiff>((resolve) => { answer = resolve })))
  await act(async () => useCompanion.setState({ epoch: 2 }))
  expect(screen.getByText("Loading…")).toBeInTheDocument()
  noVerdict()
  await act(async () => answer({ text: "", truncated: false, binary: false }))
  expect(await screen.findByText(name.includes("no longer") ? RESOLVED : MATCHED)).toBeInTheDocument()
  expect(screen.queryByText("Loading…")).toBeNull()
})

it("shows no verdict of the earlier read while a retry is in flight, then the one the retry decides", async () => {
  client.call.mockImplementation(async (method: string) => (method === "git.status" ? statusOf("f.txt") : blankDiff))
  render(<I18nextProvider i18n={i18n}><DiffScreen session="work" workspaceId="w1" source={conflicted("f.txt")} /></I18nextProvider>)
  expect(await screen.findByText(MATCHED)).toBeInTheDocument()
  client.call.mockImplementation(async (method: string) => {
    if (method === "git.status") throw new RpcError("git-failed")
    return blankDiff
  })
  await act(async () => useCompanion.setState({ epoch: 2 }))
  expect(await screen.findByText(ERROR)).toBeInTheDocument()
  noVerdict()
  // Try again: the status read is in flight, the desktop has staged the file meanwhile.
  let answer: (status: GitStatus) => void = () => {}
  client.call.mockImplementation((method: string) => (method === "git.status" ? new Promise<GitStatus>((resolve) => { answer = resolve }) : Promise.resolve(blankDiff)))
  fireEvent.click(screen.getByRole("button", { name: "Try again" }))
  await waitFor(() => expect(statusReads()).toHaveLength(3))
  await act(async () => {})
  expect(screen.getByText("Loading…")).toBeInTheDocument()
  expect(screen.queryByText(ERROR)).toBeNull()
  noVerdict()
  await act(async () => answer(statusOf()))
  expect(await screen.findByText(RESOLVED)).toBeInTheDocument()
  expect(screen.queryByText(MATCHED)).toBeNull()
})

it("keeps a file's diff on screen while its reload is in flight when it is not a Conflicted row", async () => {
  renderDiff({ text, truncated: false, binary: false })
  expect(await screen.findByText("const next = 2")).toBeInTheDocument()
  client.call.mockImplementation(() => new Promise(() => {}))
  await act(async () => useCompanion.setState({ epoch: 2 }))
  expect(screen.getByText("const next = 2")).toBeInTheDocument()
  expect(screen.queryByText("Loading…")).toBeNull()
})

// What rpc.ts does on a real reconnect: "waiting", then "ready" while hello is still out (the epoch is the old one), then the epoch bump.
const dropSocket = () => act(async () => useCompanion.setState({ socket: "waiting" }))
const socketReady = () => act(async () => useCompanion.setState({ socket: "ready" }))
const helloAnswered = () => act(async () => useCompanion.setState({ epoch: 2 }))

it.each([
  ["a bare `--cc` header", takenWhole, MATCHED],
  ["an `* Unmerged path` line", unmergedPath, /Unresolved conflict: one side deleted or added this file/],
  ["a blank diff of a file still conflicted", "", MATCHED],
])("shows no verdict of the earlier read from the moment the socket drops until the reconnect's result lands after %s", async (_name, first, message) => {
  client.call.mockImplementation(async (method: string) => (method === "git.status" ? statusOf("f.txt") : { text: first, truncated: false, binary: false }))
  render(<I18nextProvider i18n={i18n}><DiffScreen session="work" workspaceId="w1" source={conflicted("f.txt")} /></I18nextProvider>)
  expect(await screen.findByText(message)).toBeInTheDocument()
  // The desktop stages the file while the phone is offline; every diff read now stays out until the test answers it.
  const answers: ((diff: GitDiff) => void)[] = []
  client.call.mockImplementation((method: string) => (method === "git.status" ? Promise.resolve(statusOf()) : new Promise<GitDiff>((resolve) => answers.push(resolve))))
  await dropSocket()
  expect(screen.getByText("Loading…")).toBeInTheDocument()
  noVerdict()
  await socketReady()
  await waitFor(() => expect(answers).toHaveLength(1))
  expect(screen.getByText("Loading…")).toBeInTheDocument()
  noVerdict()
  await helloAnswered()
  await waitFor(() => expect(answers).toHaveLength(2))
  expect(screen.getByText("Loading…")).toBeInTheDocument()
  noVerdict()
  await act(async () => answers.forEach((answer) => answer({ text: "", truncated: false, binary: false })))
  expect(await screen.findByText(RESOLVED)).toBeInTheDocument()
  expect(screen.queryByText("Loading…")).toBeNull()
  expect(screen.queryByText(MATCHED)).toBeNull()
})

it.each([
  ["a bare `--cc` header", takenWhole],
  ["an `* Unmerged path` line", unmergedPath],
])("shows only the load error when the reconnect's reads fail after %s was shown", async (_name, first) => {
  client.call.mockImplementation(async (method: string) => (method === "git.status" ? statusOf("f.txt") : { text: first, truncated: false, binary: false }))
  render(<I18nextProvider i18n={i18n}><DiffScreen session="work" workspaceId="w1" source={conflicted("f.txt")} /></I18nextProvider>)
  await screen.findByText(/Unresolved conflict/)
  client.call.mockImplementation(async (method: string) => {
    if (method === "git.status") throw new RpcError("git-failed")
    return blankDiff
  })
  await dropSocket()
  await socketReady()
  expect(await screen.findByText(ERROR)).toBeInTheDocument()
  noVerdict()
  await helloAnswered()
  expect(await screen.findByText(ERROR)).toBeInTheDocument()
  noVerdict()
})

it("keeps a file's diff on screen across a reconnect when it is not a Conflicted row", async () => {
  renderDiff({ text, truncated: false, binary: false })
  expect(await screen.findByText("const next = 2")).toBeInTheDocument()
  client.call.mockImplementation(() => new Promise(() => {}))
  for (const step of [dropSocket, socketReady, helloAnswered]) {
    await step()
    expect(screen.getByText("const next = 2")).toBeInTheDocument()
    expect(screen.queryByText("Loading…")).toBeNull()
  }
})

it("does not read git.status unless a Conflicted row's diff is blank", async () => {
  const sources: [string, GitDiff, DiffSource][] = [
    ["a blank diff of an unstaged row", blankDiff, worktree],
    ["a blank diff of a staged row", blankDiff, { kind: "staged", path: "f.txt", origPath: null }],
    ["a Conflicted row's hunks", { text: withMarkers, truncated: false, binary: false }, conflicted("f.txt")],
    ["a Conflicted row's bare header", { text: takenWhole, truncated: false, binary: false }, conflicted("f.txt")],
    ["a Conflicted row's unmerged path", { text: lines("* Unmerged path f.txt"), truncated: false, binary: false }, conflicted("f.txt")],
    ["a Conflicted row's binary flag", { text: "", truncated: false, binary: true }, conflicted("f.txt")],
  ]
  for (const [name, diff, source] of sources) {
    client.call.mockReset()
    renderDiff(diff, source)
    await waitFor(() => expect(client.call).toHaveBeenCalledWith("git.diff", expect.anything()))
    await act(async () => {})
    expect(statusReads(), name).toHaveLength(0)
    cleanup()
  }
})

it("replaces the unmerged notice with the resolved hint when the diff reloads blank after a reconnect", async () => {
  const diffs = inTurn<GitDiff>([{ text: takenWhole, truncated: false, binary: false }, blankDiff])
  client.call.mockImplementation(async (method: string) => (method === "git.status" ? statusOf() : diffs()))
  render(<I18nextProvider i18n={i18n}><DiffScreen session="work" workspaceId="w1" source={conflicted("f.txt")} /></I18nextProvider>)
  expect(await screen.findByText(MATCHED)).toBeInTheDocument()
  expect(statusReads()).toHaveLength(0)
  // The phone locked, the desktop ran `git add f.txt`, the socket came back: useLoad reloads with the same source.
  await act(async () => useCompanion.setState({ epoch: 2 }))
  expect(await screen.findByText(RESOLVED)).toBeInTheDocument()
  expect(client.call.mock.calls.filter(([method]) => method === "git.diff")).toHaveLength(2)
  expect(statusReads()).toHaveLength(1)
  expect(screen.queryByText(MATCHED)).toBeNull()
})

it("follows git.status across reloads of a blank diff, in both directions", async () => {
  const statuses = inTurn<GitStatus>([statusOf("f.txt"), statusOf(), statusOf("f.txt")])
  client.call.mockImplementation(async (method: string) => (method === "git.status" ? statuses() : blankDiff))
  render(<I18nextProvider i18n={i18n}><DiffScreen session="work" workspaceId="w1" source={conflicted("f.txt")} /></I18nextProvider>)
  expect(await screen.findByText(MATCHED)).toBeInTheDocument()
  await act(async () => useCompanion.setState({ epoch: 2 }))
  expect(await screen.findByText(RESOLVED)).toBeInTheDocument()
  expect(screen.queryByText(MATCHED)).toBeNull()
  // A merge was started again, or the desktop undid its `git add`.
  await act(async () => useCompanion.setState({ epoch: 3 }))
  expect(await screen.findByText(MATCHED)).toBeInTheDocument()
  expect(screen.queryByText(RESOLVED)).toBeNull()
  expect(statusReads()).toHaveLength(3)
})

it("replaces the resolved hint with the combined diff when a reload finds the conflict markers back", async () => {
  const diffs = inTurn<GitDiff>([blankDiff, { text: withMarkers, truncated: false, binary: false }])
  client.call.mockImplementation(async (method: string) => (method === "git.status" ? statusOf() : diffs()))
  render(<I18nextProvider i18n={i18n}><DiffScreen session="work" workspaceId="w1" source={conflicted("f.txt")} /></I18nextProvider>)
  expect(await screen.findByText(RESOLVED)).toBeInTheDocument()
  await act(async () => useCompanion.setState({ epoch: 2 }))
  expect(await screen.findByText("@@@ -1,3 -1,3 +1,7 @@@")).toBeInTheDocument()
  expect(screen.queryByText(RESOLVED)).toBeNull()
  expect(screen.queryByText(MATCHED)).toBeNull()
})

it("says so in Traditional Chinese for a conflict that already matches one side", async () => {
  await i18n.changeLanguage("zh-TW")
  renderDiff({ text: deletedByHand, truncated: false, binary: false }, conflicted("f.txt"))
  expect(await screen.findByText(/這個檔案仍是未合併狀態，工作樹的內容可能已與其中一方相同/)).toBeInTheDocument()
  expect(screen.queryByText("內容沒有變更。")).toBeNull()
  expect(screen.queryByText("已刪除")).toBeNull()
})

it("keeps the same header-only diff as no content change for a file that is not a conflict", async () => {
  renderDiff({ text: takenWhole, truncated: false, binary: false }, { kind: "worktree", path: "f.txt", untracked: false })
  expect(await screen.findByText("No content changes.")).toBeInTheDocument()
  expect(screen.queryByText(MATCHED)).toBeNull()
})

it("shows the combined diff of a conflict that still has hunks, without the notice", async () => {
  renderDiff({ text: withMarkers, truncated: false, binary: false }, conflicted("f.txt"))
  expect(await screen.findByText("@@@ -1,3 -1,3 +1,7 @@@")).toBeInTheDocument()
  expect(screen.getByText("<<<<<<< HEAD")).toBeInTheDocument()
  expect(screen.getByText("OURS")).toBeInTheDocument()
  expect(screen.queryByText(MATCHED)).toBeNull()
  expect(screen.queryByText(/Unresolved conflict/)).toBeNull()
  expect(screen.queryByText("No content changes.")).toBeNull()
})

it("keeps the delete/modify notice for a UD or DU conflict opened from the Conflicted group", async () => {
  renderDiff({ text: lines("* Unmerged path f.txt"), truncated: false, binary: false }, conflicted("f.txt"))
  expect(await screen.findByText(/Unresolved conflict: one side deleted or added this file/)).toBeInTheDocument()
  expect(screen.queryByText(MATCHED)).toBeNull()
  expect(screen.queryByText("No content changes.")).toBeNull()
  expect(screen.queryByText("No differences to show.")).toBeNull()
})

it("shows a UD file deleted in the working tree with its notice and its own diff", async () => {
  const diff = lines("* Unmerged path f.txt", "diff --git a/f.txt b/f.txt", "deleted file mode 100644", "index ddc897f..0000000", "--- a/f.txt", "+++ /dev/null", "@@ -1,3 +0,0 @@", "-one", "-TWO", "-three")
  renderDiff({ text: diff, truncated: false, binary: false }, conflicted("f.txt"))
  expect(await screen.findByText(/Unresolved conflict: one side deleted or added this file/)).toBeInTheDocument()
  expect(screen.getByText("TWO")).toBeInTheDocument()
  expect(screen.queryByText(MATCHED)).toBeNull()
})

// A conflict resolved and staged on the desktop, then only chmodded: git prints a plain `diff --git` with the mode lines, which says nothing about a conflict.
const modeOnly = lines("diff --git a/f.txt b/f.txt", "old mode 100644", "new mode 100755")

it("shows a plain mode change of a Conflicted row like any other diff, with no unmerged notice", async () => {
  renderDiff({ text: modeOnly, truncated: false, binary: false }, conflicted("f.txt"), statusOf("f.txt"))
  expect(await screen.findByText("Mode 100644 → 100755")).toBeInTheDocument()
  expect(screen.queryByText(/Unresolved conflict/)).toBeNull()
  expect(screen.queryByText(RESOLVED)).toBeNull()
  expect(screen.queryByText("No differences to show.")).toBeNull()
  const conflictBody = document.body.textContent
  cleanup()
  renderDiff({ text: modeOnly, truncated: false, binary: false }, { kind: "worktree", path: "f.txt", untracked: false })
  await screen.findByText("Mode 100644 → 100755")
  expect(document.body.textContent).toBe(conflictBody)
  expect(statusReads()).toHaveLength(0)
})

it("shows a plain `diff --git` of a Conflicted row with hunks like any other diff", async () => {
  renderDiff({ text, truncated: false, binary: false }, { kind: "worktree", path: "src/a.ts", untracked: false, conflicted: true })
  expect(await screen.findByText("const next = 2")).toBeInTheDocument()
  expect(screen.queryByText(/Unresolved conflict/)).toBeNull()
})

it("shows a bare combined header with a mode change as unmerged, since only an unmerged path gets that header", async () => {
  const diff = lines("diff --cc f.txt", "old mode 100644", "new mode 100755", "index daf31e1,594dc4f..0000000", "--- a/f.txt", "+++ b/f.txt")
  renderDiff({ text: diff, truncated: false, binary: false }, conflicted("f.txt"))
  expect(await screen.findByText(MATCHED)).toBeInTheDocument()
  expect(screen.queryByText("Mode 100644 → 100755")).toBeNull()
})

// The three messages answer one question, so the screen can only ever say one of them.
it("never renders more than one of still-unmerged, no-longer-in-conflict and no-differences", async () => {
  const messages = ["Unresolved conflict: one side deleted or added this file", "Unresolved conflict: this file is still unmerged", RESOLVED, "No differences to show."]
  const texts = ["", "\n", " \n\t\n\n", takenWhole, combinedHeader, deletedByHand, modeOnly, withMarkers, lines("* Unmerged path f.txt"), `${lines("* Unmerged path f.txt")}${modeOnly}`, "garbage\n"]
  const cases: [DiffSource, GitStatus][] = [
    [conflicted("f.txt"), statusOf("f.txt")],
    [conflicted("f.txt"), statusOf()],
    [worktree, statusOf("f.txt")],
    [{ kind: "staged", path: "f.txt", origPath: null }, statusOf()],
  ]
  const violations: string[] = []
  for (const text of texts) for (const [source, status] of cases) for (const binary of [false, true]) {
    client.call.mockReset()
    renderDiff({ text, truncated: false, binary }, source, status)
    await waitFor(() => expect(document.body.textContent).not.toContain("Loading…"))
    const shown = messages.filter((message) => document.body.textContent?.includes(message))
    if (shown.length > 1) violations.push(`${JSON.stringify(text)} ${source.kind} binary=${binary}: ${shown.join(" | ")}`)
    cleanup()
  }
  expect(violations).toEqual([])
}, 30_000)

it("labels a commit's diff with the commit", async () => {
  renderDiff({ text, truncated: false, binary: false }, { kind: "commit", hash: "0123456789abcdef", path: "src/a.ts", origPath: "src/old.ts" })
  expect(await screen.findByText("Commit 0123456")).toBeInTheDocument()
  expect(client.call).toHaveBeenCalledWith("git.diff", { session: "work", workspaceId: "w1", kind: "commit", path: "src/a.ts", origPath: "src/old.ts", commit: "0123456789abcdef" })
})

it("reports a failed read and tries again on request", async () => {
  renderDiff(new RpcError("git-failed"))
  expect(await screen.findByText(/Git could not read this repository/)).toBeInTheDocument()
  client.call.mockResolvedValue({ text, truncated: false, binary: false })
  fireEvent.click(screen.getByRole("button", { name: "Try again" }))
  expect(await screen.findByText("const next = 2")).toBeInTheDocument()
})
