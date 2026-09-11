import { afterAll, beforeAll, expect, it, vi } from "vitest"
import { fireEvent, screen } from "@testing-library/react"
import { clearMocks } from "@tauri-apps/api/mocks"
import { invoke } from "@tauri-apps/api/core"
import { ROOT } from "./runtime"
import type { GitStatus, LogPage } from "@/lib/types"

vi.mock("./Demo", () => ({ mountDemo: async () => {} }))
beforeAll(async () => { await import("./long-session-e2e-git") })
afterAll(() => { clearMocks(); document.body.replaceChildren() })

it("provides history, simulated writes and existing file reads through memory-only transport", async () => {
    const page = await invoke<LogPage>("git_log_page", { repositoryRoot: ROOT })
    expect(page.commits).toHaveLength(34)
    expect(page.commits[0].parents).toHaveLength(32)
    const before = await invoke<GitStatus>("git_status_cmd", { repositoryRoot: ROOT })
    await invoke("git_commit_cmd", { repositoryRoot: ROOT, message: "amend memory", amendHead: before.headOid })
    const after = await invoke<GitStatus>("git_status_cmd", { repositoryRoot: ROOT })
    expect(after.headOid).not.toBe(before.headOid)
    expect(await invoke("git_commit_detail", { repositoryRoot: ROOT, hash: after.headOid })).toMatchObject({ subject: "amend memory", body: "" })
    expect(after.unstaged).toEqual(before.unstaged)
    expect(after.untracked).toEqual(before.untracked)
    expect(await invoke("open_file", { path: `${ROOT}/src/App.tsx` })).toMatchObject({ kind: "full" })
    expect(screen.getByTestId("memory-git-commands")).toHaveTextContent("git_commit_cmd")
    await expect(invoke("git_unknown_write", {})).rejects.toThrow("MEMORY: unsupported")
})

it("injects one-shot read and operation errors and resets its in-memory repository", async () => {
    fireEvent.click(screen.getByRole("button", { name: "Fail branches once" }))
    await expect(invoke("git_branches", { repositoryRoot: ROOT })).rejects.toThrow("one-shot branch")
    await expect(invoke("git_branches", { repositoryRoot: ROOT })).resolves.toHaveProperty("local")
    fireEvent.click(screen.getByRole("button", { name: "Fail fetch once" }))
    await expect(invoke("git_fetch_cmd", { repositoryRoot: ROOT })).rejects.toThrow("authentication failed")
    await expect(invoke("git_fetch_cmd", { repositoryRoot: ROOT })).resolves.toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "Reset Git memory" }))
    expect((await invoke<GitStatus>("git_status_cmd", { repositoryRoot: ROOT })).headOid).toBe((100).toString(16).padStart(40, "0"))
})

it("arms branch failure only after fetch and provides complete long diff content", async () => {
    fireEvent.click(screen.getByRole("button", { name: "Fail branches after Fetch" }))
    await expect(invoke("git_branches", { repositoryRoot: ROOT })).resolves.toHaveProperty("local")
    await invoke("git_fetch_cmd", { repositoryRoot: ROOT })
    await expect(invoke("git_branches", { repositoryRoot: ROOT })).rejects.toThrow("one-shot branch")
    fireEvent.click(screen.getByRole("button", { name: "Use long diff" }))
    const diff = await invoke<{ original: { content: string }; modified: { content: string } }>("git_diff_content", { repositoryRoot: ROOT, path: "src/App.tsx" })
    expect(diff.original.content).toContain("ORIGINAL END MARKER")
    expect(diff.modified.content).toContain("MODIFIED END MARKER")
    expect(diff.modified.content.split("\n")).toHaveLength(602)
    expect(diff.modified.content.split("\n")[0].length).toBeGreaterThan(500)
})

it("recovers through an automatic watcher while the post-fetch status is pending", async () => {
    const { useGitStore, gitMutationsBlocked } = await import("@/state/gitStore")
    await useGitStore.getState().detect(ROOT)
    fireEvent.click(screen.getByRole("button", { name: "Fetch failure + auto watcher recovery" }))
    const result = await useGitStore.getState().runOp("fetch", () => invoke("git_fetch_cmd", { repositoryRoot: ROOT }))
    expect(result).toBe(true)
    expect(gitMutationsBlocked()).toBe(false)
    expect(useGitStore.getState().lastError).toBeNull()
    expect(screen.getByTestId("memory-git-commands")).toHaveTextContent('"busy":"fetch","stale":false')
})
