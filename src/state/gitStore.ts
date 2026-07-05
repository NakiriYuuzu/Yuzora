import { create } from "zustand"

import {
    gitBranches,
    gitDetect,
    gitFetch,
    gitRemoteProbe,
    gitStatus
} from "../lib/ipc"
import type { BranchList, GitEnvironment, GitStatus, RemoteProbe } from "../lib/types"

export type RemoteCheckMode = "off" | "probe" | "autofetch"
export interface RemoteCheckConfig {
    mode: RemoteCheckMode
    intervalSec: number
}

// Console tab entry (design §Console dc.html:918-932 / prototype gitLog
// dc.html:2391-2394). One record per runOp completion, newest prepended.
export interface GitConsoleEntry {
    id: number
    cmd: string
    out: string[]
    tone: "ok" | "err"
    time: string
}

// Ring-buffer cap — matches the prototype's "keep recent history" intent
// without unbounded growth.
export const CONSOLE_LOG_LIMIT = 200

// runOp op name → human-readable git command shown in the Console. runOp does
// not carry op arguments (message/branch), so these are static descriptions
// matching the design prototype's phrasing (dc.html:2396-2402, 2394). Unknown
// names fall back to `git <name>`.
const CONSOLE_CMD_LABELS: Record<string, string> = {
    fetch: "git fetch",
    pull: "git pull --rebase",
    push: "git push",
    stage: "git add",
    unstage: "git restore --staged",
    discard: "git restore",
    commit: 'git commit -m "…"',
    checkout: "git checkout",
    "create-branch": "git branch",
    "conflict-abort": "git merge --abort",
    "conflict-continue": "git merge --continue"
}

function consoleCmdLabel(name: string): string {
    return CONSOLE_CMD_LABELS[name] ?? `git ${name}`
}

function consoleTime(now = new Date()): string {
    const h = String(now.getHours()).padStart(2, "0")
    const m = String(now.getMinutes()).padStart(2, "0")
    return `${h}:${m}`
}

export const REMOTE_CHECK_STORAGE_KEY = "yuzora.git.remoteCheck.v1"

const DEFAULT_REMOTE_CHECK: RemoteCheckConfig = { mode: "probe", intervalSec: 180 }
const REFRESH_DEBOUNCE_MS = 300

function loadRemoteCheck(): RemoteCheckConfig {
    try {
        const raw = localStorage.getItem(REMOTE_CHECK_STORAGE_KEY)
        if (!raw) return DEFAULT_REMOTE_CHECK
        const parsed = JSON.parse(raw) as Partial<RemoteCheckConfig>
        const mode = parsed.mode
        const intervalSec = parsed.intervalSec
        if (
            (mode === "off" || mode === "probe" || mode === "autofetch") &&
            typeof intervalSec === "number" &&
            intervalSec > 0
        ) {
            return { mode, intervalSec }
        }
    } catch {
        // ignore malformed storage; fall through to default
    }
    return DEFAULT_REMOTE_CHECK
}

interface GitState {
    environment: GitEnvironment | null
    status: GitStatus | null
    branches: BranchList | null
    busy: string | null
    lastError: string | null
    remoteIncoming: RemoteProbe
    remotePaused: boolean
    remoteCheck: RemoteCheckConfig
    consoleLog: GitConsoleEntry[]
    commitMessage: string
    setCommitMessage: (message: string) => void
    appendConsole: (entry: GitConsoleEntry) => void
    detect: (workspacePath: string) => Promise<void>
    refresh: (paths?: string[]) => Promise<void>
    refreshQuiet: (paths?: string[]) => Promise<void>
    loadBranches: () => Promise<void>
    runOp: (name: string, fn: () => Promise<unknown>) => Promise<boolean>
    checkRemote: () => Promise<void>
    setRemoteCheck: (cfg: RemoteCheckConfig) => void
}

export const initialGitState = {
    environment: null,
    status: null,
    branches: null,
    busy: null,
    lastError: null,
    remoteIncoming: "unknown" as RemoteProbe,
    remotePaused: false,
    remoteCheck: loadRemoteCheck(),
    consoleLog: [] as GitConsoleEntry[],
    // Commit message lives in the store (not local component state) so the
    // sidebar commit card and any future entry share one draft and it survives
    // mode switches (E1 §1.3).
    commitMessage: ""
}

// Monotonic id source for console entries — survives store resets so ids stay
// unique across a session (used only as React keys / ordering, not persisted).
let consoleSeq = 0

// Debounce + single-flight state lives in module scope so it survives across
// component re-renders and is observable under fake timers. `timer` holds the
// pending trailing-debounce handle; `inflight` is the shared promise every
// caller within one debounce window awaits (等同一班機).
let timer: ReturnType<typeof setTimeout> | null = null
let inflight: Promise<void> | null = null
// Set when a refresh is requested while a status fetch is already in flight (past
// the debounce). The current fetch runs one more time on completion so a change
// that landed mid-flight isn't lost. Bounded: reset before the single rerun.
let pendingRerun = false

