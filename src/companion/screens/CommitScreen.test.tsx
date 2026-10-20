import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { I18nextProvider } from "react-i18next"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { i18n } from "../i18n"
import type { GitCommit } from "../lib/protocol"
import { RpcError } from "../lib/rpc"
import { useCompanion } from "../store"

const client = vi.hoisted(() => ({ call: vi.fn() }))
vi.mock("../client", () => ({ call: client.call, retryNow: vi.fn() }))
import { CommitScreen } from "./CommitScreen"

const hash = "0123456789abcdef0123456789abcdef01234567"
const detail: GitCommit = {
  hash,
  subject: "Fix the sidebar",
  body: "It jumped on resize.\n\nCloses #12",
  authorName: "Sora Ito",
  authorEmail: "sora@example.com",
  timestamp: 1_760_000_000,
  parents: ["aaaaaaa1111111111111111111111111111111111", "bbbbbbb2222222222222222222222222222222222"],
  files: [
    { status: "M", path: "src/side.ts", oldPath: null, additions: 5, deletions: 2, binary: false },
    { status: "R", path: "src/new.ts", oldPath: "src/old.ts", additions: 0, deletions: 0, binary: false },
    { status: "A", path: "logo.png", oldPath: null, additions: 0, deletions: 0, binary: true },
  ],
  totalAdditions: 5,
  totalDeletions: 2,
  truncated: false,
}

function renderCommit() {
  render(<I18nextProvider i18n={i18n}><CommitScreen session="work" workspaceId="w1" hash={hash} /></I18nextProvider>)
}

beforeEach(async () => {
  await i18n.changeLanguage("en")
  client.call.mockReset()
  useCompanion.setState({ socket: "ready", epoch: 1, git: {} })
})
afterEach(cleanup)

it("shows the message, author, parents and changed files with their counts", async () => {
  client.call.mockResolvedValue(detail)
  renderCommit()
  expect(await screen.findByRole("heading", { name: "Fix the sidebar" })).toBeInTheDocument()
  expect(client.call).toHaveBeenCalledWith("git.commit", { session: "work", workspaceId: "w1", hash })
  expect(screen.getByText(/It jumped on resize\./)).toBeInTheDocument()
  expect(screen.getByText(/Sora Ito/)).toBeInTheDocument()
  expect(screen.getByText("Changed files: 3")).toBeInTheDocument()
  expect(screen.getByRole("button", { name: /^side\.ts/ }).textContent).toContain("+5")
  expect(screen.getByRole("button", { name: /^logo\.png/ }).textContent).toContain("binary")
  expect(screen.getByRole("button", { name: /^new\.ts.*← src\/old\.ts/ })).toBeInTheDocument()
  // Nothing here can change the repository.
  expect(screen.queryAllByRole("button").map((button) => button.textContent)).toEqual(expect.not.arrayContaining([expect.stringMatching(/revert|reset|cherry|checkout/i)]))
})

it("opens a file's diff of that commit, and a parent commit", async () => {
  client.call.mockResolvedValue(detail)
  renderCommit()
  fireEvent.click(await screen.findByRole("button", { name: /^side\.ts/ }))
  expect(useCompanion.getState().screen).toEqual({ name: "diff", session: "work", workspaceId: "w1", source: { kind: "commit", hash, path: "src/side.ts", origPath: null } })
  fireEvent.click(screen.getByRole("button", { name: "bbbbbbb" }))
  expect(useCompanion.getState().screen).toEqual({ name: "commit", session: "work", workspaceId: "w1", hash: detail.parents[1] })
})

it("opens a renamed file's diff with its old path, so git can pair the rename", async () => {
  client.call.mockResolvedValue(detail)
  renderCommit()
  fireEvent.click(await screen.findByRole("button", { name: /^new\.ts/ }))
  expect(useCompanion.getState().screen).toEqual({ name: "diff", session: "work", workspaceId: "w1", source: { kind: "commit", hash, path: "src/new.ts", origPath: "src/old.ts" } })
})

it("does not read a commit twice", async () => {
  client.call.mockResolvedValue(detail)
  renderCommit()
  await screen.findByRole("heading", { name: "Fix the sidebar" })
  cleanup()
  renderCommit()
  expect(await screen.findByRole("heading", { name: "Fix the sidebar" })).toBeInTheDocument()
  expect(client.call).toHaveBeenCalledTimes(1)
})

it("explains a commit that could not be read", async () => {
  client.call.mockRejectedValue(new RpcError("git-untrusted"))
  renderCommit()
  expect(await screen.findByText("Trust this workspace on the desktop first.")).toBeInTheDocument()
})
