import { cleanup, render, screen, waitFor } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"
import type { HerdrWorktreeListResult } from "@/lib/herdrTypes"
import { herdrWorktreeList } from "@/lib/herdrIpc"
import { WorktreeTools } from "./WorktreeTools"
import type { HerdrOperation } from "./useHerdrOperation"

vi.mock("@/lib/herdrIpc", () => ({ herdrWorktreeList: vi.fn() }))

afterEach(() => { cleanup(); vi.mocked(herdrWorktreeList).mockReset() })

const spaces = [{ id: "A", label: "Alpha" }, { id: "B", label: "Beta" }]
const operation = { busy: false, run: vi.fn() } as unknown as HerdrOperation
const list = (path: string) => ({ source: {}, worktrees: [{ path, label: path, branch: "br-" + path, isLinkedWorktree: false, openWorkspaceId: null }] }) as unknown as HerdrWorktreeListResult
const tools = (workspaceId: string) => <WorktreeTools sessionName="s" workspaceId={workspaceId} spaces={spaces} onWorkspaceChange={() => {}} operation={operation} can={() => true} />

describe("WorktreeTools Space changes", () => {
  it("drops the previous Space's error as soon as the Space changes", async () => {
    vi.mocked(herdrWorktreeList).mockRejectedValueOnce("list-failed-A").mockReturnValueOnce(new Promise(() => {}))
    const view = render(tools("A"))
    expect(await screen.findByRole("alert")).toHaveTextContent("list-failed-A")
    view.rerender(tools("B"))
    expect(screen.queryByRole("alert")).toBeNull()
  })

  it("does not show Space A's inventory again on A to B to A before the refetch resolves", async () => {
    vi.mocked(herdrWorktreeList).mockResolvedValueOnce(list("/repo/a")).mockReturnValueOnce(new Promise(() => {})).mockReturnValueOnce(new Promise(() => {}))
    const view = render(tools("A"))
    await waitFor(() => expect(screen.getByText("/repo/a")).toBeInTheDocument())
    view.rerender(tools("B"))
    expect(screen.queryByText("/repo/a")).toBeNull()
    view.rerender(tools("A"))
    expect(screen.queryByText("/repo/a")).toBeNull()
    expect(herdrWorktreeList).toHaveBeenCalledTimes(3)
  })
})
