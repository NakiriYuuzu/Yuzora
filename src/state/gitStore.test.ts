import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { GitStatus } from "../lib/types"

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

vi.mock("../lib/ipc", () => ({
    gitDetect: vi.fn(async () => ({ status: "ready", root: "/w", version: "2.50.1" })),
    gitStatus: vi.fn(async () => makeStatus()),
    gitBranches: vi.fn(async () => ({ local: [], remote: [] })),
    gitRemoteProbe: vi.fn(async () => "yes"),
    gitFetch: vi.fn(async () => undefined)
}))

// The Bun-hosted test runtime injects an empty `localStorage` global with no
// Storage methods, which also shadows jsdom's implementation. Install a minimal
// in-memory Storage so setRemoteCheck persistence is exercised for real. The
// proper home for this is src/test/setup.ts (outside this task's file scope);
// see the task report for the lead hand-off note.
function installLocalStorage(): void {
    const store = new Map<string, string>()
    const mock = {
        getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
        setItem: (k: string, v: string) => void store.set(k, String(v)),
        removeItem: (k: string) => void store.delete(k),
        clear: () => store.clear(),
        key: (i: number) => [...store.keys()][i] ?? null,
        get length() {
            return store.size
        }
    }
    Object.defineProperty(globalThis, "localStorage", {
        value: mock,
        configurable: true,
        writable: true
    })
}

