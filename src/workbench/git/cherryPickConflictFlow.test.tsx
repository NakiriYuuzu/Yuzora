import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import type { AuthorEntry, CommitDetail, GitStatus, LogCommit, LogPage } from "@/lib/types"

const mocks = vi.hoisted(() => ({
    confirm: vi.fn(async () => true),
    invoke: vi.fn<(cmd: string, args?: unknown) => Promise<unknown>>(),
    listeners: new Map<string, (event: unknown) => void>(),
    writeText: vi.fn(async () => undefined)
}))

function makeStatus(overrides: Partial<GitStatus> = {}): GitStatus {
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
        ...overrides
    }
}

function makeCommit(i: number, overrides: Partial<LogCommit> = {}): LogCommit {
    const hash = `hash${i}`.padEnd(40, "0")
    return {
        hash,
        shortHash: `hash${i}`.slice(0, 7),
        subject: `commit subject ${i}`,
        authorName: "Kenji",
        authorEmail: "kenji@yuuzu.dev",
        timestamp: 1_770_000_000 - i * 3600,
        parents: [`hash${i + 1}`.padEnd(40, "0")],
        refs: [],
        ...overrides
    }
}

function makeDetail(overrides: Partial<CommitDetail> = {}): CommitDetail {
    return {
        subject: "commit subject 0",
        body: "",
        authorName: "Kenji",
        authorEmail: "kenji@yuuzu.dev",
        timestamp: 1_770_000_000,
        parents: ["hash1".padEnd(40, "0")],
        files: [
            { status: "M", path: "git.rs", oldPath: null, additions: 9, deletions: 3, binary: false }
        ],
        totalAdditions: 9,
        totalDeletions: 3,
        ...overrides
    }
}

function invokeCalls(cmd: string) {
    return mocks.invoke.mock.calls.filter(([seen]) => seen === cmd)
}

vi.mock("@tauri-apps/api/core", () => ({
    Channel: class MockChannel<T = unknown> {
        onmessage: ((message: T) => void) | null = null
    },
    invoke: (cmd: string, args?: unknown) => mocks.invoke(cmd, args)
}))

vi.mock("@tauri-apps/api/event", () => ({
    listen: vi.fn(async (event: string, cb: unknown) => {
        mocks.listeners.set(event, cb as (event: unknown) => void)
        return () => mocks.listeners.delete(event)
    })
}))

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
    writeText: () => mocks.writeText()
}))

vi.mock("@tauri-apps/plugin-dialog", () => ({
    confirm: () => mocks.confirm()
}))

const { LogTab } = await import("./LogTab")
const { ConflictBanner } = await import("./ConflictBanner")
const { GitBridge } = await import("../GitBridge")
const { useGitLogStore } = await import("@/state/gitLogStore")
const { useGitStore, initialGitState } = await import("@/state/gitStore")
const { useWorkspaceStore } = await import("@/state/workspaceStore")

describe("cherry-pick conflict flow", () => {
    beforeEach(() => {
        mocks.confirm.mockReset().mockResolvedValue(true)
        mocks.invoke.mockReset()
        mocks.listeners.clear()
        mocks.writeText.mockReset().mockResolvedValue(undefined)

        useGitLogStore.getState().reset()
        useGitStore.setState({
            ...initialGitState,
            environment: { status: "ready", root: "/w", version: "2.50.1" },
            status: makeStatus(),
            remoteCheck: { mode: "off", intervalSec: 180 }
        })
        useWorkspaceStore.setState({
            workspacePath: null,
            groups: [{ tabs: [], activePath: null }],
            activeGroupIndex: 0
        })

        mocks.invoke.mockImplementation(async (cmd) => {
            switch (cmd) {
                case "git_log_page":
                    return { commits: [makeCommit(0)], hasMore: false } satisfies LogPage
                case "git_log_authors":
                    return [] satisfies AuthorEntry[]
                case "git_commit_detail":
                    return makeDetail()
                case "git_cherry_pick":
                    throw new Error("error: could not apply hash0")
                case "git_status_cmd":
                    return makeStatus({
                        inProgress: "cherry-pick",
                        conflicted: [{ path: "src/conflicted.ts", origPath: null, status: "UU" }]
                    })
                case "git_branches":
                    return { local: [], remote: [] }
                case "git_conflict_abort":
                case "log_event":
                    return undefined
                default:
                    throw new Error(`unexpected invoke: ${cmd}`)
            }
        })
    })

    afterEach(() => {
        cleanup()
        vi.clearAllMocks()
    })

    it("shows ConflictBanner after a rejected cherry-pick refreshes status", async () => {
        render(
            <>
                <GitBridge />
                <LogTab />
                <ConflictBanner />
            </>
        )
        await waitFor(() => expect(mocks.listeners.has("git:state-changed")).toBe(true))

        fireEvent.click(await screen.findByText("commit subject 0"))
        await screen.findByText("git.rs")

        fireEvent.click(screen.getByRole("button", { name: "Cherry-pick" }))

        await waitFor(() =>
            expect(mocks.invoke).toHaveBeenCalledWith("git_cherry_pick", {
                hash: makeCommit(0).hash
            })
        )
        await waitFor(() =>
            expect(useGitStore.getState().consoleLog[0]?.cmd).toBe("git cherry-pick")
        )

        const onGitStateChanged = mocks.listeners.get("git:state-changed")
        expect(onGitStateChanged).toBeDefined()
        act(() => {
            onGitStateChanged?.({})
        })
        await waitFor(() => expect(invokeCalls("git_status_cmd").length).toBeGreaterThan(0))

        expect(await screen.findByText("cherry-pick 進行中")).toBeInTheDocument()
        expect(screen.getByText("src/conflicted.ts")).toBeInTheDocument()

        fireEvent.click(screen.getByRole("button", { name: "Abort" }))

        await waitFor(() =>
            expect(mocks.invoke).toHaveBeenCalledWith("git_conflict_abort", {
                op: "cherry-pick"
            })
        )
        await waitFor(() =>
            expect(useGitStore.getState().consoleLog[0]?.cmd).toBe("git cherry-pick --abort")
        )
        await waitFor(() => expect(useGitStore.getState().busy).toBeNull())
    })
})
