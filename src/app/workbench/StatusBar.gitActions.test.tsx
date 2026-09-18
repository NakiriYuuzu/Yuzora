import { beforeEach, expect, it, vi } from "vitest"
import { fireEvent, render, screen, waitFor } from "@testing-library/react"
vi.mock("@/lib/ipc", async original => ({
    ...await original<typeof import("@/lib/ipc")>(),
    gitFetch: vi.fn(), gitCheckout: vi.fn(), gitCreateBranch: vi.fn(), gitStatus: vi.fn(), gitBranches: vi.fn(), gitBootstrap: vi.fn(),
}))
import { gitFetch, gitCheckout, gitCreateBranch, gitStatus, gitBranches, gitBootstrap } from "@/lib/ipc"
import { clearGitSnapshots, initialGitState, useGitStore } from "@/state/gitStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { StatusBar } from "./StatusBar"

const status = { branch: "main", headOid: "a".repeat(40), detached: false, upstream: null, ahead: 0, behind: 0, staged: [], unstaged: [], untracked: [], conflicted: [], inProgress: null }
const branches = { local: [
    { name: "main", isCurrent: true, upstream: null, ahead: 0, behind: 0, gone: false },
    { name: "feature", isCurrent: false, upstream: null, ahead: 0, behind: 0, gone: false },
], remote: [], tags: [] }

beforeEach(() => {
    vi.resetAllMocks()
    clearGitSnapshots()
    useWorkspaceStore.setState({ workspacePath: "/w", activeGroupIndex: 0, groups: [{ activePath: null, tabs: [] }] })
    useGitStore.setState({ ...initialGitState, environment: { status: "ready", root: "/w", version: "2.50" }, status, branches })
    vi.mocked(gitStatus).mockResolvedValue(status)
    vi.mocked(gitBranches).mockResolvedValue(branches)
    vi.mocked(gitBootstrap).mockResolvedValue({ environment: { status: "ready", root: "/w", version: "2.50" }, status, branches })
})

it.each(["fetch", "checkout", "create"])("StatusBar %s performs one mutation while retrying only a busy snapshot", async action => {
    vi.mocked(gitBranches).mockRejectedValueOnce("git-jobs-busy")
    render(<StatusBar />)
    fireEvent.click(screen.getByRole("button", { name: "main" }))
    if (action === "fetch") fireEvent.click(await screen.findByRole("button", { name: "Fetch" }))
    if (action === "checkout") fireEvent.click(await screen.findByRole("option", { name: "feature" }))
    if (action === "create") {
        fireEvent.click(await screen.findByRole("button", { name: "New branch…" }))
        const input = screen.getByPlaceholderText("Branch name…")
        fireEvent.change(input, { target: { value: "new-branch" } })
        fireEvent.keyDown(input, { key: "Enter" })
    }
    const mutation = action === "fetch" ? gitFetch : action === "checkout" ? gitCheckout : gitCreateBranch
    await waitFor(() => expect(mutation).toHaveBeenCalledOnce())
    await waitFor(() => expect(useGitStore.getState().busy).toBeNull())
    expect(gitBranches).toHaveBeenCalledTimes(2)
    expect(useGitStore.getState().lastError).toBeNull()
})

it("keeps a failed refresh distinct from a successful fetch, and Retry does not fetch again", async () => {
    await useGitStore.getState().detect("/w")
    vi.mocked(gitBranches).mockRejectedValue(new Error("read failed"))
    render(<StatusBar />)
    fireEvent.click(screen.getByRole("button", { name: "main" }))
    fireEvent.click(await screen.findByRole("button", { name: "Fetch" }))
    expect(await screen.findByRole("alert")).toHaveTextContent("Git state could not be refreshed")
    expect(gitFetch).toHaveBeenCalledOnce()
    await waitFor(() => expect(useGitStore.getState().busy).toBeNull())
    fireEvent.click(screen.getByRole("button", { name: "Reload Git status" }))
    await waitFor(() => expect(useGitStore.getState().snapshotStale).toBe(false))
    expect(gitFetch).toHaveBeenCalledOnce()
})
