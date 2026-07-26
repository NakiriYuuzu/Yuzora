import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { GitBootstrapResult, GitStatus } from "../lib/types"

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

function makeBootstrap(root = "/w"): GitBootstrapResult {
    return {
        environment: { status: "ready", root, version: "2.50.1" },
        status: makeStatus(),
        branches: { local: [], remote: [] }
    }
}

vi.mock("../lib/ipc", () => ({
    gitBootstrap: vi.fn(async () => ({
        environment: { status: "ready", root: "/w", version: "2.50.1" },
        status: {
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
        },
        branches: { local: [], remote: [] }
    })),
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
        const { useGitStore, initialGitState, clearGitSnapshots } = await import("./gitStore")
        useGitStore.setState(initialGitState)
        // #58 T4a：per-root 快照與 live key 是模組級狀態，逐測清空避免串場。
        clearGitSnapshots()
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

    // #57 T3 AC1/AC2：detect 一趟 bootstrap 回齊 environment＋status＋branches，
    // 不再走 gitStatus/gitBranches 的 waterfall，首載也不吃 300ms debounce——
    // 全程不撥 fake timers，resolve 即填滿。
    it("detect fills environment+status+branches in one bootstrap trip with no debounce (#57 T3)", async () => {
        const { useGitStore } = await import("./gitStore")
        const ipc = await import("../lib/ipc")
        await useGitStore.getState().detect("/w")
        expect(useGitStore.getState().environment).toEqual({
            status: "ready",
            root: "/w",
            version: "2.50.1"
        })
        expect(useGitStore.getState().status).not.toBe(null)
        expect(useGitStore.getState().branches).toEqual({ local: [], remote: [] })
        // 單趟完成：細粒度 command 留給後續 refresh，首載一律不碰。
        expect(ipc.gitStatus).not.toHaveBeenCalled()
        expect(ipc.gitBranches).not.toHaveBeenCalled()
    })

    // #57 覆核修正：Ready 落地但快照失敗（大 repo status timeout、repo 中途被
    // 刪）→ environment 仍要換血落地、status/branches 清 null、lastError 記
    // snapshotError。殘留前一個 workspace 的 git 面板會與 Rust 端已切換的
    // RepoHandle 形成跨 workspace 混血顯示。
    it("detect lands the new environment and clears the stale snapshot on snapshotError", async () => {
        const { useGitStore } = await import("./gitStore")
        const ipc = await import("../lib/ipc")
        // 前一個 workspace 的完整 git 面板殘留。
        useGitStore.setState({
            environment: { status: "ready", root: "/old", version: "2.50.1" },
            status: makeStatus(),
            branches: { local: [], remote: [] }
        })
        ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
            environment: { status: "ready", root: "/new", version: "2.50.1" },
            status: null,
            branches: null,
            snapshotError: "git status timed out"
        })
        await useGitStore.getState().detect("/new")
        expect(useGitStore.getState().environment).toEqual({
            status: "ready",
            root: "/new",
            version: "2.50.1"
        })
        expect(useGitStore.getState().status).toBe(null)
        expect(useGitStore.getState().branches).toBe(null)
        expect(useGitStore.getState().lastError).toContain("timed out")
    })

    // #57 覆核修正：ready→ready 切換的 stale-resolve 丟棄要比對 root——只看
    // status 是否 ready 擋不住「A 的慢 status 晚到、蓋掉 B 的 bootstrap 快照」。
    it("discards a stale status resolve after a ready→ready workspace switch (root guard)", async () => {
        const { useGitStore } = await import("./gitStore")
        const ipc = await import("../lib/ipc")
        useGitStore.setState({ environment: { status: "ready", root: "/a", version: "2.50.1" } })
        let release = () => {}
        ;(ipc.gitStatus as ReturnType<typeof vi.fn>).mockImplementationOnce(
            () => new Promise((res) => { release = () => res({ ...makeStatus(), branch: "a-branch" }) })
        )
        void useGitStore.getState().refresh()
        await vi.advanceTimersByTimeAsync(300) // gitStatus(A) called, hanging
        // detect(B) 的 bootstrap 快照落地：environment 換到 /b、status 為 B 的。
        const bStatus = { ...makeStatus(), branch: "b-branch" }
        useGitStore.setState({
            environment: { status: "ready", root: "/b", version: "2.50.1" },
            status: bStatus
        })
        release()
        await Promise.resolve()
        await Promise.resolve()
        // A 的晚到 resolve 必須被丟棄：B 的面板不得顯示 A 的變更清單。
        expect(useGitStore.getState().status).toBe(bStatus)
        expect(useGitStore.getState().status?.branch).toBe("b-branch")
    })

    it("discards a stale status rejection silently after a ready→ready workspace switch", async () => {
        const { useGitStore } = await import("./gitStore")
        const ipc = await import("../lib/ipc")
        useGitStore.setState({ environment: { status: "ready", root: "/a", version: "2.50.1" } })
        let reject = () => {}
        ;(ipc.gitStatus as ReturnType<typeof vi.fn>).mockImplementationOnce(
            () => new Promise((_res, rej) => { reject = () => rej(new Error("a-repo boom")) })
        )
        void useGitStore.getState().refresh()
        await vi.advanceTimersByTimeAsync(300)
        useGitStore.setState({ environment: { status: "ready", root: "/b", version: "2.50.1" } })
        reject()
        await Promise.resolve()
        await Promise.resolve()
        // 舊 workspace 的失敗不得在新 workspace 冒 lastError。
        expect(useGitStore.getState().lastError).toBe(null)
    })

    it("discards a stale branches resolve after a ready→ready workspace switch (root guard)", async () => {
        const { useGitStore } = await import("./gitStore")
        const ipc = await import("../lib/ipc")
        useGitStore.setState({ environment: { status: "ready", root: "/a", version: "2.50.1" } })
        let release = () => {}
        ;(ipc.gitBranches as ReturnType<typeof vi.fn>).mockImplementationOnce(
            () =>
                new Promise((res) => {
                    release = () =>
                        res({
                            local: [{ name: "a-branch", upstream: null, ahead: 0, behind: 0, isCurrent: true }],
                            remote: []
                        })
                })
        )
        const pending = useGitStore.getState().loadBranches()
        // detect(B) 的 bootstrap 快照落地。
        const bBranches = {
            local: [{ name: "b-branch", upstream: null, ahead: 0, behind: 0, isCurrent: true }],
            remote: []
        }
        useGitStore.setState({
            environment: { status: "ready", root: "/b", version: "2.50.1" },
            branches: bBranches
        })
        release()
        await pending
        // A 的晚到 branches 不得蓋掉 B 的首載快照。
        expect(useGitStore.getState().branches).toBe(bBranches)
    })

    it("refreshQuiet discards a stale status resolve across a workspace switch", async () => {
        const { useGitStore } = await import("./gitStore")
        const ipc = await import("../lib/ipc")
        useGitStore.setState({ environment: { status: "ready", root: "/a", version: "2.50.1" } })
        let release = () => {}
        ;(ipc.gitStatus as ReturnType<typeof vi.fn>).mockImplementationOnce(
            () => new Promise((res) => { release = () => res({ ...makeStatus(), branch: "a-branch" }) })
        )
        const pending = useGitStore.getState().refreshQuiet()
        const bStatus = { ...makeStatus(), branch: "b-branch" }
        useGitStore.setState({
            environment: { status: "ready", root: "/b", version: "2.50.1" },
            status: bStatus
        })
        release()
        await pending
        expect(useGitStore.getState().status).toBe(bStatus)
    })

    it("discards a stale detect resolution when a newer detect has started (#55 T1 race)", async () => {
        const { useGitStore } = await import("./gitStore")
        const ipc = await import("../lib/ipc")
        // detect(old) hangs — a slow repo on the workspace being switched away from…
        let resolveOld: (env: unknown) => void = () => {}
        ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockImplementationOnce(
            () => new Promise((res) => { resolveOld = res })
        )
        const oldDetect = useGitStore.getState().detect("/old")
        // …while detect(new) starts later and resolves first (async 化後完成順序可反轉).
        ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
            makeBootstrap("/new")
        )
        await useGitStore.getState().detect("/new")
        expect(useGitStore.getState().environment).toEqual({
            status: "ready",
            root: "/new",
            version: "2.50.1"
        })
        expect(useGitStore.getState().status).not.toBe(null)
        // The old workspace's late resolve must be discarded whole: environment
        // stays on /new and the freshly populated status is not cleared to null.
        resolveOld(makeBootstrap("/old"))
        await oldDetect
        expect(useGitStore.getState().environment).toEqual({
            status: "ready",
            root: "/new",
            version: "2.50.1"
        })
        expect(useGitStore.getState().status).not.toBe(null)
    })

    it("discards a stale detect rejection silently when a newer detect has started (#55 T1 race)", async () => {
        const { useGitStore } = await import("./gitStore")
        const ipc = await import("../lib/ipc")
        let rejectOld: (e: unknown) => void = () => {}
        ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockImplementationOnce(
            () => new Promise((_res, rej) => { rejectOld = rej })
        )
        const oldDetect = useGitStore.getState().detect("/old")
        ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
            environment: { status: "notARepo" },
            status: null,
            branches: null
        })
        await useGitStore.getState().detect("/new")
        // A late failure from the abandoned workspace must not surface lastError
        // noise on the new one.
        rejectOld(new Error("old workspace boom"))
        await oldDetect
        expect(useGitStore.getState().lastError).toBe(null)
        expect(useGitStore.getState().environment).toEqual({ status: "notARepo" })
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
        ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
            environment: { status: "notARepo" },
            status: null,
            branches: null
        })
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

    it("reuses one changed-path index for the same Git status snapshot", async () => {
        const { changedPathSet } = await import("./gitStore")
        const status: GitStatus = {
            ...makeStatus(),
            unstaged: [{ path: "src/changed.ts", origPath: null, status: "M" }],
            untracked: ["src/new.ts"],
            conflicted: [{ path: "src/conflict.ts", origPath: null, status: "UU" }]
        }

        const first = changedPathSet(status)

        expect([...first].sort()).toEqual(["src/changed.ts", "src/conflict.ts", "src/new.ts"])
        expect(changedPathSet(status)).toBe(first)
        expect(changedPathSet(null)).toBe(changedPathSet(null))
    })

    it("isolates changed-path indexes between Git status snapshots", async () => {
        const { changedPathSet } = await import("./gitStore")
        const previous = { ...makeStatus(), untracked: ["old.ts"] }
        const current = { ...makeStatus(), untracked: ["current.ts"] }

        const previousIndex = changedPathSet(previous)
        const currentIndex = changedPathSet(current)

        expect(currentIndex).not.toBe(previousIndex)
        expect(currentIndex.has("current.ts")).toBe(true)
        expect(currentIndex.has("old.ts")).toBe(false)
    })

    it("reuses the index across large rendered-node lookup batches", async () => {
        const { changedPathSet } = await import("./gitStore")
        const status = {
            ...makeStatus(),
            untracked: Array.from({ length: 2_000 }, (_, index) => `generated/file-${index}.ts`)
        }
        const index = changedPathSet(status)

        for (let node = 0; node < 4_000; node += 1) {
            const lookupIndex = changedPathSet(status)
            expect(lookupIndex).toBe(index)
            expect(lookupIndex.has(`generated/file-${node % 2_000}.ts`)).toBe(true)
        }
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

    it("runOp records cherry-pick with the mapped console label", async () => {
        const { useGitStore } = await import("./gitStore")
        const opDone = useGitStore.getState().runOp("cherry-pick", async () => {})
        await vi.advanceTimersByTimeAsync(400)
        expect(await opDone).toBe(true)
        expect(useGitStore.getState().consoleLog[0]?.cmd).toBe("git cherry-pick")
    })

    it("setCommitMessage stores the draft; initial state is empty", async () => {
        const { useGitStore, initialGitState } = await import("./gitStore")
        expect(initialGitState.commitMessage).toBe("")
        useGitStore.getState().setCommitMessage("feat: x")
        expect(useGitStore.getState().commitMessage).toBe("feat: x")
    })

    // #58 T4a：per-workspace git 快照（stale-while-revalidate）。切回已開過的
    // workspace 時先 hydrate 上次快照（零空白），背景 bootstrap 完成後覆蓋收斂。
    describe("workspace snapshots (#58 T4a)", () => {
        function makeBranches(name: string) {
            return {
                local: [{ name, upstream: null, ahead: 0, behind: 0, isCurrent: true }],
                remote: []
            }
        }

        it("hydrates the cached snapshot immediately on A→B→A, then background bootstrap overwrites", async () => {
            const { useGitStore } = await import("./gitStore")
            const ipc = await import("../lib/ipc")
            // detect(/a) 落地 → 快照播種。
            ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
                environment: { status: "ready", root: "/a", version: "2.50.1" },
                status: { ...makeStatus(), branch: "a-branch" },
                branches: makeBranches("a-branch")
            })
            await useGitStore.getState().detect("/a")
            // detect(/b) 落地 → 面板換血成 B。
            ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockResolvedValueOnce(makeBootstrap("/b"))
            await useGitStore.getState().detect("/b")
            expect(useGitStore.getState().environment).toEqual({
                status: "ready",
                root: "/b",
                version: "2.50.1"
            })
            // 切回 /a：bootstrap 掛住，hydrate 必須「立即」把 A 的快照放回面板。
            let release: (r: unknown) => void = () => {}
            ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockImplementationOnce(
                () => new Promise((res) => { release = res })
            )
            const pending = useGitStore.getState().detect("/a")
            expect(useGitStore.getState().environment).toEqual({
                status: "ready",
                root: "/a",
                version: "2.50.1"
            })
            expect(useGitStore.getState().status?.branch).toBe("a-branch")
            expect(useGitStore.getState().branches).toEqual(makeBranches("a-branch"))
            expect(useGitStore.getState().snapshotStale).toBe(true)
            // 背景 bootstrap 完成 → 以新資料覆蓋、stale 收斂。
            release({
                environment: { status: "ready", root: "/a", version: "2.50.1" },
                status: { ...makeStatus(), branch: "a-fresh" },
                branches: makeBranches("a-fresh")
            })
            await pending
            expect(useGitStore.getState().status?.branch).toBe("a-fresh")
            expect(useGitStore.getState().branches).toEqual(makeBranches("a-fresh"))
            expect(useGitStore.getState().snapshotStale).toBe(false)
        })

        it("keeps hydrated data (still stale) and records lastError when background bootstrap fails", async () => {
            const { useGitStore } = await import("./gitStore")
            const ipc = await import("../lib/ipc")
            ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
                environment: { status: "ready", root: "/a", version: "2.50.1" },
                status: { ...makeStatus(), branch: "a-branch" },
                branches: makeBranches("a-branch")
            })
            await useGitStore.getState().detect("/a")
            ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockResolvedValueOnce(makeBootstrap("/b"))
            await useGitStore.getState().detect("/b")
            ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new Error("bootstrap boom"))
            await useGitStore.getState().detect("/a")
            // 快照留在面板上（總比空白好），錯誤照舊記 lastError，stale 未收斂。
            expect(useGitStore.getState().status?.branch).toBe("a-branch")
            expect(useGitStore.getState().snapshotStale).toBe(true)
            expect(useGitStore.getState().lastError).toContain("bootstrap boom")
        })

        it("keeps the snapshot in sync with later refresh/loadBranches, so re-hydrate shows the newest data", async () => {
            const { useGitStore } = await import("./gitStore")
            const ipc = await import("../lib/ipc")
            ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
                environment: { status: "ready", root: "/a", version: "2.50.1" },
                status: { ...makeStatus(), branch: "a-branch" },
                branches: makeBranches("a-branch")
            })
            await useGitStore.getState().detect("/a")
            // watcher 觸發的 refresh 與 loadBranches 更新面板 → 快照必須同步更新。
            ;(ipc.gitStatus as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
                ...makeStatus(),
                branch: "after-refresh"
            })
            void useGitStore.getState().refresh()
            await vi.advanceTimersByTimeAsync(400)
            ;(ipc.gitBranches as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
                makeBranches("after-refresh")
            )
            await useGitStore.getState().loadBranches()
            // 切去 /b 再切回 /a（bootstrap 掛住）→ hydrate 的是 refresh 後的資料。
            ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockResolvedValueOnce(makeBootstrap("/b"))
            await useGitStore.getState().detect("/b")
            ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockImplementationOnce(
                () => new Promise(() => {})
            )
            void useGitStore.getState().detect("/a")
            expect(useGitStore.getState().status?.branch).toBe("after-refresh")
            expect(useGitStore.getState().branches).toEqual(makeBranches("after-refresh"))
            expect(useGitStore.getState().snapshotStale).toBe(true)
        })

        it("does not hydrate (and drops the snapshot) once a workspace stops being a repo", async () => {
            const { useGitStore } = await import("./gitStore")
            const ipc = await import("../lib/ipc")
            // /p 曾是 repo → 快照播種。
            ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockResolvedValueOnce(makeBootstrap("/p"))
            await useGitStore.getState().detect("/p")
            // 再 detect(/p) 回 notARepo → 面板清空、快照失效。
            ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
                environment: { status: "notARepo" },
                status: null,
                branches: null
            })
            await useGitStore.getState().detect("/p")
            expect(useGitStore.getState().status).toBe(null)
            expect(useGitStore.getState().branches).toBe(null)
            // 第三次 detect(/p)（掛住）→ 不得 hydrate 舊 repo 資料（非 repo 不殘留）。
            ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockImplementationOnce(
                () => new Promise(() => {})
            )
            void useGitStore.getState().detect("/p")
            expect(useGitStore.getState().environment).toEqual({ status: "notARepo" })
            expect(useGitStore.getState().status).toBe(null)
            expect(useGitStore.getState().snapshotStale).toBe(false)
        })

        it("evicts the least-recently-used snapshot beyond the LRU cap of 8", async () => {
            const { useGitStore } = await import("./gitStore")
            const ipc = await import("../lib/ipc")
            // 依序 detect 9 個 workspace → /w1 應被逐出，/w2../w9 保留。
            for (let i = 1; i <= 9; i++) {
                ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
                    makeBootstrap(`/w${i}`)
                )
                await useGitStore.getState().detect(`/w${i}`)
            }
            // 切回 /w1（掛住）→ 快照已被逐出，不 hydrate：面板仍是 /w9。
            ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockImplementationOnce(
                () => new Promise(() => {})
            )
            void useGitStore.getState().detect("/w1")
            expect(useGitStore.getState().environment).toEqual({
                status: "ready",
                root: "/w9",
                version: "2.50.1"
            })
            expect(useGitStore.getState().snapshotStale).toBe(false)
            // 切回 /w2（掛住）→ 仍在 LRU 內，正常 hydrate。
            ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockImplementationOnce(
                () => new Promise(() => {})
            )
            void useGitStore.getState().detect("/w2")
            expect(useGitStore.getState().environment).toEqual({
                status: "ready",
                root: "/w2",
                version: "2.50.1"
            })
            expect(useGitStore.getState().snapshotStale).toBe(true)
        })

        it("touches LRU recency on hydrate so a recently revisited workspace is not evicted", async () => {
            const { useGitStore } = await import("./gitStore")
            const ipc = await import("../lib/ipc")
            for (let i = 1; i <= 8; i++) {
                ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
                    makeBootstrap(`/w${i}`)
                )
                await useGitStore.getState().detect(`/w${i}`)
            }
            // 重訪 /w1 → recency 提到最前。
            ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockResolvedValueOnce(makeBootstrap("/w1"))
            await useGitStore.getState().detect("/w1")
            // 再開新的 /w9 → 超出上限，被逐出的應是 /w2（最舊），不是 /w1。
            ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockResolvedValueOnce(makeBootstrap("/w9"))
            await useGitStore.getState().detect("/w9")
            // /w1 仍可 hydrate。
            ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockImplementationOnce(
                () => new Promise(() => {})
            )
            void useGitStore.getState().detect("/w1")
            expect(useGitStore.getState().environment).toEqual({
                status: "ready",
                root: "/w1",
                version: "2.50.1"
            })
            expect(useGitStore.getState().snapshotStale).toBe(true)
            // /w2 已被逐出：切回不 hydrate（面板停在 /w1 的 hydrate 結果）。
            ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockImplementationOnce(
                () => new Promise(() => {})
            )
            void useGitStore.getState().detect("/w2")
            expect(useGitStore.getState().environment).toEqual({
                status: "ready",
                root: "/w1",
                version: "2.50.1"
            })
        })

        // #58 覆核修正：hydrate 後、bootstrap 落地前，Rust 端 repo state 可能仍
        // 指向前一個 workspace——此窗口內 gitStatus/gitBranches 會以「舊 repo」
        // 回應，而前端 root guard 兩端比的都是前端 env root（hydrate 後同為新
        // root）擋不住。窗口內必須抑制 refresh/refreshQuiet/loadBranches，否則
        // 舊 repo 的資料會掛在新 workspace 標頭下、還寫進新 workspace 的快照。
        describe("detect in-flight suppression（Rust repo state 時序盲視窗口）", () => {
            /** detect(/a) 落地播種 → detect(/b) 落地 → detect(/a) 掛住（hydrate
             *  stale）。回傳 release 讓測試自行決定何時讓 bootstrap 落地。 */
            async function enterHydrateWindow(
                useGitStore: (typeof import("./gitStore"))["useGitStore"],
                ipc: typeof import("../lib/ipc")
            ) {
                ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
                    environment: { status: "ready", root: "/a", version: "2.50.1" },
                    status: { ...makeStatus(), branch: "a-branch" },
                    branches: makeBranches("a-branch")
                })
                await useGitStore.getState().detect("/a")
                ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
                    makeBootstrap("/b")
                )
                await useGitStore.getState().detect("/b")
                let release: (r: unknown) => void = () => {}
                ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockImplementationOnce(
                    () => new Promise((res) => { release = res })
                )
                const pending = useGitStore.getState().detect("/a")
                // hydrate 已落地：前端 env root=/a，但（真實世界裡）Rust 端可能
                // 仍服務 /b 的 repo——窗口成立。
                expect(useGitStore.getState().snapshotStale).toBe(true)
                return { pending, release }
            }

            it("suppresses a refresh initiated inside the hydrate window, then reruns it after bootstrap lands", async () => {
                const { useGitStore } = await import("./gitStore")
                const ipc = await import("../lib/ipc")
                const { pending, release } = await enterHydrateWindow(useGitStore, ipc)
                // 窗口內的 focus/watcher refresh：不得發出 gitStatus（Rust 端可能
                // 回舊 repo 的 status，root guard 擋不住）。
                void useGitStore.getState().refresh()
                await vi.advanceTimersByTimeAsync(400)
                expect(ipc.gitStatus).not.toHaveBeenCalled()
                // 面板維持 hydrate 內容，快照未被污染。
                expect(useGitStore.getState().status?.branch).toBe("a-branch")
                // bootstrap 落地 → 被抑制的 refresh 補跑一次（fs 變更不漏）。
                release({
                    environment: { status: "ready", root: "/a", version: "2.50.1" },
                    status: { ...makeStatus(), branch: "a-fresh" },
                    branches: makeBranches("a-fresh")
                })
                await pending
                expect(useGitStore.getState().status?.branch).toBe("a-fresh")
                await vi.advanceTimersByTimeAsync(400)
                expect(ipc.gitStatus).toHaveBeenCalledTimes(1)
            })

            it("abandons a debounced refresh whose timer fires inside the hydrate window (rerun after landing)", async () => {
                const { useGitStore } = await import("./gitStore")
                const ipc = await import("../lib/ipc")
                ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
                    environment: { status: "ready", root: "/a", version: "2.50.1" },
                    status: { ...makeStatus(), branch: "a-branch" },
                    branches: makeBranches("a-branch")
                })
                await useGitStore.getState().detect("/a")
                ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
                    makeBootstrap("/b")
                )
                await useGitStore.getState().detect("/b")
                // refresh 於切換前排入（debounce 計時中）……
                void useGitStore.getState().refresh()
                // ……timer 未到，detect(/a) 進場（hydrate、bootstrap 掛住）。
                let release: (r: unknown) => void = () => {}
                ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockImplementationOnce(
                    () => new Promise((res) => { release = res })
                )
                const pending = useGitStore.getState().detect("/a")
                await vi.advanceTimersByTimeAsync(400)
                // timer 在窗口內開火 → 放棄本次 fetch。
                expect(ipc.gitStatus).not.toHaveBeenCalled()
                release({
                    environment: { status: "ready", root: "/a", version: "2.50.1" },
                    status: { ...makeStatus(), branch: "a-fresh" },
                    branches: makeBranches("a-fresh")
                })
                await pending
                await vi.advanceTimersByTimeAsync(400)
                expect(ipc.gitStatus).toHaveBeenCalledTimes(1)
            })

            it("refreshQuiet is a no-op inside the hydrate window (no cross-repo write into the snapshot)", async () => {
                const { useGitStore } = await import("./gitStore")
                const ipc = await import("../lib/ipc")
                await enterHydrateWindow(useGitStore, ipc)
                await useGitStore.getState().refreshQuiet()
                expect(ipc.gitStatus).not.toHaveBeenCalled()
                expect(useGitStore.getState().status?.branch).toBe("a-branch")
            })

            it("loadBranches is a no-op inside the hydrate window", async () => {
                const { useGitStore } = await import("./gitStore")
                const ipc = await import("../lib/ipc")
                await enterHydrateWindow(useGitStore, ipc)
                await useGitStore.getState().loadBranches()
                expect(ipc.gitBranches).not.toHaveBeenCalled()
                expect(useGitStore.getState().branches).toEqual(makeBranches("a-branch"))
            })

            it("drops the suppressed rerun when bootstrap fails (hydrated content stays, no phantom fetch)", async () => {
                const { useGitStore } = await import("./gitStore")
                const ipc = await import("../lib/ipc")
                ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
                    environment: { status: "ready", root: "/a", version: "2.50.1" },
                    status: { ...makeStatus(), branch: "a-branch" },
                    branches: makeBranches("a-branch")
                })
                await useGitStore.getState().detect("/a")
                ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockResolvedValueOnce(
                    makeBootstrap("/b")
                )
                await useGitStore.getState().detect("/b")
                let reject: (e: unknown) => void = () => {}
                ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockImplementationOnce(
                    () => new Promise((_res, rej) => { reject = rej })
                )
                const pending = useGitStore.getState().detect("/a")
                void useGitStore.getState().refresh() // 窗口內 → 抑制
                reject(new Error("bootstrap boom"))
                await pending
                await vi.advanceTimersByTimeAsync(400)
                // 失敗後不補跑（Rust state 歸屬仍不明）；hydrate 內容與 stale 保留。
                expect(ipc.gitStatus).not.toHaveBeenCalled()
                expect(useGitStore.getState().status?.branch).toBe("a-branch")
                expect(useGitStore.getState().snapshotStale).toBe(true)
            })
        })

        it("does not seed a snapshot from a stale detect resolve of an abandoned workspace", async () => {
            const { useGitStore } = await import("./gitStore")
            const ipc = await import("../lib/ipc")
            // detect(/old) 掛住 → detect(/new) 先落地 → /old 的晚到 resolve 被丟棄，
            // 也不得替 /old 播種快照。
            let resolveOld: (r: unknown) => void = () => {}
            ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockImplementationOnce(
                () => new Promise((res) => { resolveOld = res })
            )
            const oldDetect = useGitStore.getState().detect("/old")
            ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockResolvedValueOnce(makeBootstrap("/new"))
            await useGitStore.getState().detect("/new")
            resolveOld(makeBootstrap("/old"))
            await oldDetect
            // 切到 /old（掛住）→ 不得 hydrate（快照從未合法播種）。
            ;(ipc.gitBootstrap as ReturnType<typeof vi.fn>).mockImplementationOnce(
                () => new Promise(() => {})
            )
            void useGitStore.getState().detect("/old")
            expect(useGitStore.getState().environment).toEqual({
                status: "ready",
                root: "/new",
                version: "2.50.1"
            })
            expect(useGitStore.getState().snapshotStale).toBe(false)
        })
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
