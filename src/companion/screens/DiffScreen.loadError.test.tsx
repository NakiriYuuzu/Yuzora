import { cleanup, render, screen } from "@testing-library/react"
import { I18nextProvider } from "react-i18next"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { i18n } from "../i18n"
import type { DiffSource, GitDiff } from "../lib/protocol"

// The hook is stood in for so the screen can be handed a result next to an error, whatever the hook itself would keep.
const hook = vi.hoisted(() => ({ outcome: { data: null, error: null } as { data: unknown; error: string | null } }))
vi.mock("../components/useLoad", () => ({ useLoad: () => ({ ...hook.outcome, reload: vi.fn() }) }))
vi.mock("../client", () => ({ call: vi.fn(), retryNow: vi.fn() }))
import { DiffScreen } from "./DiffScreen"

const diff = (text: string): GitDiff => ({ text, truncated: false, binary: false })
const BARE_COMBINED = "diff --cc f.txt\nindex 85cb8e3,8948000..0000000\n--- a/f.txt\n+++ b/f.txt\n"
const UNMERGED_PATH = "* Unmerged path f.txt\n"
const MATCHED = /Unresolved conflict: this file is still unmerged/
const UNMERGED = /Unresolved conflict: one side deleted or added this file/
const conflicted: DiffSource = { kind: "worktree", path: "f.txt", untracked: false, conflicted: true }
const plain: DiffSource = { kind: "worktree", path: "f.txt", untracked: false }

function renderWith(source: DiffSource, text: string, stillConflicted: boolean, error: string | null) {
  hook.outcome = { data: { diff: diff(text), stillConflicted }, error }
  render(<I18nextProvider i18n={i18n}><DiffScreen session="work" workspaceId="w1" source={source} /></I18nextProvider>)
}

beforeEach(async () => {
  await i18n.changeLanguage("en")
})
afterEach(cleanup)

it.each([
  ["a bare combined header", BARE_COMBINED, MATCHED],
  ["an unmerged path line", UNMERGED_PATH, UNMERGED],
])("shows a Conflicted row's %s notice with no error", async (_name, text, message) => {
  renderWith(conflicted, text, false, null)
  expect(screen.getByText(message)).toBeInTheDocument()
  expect(screen.queryByText(/Git could not read this repository/)).toBeNull()
})

it.each([
  ["a bare combined header", BARE_COMBINED],
  ["an unmerged path line", UNMERGED_PATH],
  ["blank text of a file still listed", ""],
  ["blank text of a file no longer listed", ""],
])("shows only the load error for a Conflicted row's %s", async (name, text) => {
  renderWith(conflicted, text, name.includes("still listed"), "git-failed")
  expect(screen.getByText(/Git could not read this repository/)).toBeInTheDocument()
  for (const message of [MATCHED, UNMERGED, /no longer in conflict/, "No differences to show.", "No content changes."]) expect(screen.queryByText(message)).toBeNull()
})

it("leaves the notice of a file that is not a Conflicted row alone", async () => {
  renderWith(plain, UNMERGED_PATH, false, "git-failed")
  expect(screen.getByText(/Git could not read this repository/)).toBeInTheDocument()
  expect(screen.getByText(UNMERGED)).toBeInTheDocument()
})