export const useGitStore = create<GitState>()((set, get) => ({
    ...initialGitState,

    setCommitMessage: (message) => set({ commitMessage: message }),

    // Prepend newest-first, cap at CONSOLE_LOG_LIMIT dropping the tail.
    appendConsole: (entry) => {
        set((s) => ({ consoleLog: [entry, ...s.consoleLog].slice(0, CONSOLE_LOG_LIMIT) }))
    },

    detect: async (workspacePath) => {
        try {
            const environment = await gitDetect(workspacePath)
            // 切換 workspace 時清掉舊 repo 殘留（notARepo/missing 不再 refresh，否則舊
            // status/branches/remote 狀態會殘存誤導 UI）。ready 分支下方隨即重載。
            set({
                environment,
                status: null,
                branches: null,
                remoteIncoming: "unknown",
                remotePaused: false
            })
            if (environment.status === "ready") {
                await Promise.all([get().refresh(), get().loadBranches()])
            }
        } catch (e) {
            set({ lastError: String(e) })
        }
    },

    refresh: (paths) => {
        // Non-ready environments (fs/focus-driven refreshes before detect, or a
        // non-repo workspace) must not touch git or write lastError (background
        // noise rule, m2).
        if (get().environment?.status !== "ready") return Promise.resolve()
        if (inflight) {
            // Within the debounce window (timer still pending) calls just coalesce;
            // once the fetch is actually running, remember to run once more so a
            // change that landed mid-flight isn't lost (m3).
            if (!timer) pendingRerun = true
            return inflight
        }
        inflight = new Promise<void>((resolve) => {
            if (timer) clearTimeout(timer)
            timer = setTimeout(async () => {
                timer = null
                // Re-check readiness at execution time: the environment can flip to
                // non-ready (notARepo/missing) during the debounce window, and
                // running the fetch then would write lastError — the very background
                // noise m2 removes (F2). Abandon the fetch and drop any pending rerun
                // so no stale flag lingers.
                if (get().environment?.status !== "ready") {
                    pendingRerun = false
                    inflight = null
                    resolve()
                    return
                }
                try {
                    const status = await gitStatus(paths)
                    // Re-check after the await: the environment can flip to
                    // non-ready while the fetch is in flight (detect() switching to
                    // a non-repo workspace clears status and does not refresh), so a
                    // stale resolve would re-fill the just-cleared status. Discard it
                    // (F-1).
                    if (get().environment?.status === "ready") set({ status })
                } catch (e) {
                    // Same guard for a stale rejection — a failure from the old
                    // workspace must not surface lastError noise on the new one (F-1).
                    if (get().environment?.status === "ready") set({ lastError: String(e) })
                } finally {
                    inflight = null
                    resolve()
                    if (pendingRerun) {
                        pendingRerun = false
                        // Same guard before the rerun: skip (not just entry-gate) if
                        // the environment went non-ready while the fetch was running.
                        if (get().environment?.status === "ready") {
                            void get().refresh(paths)
                        }
                    }
                }
            }, REFRESH_DEBOUNCE_MS)
        })
        return inflight
    },

    // Background refresh for checkRemote: updates `status` on success but keeps
    // failures silent — never writes lastError, and rethrows so checkRemote's
    // catch can set remotePaused. Deliberately bypasses the loud `refresh`
    // debounce/single-flight to avoid inheriting a merged loud caller's error
    // attribution (a foreground refresh in the same window would otherwise route
    // the failure into lastError, breaking the background silence rule).
    refreshQuiet: async (paths) => {
        const status = await gitStatus(paths)
        set({ status })
    },

    loadBranches: async () => {
        try {
            const branches = await gitBranches()
            set({ branches })
        } catch (e) {
            set({ lastError: String(e) })
        }
    },

    runOp: async (name, fn) => {
        if (get().busy) return false
        set({ busy: name })
        // Single-point Console wiring: every runOp completion (success and
        // failure) records one entry here. The IPC layer returns no stdout, so
        // success shows "Done" and failure shows the error message (brief B1 —
        // do not touch the Rust side just to surface stdout).
        const cmd = consoleCmdLabel(name)
        try {
            await fn()
            set({ lastError: null })
            if (name === "fetch") set({ remotePaused: false, remoteIncoming: "no" })
            get().appendConsole({
                id: ++consoleSeq,
                cmd,
                out: ["Done"],
                tone: "ok",
                time: consoleTime()
            })
            await Promise.all([get().refresh(), get().loadBranches()])
            return true
        } catch (e) {
            set({ lastError: String(e) })
            get().appendConsole({
                id: ++consoleSeq,
                cmd,
                out: [String(e)],
                tone: "err",
                time: consoleTime()
            })
            return false
        } finally {
            set({ busy: null })
        }
    },

    checkRemote: async () => {
        // busy：不與前景 op 爭用（spec 契約）。remotePaused：背景檢查已因失敗停檢，
        // 待手動 fetch 成功復位（runOp fetch 分支清 remotePaused）——閉環成立。
        if (get().busy || get().remotePaused) return
        const { mode } = get().remoteCheck
        if (mode === "off") return
        try {
            if (mode === "probe") {
                const remoteIncoming = await gitRemoteProbe()
                set({ remoteIncoming })
            } else {
                await gitFetch(true)
                await get().refreshQuiet()
            }
        } catch {
            // Background auth/network failures must stay silent: pause future
            // checks, never surface an error or trigger an interactive path.
            set({ remotePaused: true })
        }
    },

    setRemoteCheck: (cfg) => {
        try {
            localStorage.setItem(REMOTE_CHECK_STORAGE_KEY, JSON.stringify(cfg))
        } catch {
            // localStorage unavailable (private mode / quota); keep in-memory only
        }
        set({ remoteCheck: cfg })
    }
}))

export function changedPathSet(status: GitStatus | null): Set<string> {
    const set = new Set<string>()
    if (!status) return set
    for (const entry of status.unstaged) set.add(entry.path)
    for (const path of status.untracked) set.add(path)
    for (const entry of status.conflicted) set.add(entry.path)
    return set
}