describe("gitStore", () => {
    beforeEach(() => {
        vi.useFakeTimers()
        installLocalStorage()
    })
    afterEach(async () => {
        vi.useRealTimers()
        vi.clearAllMocks()
        localStorage.clear()
        const { useGitStore, initialGitState } = await import("./gitStore")
        useGitStore.setState(initialGitState)
    })

    it("refresh debounces multiple calls into one gitStatus", async () => {
        const { useGitStore } = await import("./gitStore")
        const ipc = await import("../lib/ipc")
        // refresh gates on a ready environment (m2); set it up as the precondition.
        useGitStore.setState({ environment: { status: "ready", root: "/w", version: "2.50.1" } })
        void useGitStore.getState().refresh()
        void useGitStore.getState().refresh()
        void useGitStore.getState().refresh()
        await vi.advanceTimersByTimeAsync(400)
        expect(ipc.gitStatus).toHaveBeenCalledTimes(1)
    })

    it("refresh no-ops when the environment is not ready (m2)", async () => {
        const { useGitStore } = await import("./gitStore")
        const ipc = await import("../lib/ipc")
        // environment stays null (initial) → not ready. refresh must not call
        // gitStatus and must not write lastError (background noise rule).
        await useGitStore.getState().refresh()
        await vi.advanceTimersByTimeAsync(400)
        expect(ipc.gitStatus).not.toHaveBeenCalled()
        expect(useGitStore.getState().lastError).toBe(null)
    })

    it("refresh reruns once when called during an in-flight fetch (m3)", async () => {
        const { useGitStore } = await import("./gitStore")
        const ipc = await import("../lib/ipc")
        useGitStore.setState({ environment: { status: "ready", root: "/w", version: "2.50.1" } })
        // Make the first status fetch hang so a second refresh lands while it is
        // actually in flight (past the debounce window).
        let release = () => {}
        ;(ipc.gitStatus as ReturnType<typeof vi.fn>).mockImplementationOnce(
            () => new Promise((res) => { release = () => res(makeStatus()) })
        )
        void useGitStore.getState().refresh()
        await vi.advanceTimersByTimeAsync(300) // debounce fires → gitStatus called, now hanging
        expect(ipc.gitStatus).toHaveBeenCalledTimes(1)
        // Second call arrives during the in-flight fetch → should schedule a rerun.
        void useGitStore.getState().refresh()
        release()
        await Promise.resolve()
        await Promise.resolve()
        await vi.advanceTimersByTimeAsync(300) // rerun debounce fires → second fetch
        expect(ipc.gitStatus).toHaveBeenCalledTimes(2)
    })

    it("abandons the debounced fetch if environment flips to non-ready during the window (m2/F2)", async () => {
        const { useGitStore } = await import("./gitStore")
        const ipc = await import("../lib/ipc")
        useGitStore.setState({ environment: { status: "ready", root: "/w", version: "2.50.1" } })
        // Scheduled while ready…
        void useGitStore.getState().refresh()
        // …then environment flips to non-ready before the 300ms debounce fires.
        useGitStore.setState({ environment: { status: "notARepo" } })
        await vi.advanceTimersByTimeAsync(400)
        // Re-checked at execution time: no gitStatus, no lastError noise.
        expect(ipc.gitStatus).not.toHaveBeenCalled()
        expect(useGitStore.getState().lastError).toBe(null)
    })

    it("abandons the in-flight rerun if environment flips to non-ready (m3/F2)", async () => {
        const { useGitStore } = await import("./gitStore")
        const ipc = await import("../lib/ipc")
        useGitStore.setState({ environment: { status: "ready", root: "/w", version: "2.50.1" } })
        let release = () => {}
        ;(ipc.gitStatus as ReturnType<typeof vi.fn>).mockImplementationOnce(
            () => new Promise((res) => { release = () => res(makeStatus()) })
        )
        void useGitStore.getState().refresh()
        await vi.advanceTimersByTimeAsync(300) // debounce fires → gitStatus #1, hanging
        expect(ipc.gitStatus).toHaveBeenCalledTimes(1)
        void useGitStore.getState().refresh() // during in-flight → schedules a rerun
        release()
        await Promise.resolve()
        await Promise.resolve() // finally runs → rerun scheduled (still ready)
        // Flip to non-ready before the rerun's own debounce fires.
        useGitStore.setState({ environment: { status: "missing", reason: "gone" } })
        await vi.advanceTimersByTimeAsync(300) // rerun callback fires → re-check aborts it
        expect(ipc.gitStatus).toHaveBeenCalledTimes(1)
        expect(useGitStore.getState().lastError).toBe(null)
    })

    it("discards a resolved status if environment flipped to non-ready mid-flight (F-1)", async () => {
        const { useGitStore } = await import("./gitStore")
        const ipc = await import("../lib/ipc")
        useGitStore.setState({
            environment: { status: "ready", root: "/w", version: "2.50.1" },
            status: makeStatus()
        })
        let release = () => {}
        ;(ipc.gitStatus as ReturnType<typeof vi.fn>).mockImplementationOnce(
            () => new Promise((res) => { release = () => res(makeStatus()) })
        )
        void useGitStore.getState().refresh()
        await vi.advanceTimersByTimeAsync(300) // gitStatus called, hanging
        // Simulate detect() switching to a non-repo workspace: environment flips
        // non-ready and status is cleared to null.
        useGitStore.setState({ environment: { status: "notARepo" }, status: null })
        release()
        await Promise.resolve()
        await Promise.resolve()
        // The stale resolve must NOT re-fill the just-cleared status.
        expect(useGitStore.getState().status).toBe(null)
        expect(useGitStore.getState().lastError).toBe(null)
    })

    it("discards a rejected fetch (no lastError) if environment flipped mid-flight (F-1)", async () => {
        const { useGitStore } = await import("./gitStore")
        const ipc = await import("../lib/ipc")
        useGitStore.setState({ environment: { status: "ready", root: "/w", version: "2.50.1" } })
        let reject = () => {}
        ;(ipc.gitStatus as ReturnType<typeof vi.fn>).mockImplementationOnce(
            () => new Promise((_res, rej) => { reject = () => rej(new Error("stale boom")) })
        )
        void useGitStore.getState().refresh()
        await vi.advanceTimersByTimeAsync(300)
        useGitStore.setState({ environment: { status: "notARepo" }, status: null })
        reject()
        await Promise.resolve()
        await Promise.resolve()
        // A stale rejection from the old workspace must stay silent (no noise).
        expect(useGitStore.getState().lastError).toBe(null)
    })

    it("runOp rejects concurrent ops and clears busy after failure", async () => {
        const { useGitStore } = await import("./gitStore")
        const slow = useGitStore.getState().runOp("push", () => new Promise((r) => setTimeout(r, 1000)))
        expect(await useGitStore.getState().runOp("pull", async () => {})).toBe(false)
        await vi.advanceTimersByTimeAsync(1500)
        expect(await slow).toBe(true)
        expect(useGitStore.getState().busy).toBe(null)
        expect(await useGitStore.getState().runOp("fail", async () => { throw new Error("boom") })).toBe(false)
        expect(useGitStore.getState().lastError).toContain("boom")
        expect(useGitStore.getState().busy).toBe(null)
    })

    it("checkRemote probe mode sets remoteIncoming; failure pauses silently", async () => {
        const { useGitStore } = await import("./gitStore")
        const ipc = await import("../lib/ipc")
        useGitStore.getState().setRemoteCheck({ mode: "probe", intervalSec: 180 })
        await useGitStore.getState().checkRemote()
        expect(useGitStore.getState().remoteIncoming).toBe("yes")
        ;(ipc.gitRemoteProbe as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error("auth"))
        await useGitStore.getState().checkRemote()
        expect(useGitStore.getState().remotePaused).toBe(true)
        expect(useGitStore.getState().lastError).toBe(null)
    })

    it("checkRemote autofetch pauses silently when background gitStatus fails", async () => {
        const { useGitStore } = await import("./gitStore")
        const ipc = await import("../lib/ipc")
        useGitStore.getState().setRemoteCheck({ mode: "autofetch", intervalSec: 60 })
        ;(ipc.gitStatus as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error("status boom"))
        await useGitStore.getState().checkRemote()
        expect(ipc.gitFetch).toHaveBeenCalledWith(true)
        expect(useGitStore.getState().remotePaused).toBe(true)
        expect(useGitStore.getState().lastError).toBe(null)
    })

    it("detect to notARepo clears stale repo state", async () => {
        const { useGitStore } = await import("./gitStore")
        const ipc = await import("../lib/ipc")
        // 先塞入舊 repo 殘留（模擬前一個 workspace）。
        useGitStore.setState({
            status: makeStatus(),
            branches: { local: [{ name: "main", upstream: null, ahead: 0, behind: 0, isCurrent: true }], remote: [] },
            remoteIncoming: "yes",
            remotePaused: true
        })
        ;(ipc.gitDetect as ReturnType<typeof vi.fn>).mockResolvedValueOnce({ status: "notARepo" })
        await useGitStore.getState().detect("/other")
        expect(useGitStore.getState().status).toBe(null)
        expect(useGitStore.getState().branches).toBe(null)
        expect(useGitStore.getState().remoteIncoming).toBe("unknown")
        expect(useGitStore.getState().remotePaused).toBe(false)
    })

    it("checkRemote is a no-op while busy (no probe/fetch)", async () => {
        const { useGitStore } = await import("./gitStore")
        const ipc = await import("../lib/ipc")
        useGitStore.getState().setRemoteCheck({ mode: "probe", intervalSec: 180 })
        useGitStore.setState({ busy: "push" })
        await useGitStore.getState().checkRemote()
        expect(ipc.gitRemoteProbe).not.toHaveBeenCalled()
        expect(ipc.gitFetch).not.toHaveBeenCalled()
    })

    it("checkRemote is a no-op while remotePaused (no probe/fetch)", async () => {
        const { useGitStore } = await import("./gitStore")
        const ipc = await import("../lib/ipc")
        useGitStore.getState().setRemoteCheck({ mode: "autofetch", intervalSec: 60 })
        useGitStore.setState({ remotePaused: true })
        await useGitStore.getState().checkRemote()
        expect(ipc.gitRemoteProbe).not.toHaveBeenCalled()
        expect(ipc.gitFetch).not.toHaveBeenCalled()
    })

    it("setRemoteCheck persists to localStorage", async () => {
        const { useGitStore, REMOTE_CHECK_STORAGE_KEY } = await import("./gitStore")
        useGitStore.getState().setRemoteCheck({ mode: "autofetch", intervalSec: 60 })
        expect(JSON.parse(localStorage.getItem(REMOTE_CHECK_STORAGE_KEY)!)).toEqual({ mode: "autofetch", intervalSec: 60 })
    })

    it("appendConsole prepends newest-first", async () => {
        const { useGitStore } = await import("./gitStore")
        const mk = (id: number, cmd: string) => ({ id, cmd, out: [], tone: "ok" as const, time: "12:00" })
        useGitStore.getState().appendConsole(mk(1, "git fetch"))
        useGitStore.getState().appendConsole(mk(2, "git pull --rebase"))
        const log = useGitStore.getState().consoleLog
        expect(log.map((e) => e.cmd)).toEqual(["git pull --rebase", "git fetch"])
    })

    it("appendConsole caps the ring buffer at CONSOLE_LOG_LIMIT, dropping the tail", async () => {
        const { useGitStore, CONSOLE_LOG_LIMIT } = await import("./gitStore")
        for (let i = 0; i < CONSOLE_LOG_LIMIT + 5; i++) {
            useGitStore.getState().appendConsole({ id: i, cmd: `op-${i}`, out: [], tone: "ok", time: "12:00" })
        }
        const log = useGitStore.getState().consoleLog
        expect(log).toHaveLength(CONSOLE_LOG_LIMIT)
        // Newest (last appended) is at the head; the oldest 5 were dropped.
        expect(log[0].cmd).toBe(`op-${CONSOLE_LOG_LIMIT + 4}`)
        expect(log[log.length - 1].cmd).toBe("op-5")
    })

    it("runOp records an ok console entry on success (mapped cmd label)", async () => {
        const { useGitStore } = await import("./gitStore")
        // runOp awaits its internal debounced refresh(), so drive the fake
        // timers forward while the op is in flight (same pattern as the
        // concurrent-op test above).
        const opDone = useGitStore.getState().runOp("pull", async () => {})
        await vi.advanceTimersByTimeAsync(400)
        expect(await opDone).toBe(true)
        const log = useGitStore.getState().consoleLog
        expect(log).toHaveLength(1)
        expect(log[0].cmd).toBe("git pull --rebase")
        expect(log[0].tone).toBe("ok")
        expect(log[0].out).toEqual(["Done"])
    })

    it("setCommitMessage stores the draft; initial state is empty", async () => {
        const { useGitStore, initialGitState } = await import("./gitStore")
        expect(initialGitState.commitMessage).toBe("")
        useGitStore.getState().setCommitMessage("feat: x")
        expect(useGitStore.getState().commitMessage).toBe("feat: x")
    })

    it("runOp records an err console entry with the error message on failure", async () => {
        const { useGitStore } = await import("./gitStore")
        const ok = await useGitStore.getState().runOp("push", async () => {
            throw new Error("remote rejected")
        })
        expect(ok).toBe(false)
        const log = useGitStore.getState().consoleLog
        expect(log).toHaveLength(1)
        expect(log[0].cmd).toBe("git push")
        expect(log[0].tone).toBe("err")
        expect(log[0].out[0]).toContain("remote rejected")
    })
})
