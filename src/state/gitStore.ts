import { create } from "zustand"

import {
    gitBootstrap,
    gitBranches,
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

export interface RunOpOptions {
    conflictOp?: string
    /** Runs only after `fn` succeeds and before post-mutation refresh. */
    afterMutationBeforeRefresh?: () => void | Promise<void>
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
    rollback: "git restore --staged --worktree",
    commit: 'git commit -m "…"',
    checkout: "git checkout",
    "cherry-pick": "git cherry-pick",
    "create-branch": "git branch",
    "conflict-abort": "git merge --abort",
    "conflict-continue": "git merge --continue"
}

function consoleCmdLabel(name: string, options?: RunOpOptions): string {
    if (options?.conflictOp && name === "conflict-abort") {
        return `git ${options.conflictOp} --abort`
    }
    if (options?.conflictOp && name === "conflict-continue") {
        return `git ${options.conflictOp} --continue`
    }
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

// #58 T4a：per-workspace git 快照（stale-while-revalidate）。切回已開過的
// workspace 時先 hydrate 上次離開時的面板內容（零空白），背景 bootstrap 完成
// 後覆蓋收斂。設計上不信任快照的正確性——只用它消除空白等待（spec Phase 3.1）。
interface GitSnapshot {
    environment: Extract<GitEnvironment, { status: "ready" }>
    status: GitStatus | null
    branches: BranchList | null
    at: number
}

// 快照以 detect 的 workspacePath 為 key（非 repo root：detect 進場時只有
// workspacePath 可查；同 repo 多 workspace 子目錄各自成桶，內容等價無害）。
// 模組級而非 store state：快照是快取不是 UI 狀態，不觸發訂閱者重渲染。
export const GIT_SNAPSHOT_LRU_LIMIT = 8
const snapshots = new Map<string, GitSnapshot>()

// 目前面板內容所屬的快照 key——只在「store 的 environment 真正換血」時跟著換
// （hydrate 或 bootstrap 落地），不在 detect 進場時搶先換：detect(B) 在飛期間
// 舊 workspace A 的 guarded refresh 仍可能落地，那筆更新屬於 A 的快照。
let liveSnapshotKey: string | null = null

// Map 的迭代序＝插入序；delete+set 把 key 提到最尾端（most-recent），逐出時
// 刪最前端（least-recent）。
function writeSnapshot(key: string, snap: GitSnapshot): void {
    snapshots.delete(key)
    snapshots.set(key, snap)
    while (snapshots.size > GIT_SNAPSHOT_LRU_LIMIT) {
        const oldest = snapshots.keys().next().value
        if (oldest === undefined) break
        snapshots.delete(oldest)
    }
}

// 讀取即 touch recency：剛切回的 workspace 不該是下一個被逐出的。
function readSnapshot(key: string): GitSnapshot | null {
    const snap = snapshots.get(key)
    if (!snap) return null
    snapshots.delete(key)
    snapshots.set(key, snap)
    return snap
}

// refresh/loadBranches 的 guarded set 落地後同步回寫快照，讓「切走前最後一次
// 刷新」成為下次 hydrate 的內容。stale resolve 已被 root guard 丟棄、不會走到
// 這裡，所以 liveSnapshotKey 與 store 內容必然同屬一個 workspace。
function syncLiveSnapshot(state: Pick<GitState, "environment" | "status" | "branches">): void {
    if (!liveSnapshotKey) return
    if (state.environment?.status !== "ready") return
    writeSnapshot(liveSnapshotKey, {
        environment: state.environment,
        status: state.status,
        branches: state.branches,
        at: Date.now()
    })
}

// 測試用：快照與 live key／in-flight 旗標是模組級狀態，逐測清空避免串場。
export function clearGitSnapshots(): void {
    snapshots.clear()
    liveSnapshotKey = null
    detectInFlight = false
    refreshAfterDetect = false
    statusEpoch = 0
    branchEpoch = 0
    branchRequestSeq = 0
    resetRefreshFlight()
}

/**
 * Shared pre-action gate for every Git mutation surface. `runOp` remains
 * authoritative; UI/menu/dialog controls must disable with this predicate so
 * users never open a confirmation they cannot complete.
 */
export function gitMutationsBlocked(
    state: Pick<GitState, "busy" | "snapshotStale" | "environment"> = useGitStore.getState()
): boolean {
    return (
        state.busy != null
        || state.snapshotStale
        || state.environment?.status !== "ready"
        || detectInFlight
    )
}

interface GitState {
    environment: GitEnvironment | null
    status: GitStatus | null
    branches: BranchList | null
    // hydrate 後、背景 bootstrap 落地前為 true——僅標記概念（本輪不加 UI 指示）。
    snapshotStale: boolean
    // Monotonic only when a repository status snapshot is accepted. Inline
    // worktree diffs use it as an explicit content identity revision.
    statusRevision: number
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
    runOp: (name: string, fn: () => Promise<unknown>, options?: RunOpOptions) => Promise<boolean>
    checkRemote: () => Promise<void>
    setRemoteCheck: (cfg: RemoteCheckConfig) => void
}

export const initialGitState = {
    environment: null,
    status: null,
    branches: null,
    snapshotStale: false,
    statusRevision: 0,
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
let settleInflight: (() => void) | null = null
// Bumped for every new flight and every reset so a stale timer/IPC/finally
// cannot own the current module flight or its resolver.
let refreshFlightGen = 0
// Path scope for the not-yet-started debounce request, and for one post-flight
// rerun. `undefined` means a full status; `null` means none scheduled.
type RefreshPathScope = string[] | undefined
let scheduledScope: RefreshPathScope | null = null
let pendingRerunScope: RefreshPathScope | null = null

function mergeRefreshPathScope(
    current: RefreshPathScope | null,
    incoming?: string[]
): RefreshPathScope {
    if (incoming === undefined || current === undefined) return undefined
    if (current === null) return [...incoming]
    const merged = new Set(current)
    for (const path of incoming) merged.add(path)
    return [...merged]
}

function resetRefreshFlight(): void {
    refreshFlightGen += 1
    if (timer) {
        clearTimeout(timer)
        timer = null
    }
    scheduledScope = null
    pendingRerunScope = null
    inflight = null
    const settle = settleInflight
    settleInflight = null
    settle?.()
}
// git_detect async 化（#55 T1）後兩個 detect 可真併發、resolve 順序不保證：快速
// 切換 workspace 時舊 workspace 的慢結果可能晚到。以單調序號丟棄過期 resolve /
// reject，避免 stale environment 覆蓋新 workspace（Rust 端 git_detect 亦有同款
// generation guard 保護 repo state 與 watcher，兩端各自守自己的 state）。
let detectSeq = 0
// #58 覆核修正：detect 在飛（bootstrap 尚未落地）期間，Rust 端 repo state 可能
// 仍指向前一個 workspace（detect_commit_and_watch 要到 blocking task 執行才
// commit RepoHandle/.git watcher）。此窗口內 git_status_cmd／git_branches 會以
// 「舊 repo」回應，而 readyRoot guard 兩端比的都是「前端 env root」——hydrate
// 已把 env 換成新 workspace 時，跨 repo 的回應會被放行、還經 syncLiveSnapshot
// 寫進新 workspace 的快照（反向亦然：env 未 hydrate 停在舊 root、Rust 已切到
// 新 repo）。窗口內一律抑制 refresh／refreshQuiet／loadBranches 的發起與落地；
// 被抑制的 refresh 記一筆，bootstrap 落地後補跑一次收斂（fs 變更不漏）。
let detectInFlight = false
let refreshAfterDetect = false
// Invalidates in-flight gitStatus / gitBranches responses sampled before a
// successful mutation (or a landed detect). Capture immediately before IPC;
// publish only when the captured value still matches.
let statusEpoch = 0
let branchEpoch = 0
// Latest-request generation for loadBranches: two same-root/same-epoch
// fetches can still resolve out of order; only the newest seq may publish.
let branchRequestSeq = 0

// ready environment 的 root，否則 null。status/branches 的 stale-resolve 丟棄
// 用：只看 `status === "ready"` 擋不住 ready→ready 的 workspace 切換（A 的慢
// resolve 晚到時新 workspace B 也是 ready），要比對「發起當下」與「resolve 當
// 下」的 root 是否同一個 repo——與 detectSeq 同款語意，各守一條路。
function readyRoot(env: GitEnvironment | null | undefined): string | null {
    return env?.status === "ready" ? env.root : null
}

function statusRequestIsCurrent(
    env: GitEnvironment | null | undefined,
    rootAtFetch: string,
    epochAtFetch: number
): boolean {
    return !detectInFlight && readyRoot(env) === rootAtFetch && epochAtFetch === statusEpoch
}

function branchRequestIsCurrent(
    env: GitEnvironment | null | undefined,
    rootAtFetch: string,
    epochAtFetch: number,
    seqAtFetch: number
): boolean {
    return (
        !detectInFlight
        && readyRoot(env) === rootAtFetch
        && epochAtFetch === branchEpoch
        && seqAtFetch === branchRequestSeq
    )
}

function bumpMutationEpochs(): void {
    statusEpoch += 1
    branchEpoch += 1
}

export const useGitStore = create<GitState>()((set, get) => ({
    ...initialGitState,

    setCommitMessage: (message) => set({ commitMessage: message }),

    // Prepend newest-first, cap at CONSOLE_LOG_LIMIT dropping the tail.
    appendConsole: (entry) => {
        set((s) => ({ consoleLog: [entry, ...s.consoleLog].slice(0, CONSOLE_LOG_LIMIT) }))
    },

    detect: async (workspacePath) => {
        const seq = ++detectSeq
        // bootstrap 落地前 Rust 端 repo state 歸屬不明（見 detectInFlight 註解）：
        // 抑制期開始。只有「仍是最新」的 detect 會在落地時解除。
        detectInFlight = true
        // #58 T4a：有快照先 hydrate——面板立即顯示上次離開時的內容（標記 stale），
        // 消除切回時的空白窗；背景 bootstrap 完成後以真值覆蓋。此時我們是最新的
        // detect（seq 剛取），同步 set 不會與更新的 detect 競態。
        const snapshot = readSnapshot(workspacePath)
        if (snapshot) {
            set({
                environment: snapshot.environment,
                status: snapshot.status,
                branches: snapshot.branches,
                snapshotStale: true,
                remoteIncoming: "unknown",
                remotePaused: false
            })
            // environment 已換血成快照的 → live key 跟著換：hydrate 期間 watcher/
            // focus refresh 落地的更新要寫進「這個」workspace 的快照。
            liveSnapshotKey = workspacePath
        } else {
            // An uncached workspace must never keep repo A actionable while
            // repo B is still bootstrapping. Clear all repository-owned state
            // before the async request starts.
            liveSnapshotKey = null
            set({
                environment: null,
                status: null,
                branches: null,
                snapshotStale: false,
                busy: null,
                lastError: null,
                remoteIncoming: "unknown",
                remotePaused: false,
                commitMessage: ""
            })
        }
        try {
            // #57 T3：首載一趟 bootstrap 回齊 environment＋status＋branches——
            // 消除 detect→status/branches 的兩趟 IPC waterfall，也不吃 refresh 的
            // 300ms debounce（那只留給 watcher/focus 觸發的後續刷新）。
            const { environment, status, branches, snapshotError } = await gitBootstrap(workspacePath)
            // 過期 resolve（更新的 detect 已進場）→ 整段丟棄，不觸碰 store，
            // 抑制旗標歸更新的 detect 管。
            if (seq !== detectSeq) return
            // Landed detect replaces status/branches; drop samples from before it.
            bumpMutationEpochs()
            detectInFlight = false
            // 一次 set 換血：notARepo/missing 帶回 null 清掉舊 repo 殘留；ready
            // 直接帶回首載快照，無「先清空再重抓」的空白窗。ready 但快照失敗
            // （snapshotError）時 status/branches 同樣以 null 換血——environment
            // 必須落地：Rust 端 RepoHandle/.git watcher 已切到新 repo，若殘留舊
            // workspace 的 environment/status，後續 refresh 會以舊 ready 閘放行、
            // 把新 repo 的 status 填進舊 root 標頭底下（跨 workspace 混血）。
            set({
                environment,
                status,
                branches,
                snapshotStale: false,
                ...(environment.status === "ready" && status
                    ? { statusRevision: get().statusRevision + 1 }
                    : {}),
                remoteIncoming: "unknown",
                remotePaused: false,
                // 與舊流程「detect 成功、refresh 失敗」同語意：只記 lastError，
                // 後續 watcher/focus refresh 會自行收斂。
                lastError: snapshotError ?? null
            })
            // #58 T4a：bootstrap 真值落地 → 播種/覆蓋快照。非 ready（notARepo/
            // missing）則失效舊快照：這個 workspace 已不是 repo，下次切回不得
            // hydrate 舊 repo 殘影（沿用「非 repo 不殘留」語意）。
            if (environment.status === "ready") {
                liveSnapshotKey = workspacePath
                writeSnapshot(workspacePath, { environment, status, branches, at: Date.now() })
            } else {
                liveSnapshotKey = null
                snapshots.delete(workspacePath)
            }
            // 抑制期內被擋下的 refresh 補跑一次：bootstrap 快照可能在該 fs 變更
            // 之前取樣，不補跑會漏掉窗口內落地的變更（debounce 照常吸震）。
            const rerunSuppressed = refreshAfterDetect
            refreshAfterDetect = false
            if (rerunSuppressed && environment.status === "ready") {
                void get().refresh()
            }
        } catch (e) {
            // 過期 reject 同樣丟棄——舊 workspace 的失敗不得在新 workspace 上冒 lastError。
            if (seq !== detectSeq) return
            detectInFlight = false
            refreshAfterDetect = false
            set({ lastError: String(e) })
        }
    },

    refresh: (paths) => {
        // Non-ready environments (fs/focus-driven refreshes before detect, or a
        // non-repo workspace) must not touch git or write lastError (background
        // noise rule, m2).
        if (get().environment?.status !== "ready") return Promise.resolve()
        // detect 在飛期間 Rust 端 repo state 歸屬不明（可能仍是前一個 repo）：
        // 不發起 fetch，記一筆待 bootstrap 落地後補跑（見 detectInFlight 註解）。
        if (detectInFlight) {
            refreshAfterDetect = true
            return Promise.resolve()
        }
        if (inflight) {
            // Within the debounce window (timer still pending) widen the not-yet-
            // started request. Once the fetch is running, contribute this caller's
            // path scope to one post-flight rerun. Full refresh dominates.
            if (timer) {
                scheduledScope = mergeRefreshPathScope(scheduledScope, paths)
            } else {
                pendingRerunScope = mergeRefreshPathScope(pendingRerunScope, paths)
            }
            return inflight
        }
        scheduledScope = paths === undefined ? undefined : [...paths]
        const generation = ++refreshFlightGen
        let settled = false
        const promise = new Promise<void>((resolve) => {
            const settle = () => {
                if (settled) return
                settled = true
                resolve()
            }
            settleInflight = settle
            if (timer) clearTimeout(timer)
            timer = setTimeout(async () => {
                try {
                    if (generation !== refreshFlightGen) return
                    timer = null
                    // Re-check readiness at execution time: the environment can flip to
                    // non-ready (notARepo/missing) during the debounce window, and
                    // running the fetch then would write lastError — the very background
                    // noise m2 removes (F2). Abandon the fetch and drop any pending rerun
                    // so no stale flag lingers.
                    const rootAtFetch = readyRoot(get().environment)
                    // detect 在飛（debounce 窗口內開始了 workspace 切換）同樣放棄本次
                    // fetch——Rust 端可能回「另一個 repo」的 status；記一筆待補跑。
                    if (rootAtFetch === null || detectInFlight) {
                        if (generation !== refreshFlightGen) return
                        if (detectInFlight) refreshAfterDetect = true
                        scheduledScope = null
                        pendingRerunScope = null
                        inflight = null
                        if (settleInflight === settle) settleInflight = null
                        return
                    }
                    const requestPaths = scheduledScope === null ? paths : scheduledScope
                    if (generation === refreshFlightGen) scheduledScope = null
                    const epochAtFetch = statusEpoch
                    try {
                        const status = await gitStatus(rootAtFetch, requestPaths)
                        if (generation !== refreshFlightGen) return
                        // Re-check after the await: the environment can flip to
                        // non-ready while the fetch is in flight (detect() switching to
                        // a non-repo workspace clears status and does not refresh), so a
                        // stale resolve would re-fill the just-cleared status. Discard it
                        // (F-1). ready→ready 切換同樣要擋：A 的慢 status 晚到時 B 也是
                        // ready，比對 root 才能丟棄跨 repo 的 stale resolve（#57 覆核）。
                        // detectInFlight 落地閘：resolve 期間切換開始的話，這筆 status
                        // 的 repo 歸屬不明（root guard 對 Rust state 時序盲視）→ 丟棄，
                        // bootstrap 落地後自帶新快照收斂。
                        // statusEpoch: a successful mutation (or landed detect) sampled
                        // after this request started makes the result stale even on the
                        // same root.
                        if (statusRequestIsCurrent(get().environment, rootAtFetch, epochAtFetch)) {
                            set((state) => ({ status, lastError: null, statusRevision: state.statusRevision + 1 }))
                            syncLiveSnapshot(get())
                        }
                    } catch (e) {
                        if (generation !== refreshFlightGen) return
                        // Same guard for a stale rejection — a failure from the old
                        // workspace or pre-mutation epoch must stay silent.
                        if (statusRequestIsCurrent(get().environment, rootAtFetch, epochAtFetch)) {
                            set({ lastError: String(e) })
                        }
                    }
                    if (generation !== refreshFlightGen) return
                    const rerunPaths = pendingRerunScope
                    pendingRerunScope = null
                    inflight = null
                    if (settleInflight === settle) settleInflight = null
                    if (rerunPaths !== null && get().environment?.status === "ready") {
                        // Include the scheduled rerun in this promise so coalesced
                        // callers (especially runOp after an epoch bump) wait for
                        // a current-epoch publish, not just the discarded request.
                        // Full-refresh callers win over any path-scoped pending set.
                        await get().refresh(rerunPaths)
                    }
                } finally {
                    settle()
                }
            }, REFRESH_DEBOUNCE_MS)
        })
        inflight = promise
        return promise
    },

    // Background refresh for checkRemote: updates `status` on success but keeps
    // failures silent — never writes lastError, and rethrows so checkRemote's
    // catch can set remotePaused. Deliberately bypasses the loud `refresh`
    // debounce/single-flight to avoid inheriting a merged loud caller's error
    // attribution (a foreground refresh in the same window would otherwise route
    // the failure into lastError, breaking the background silence rule).
    refreshQuiet: async (paths) => {
        // detect 在飛期間跳過（Rust 端 repo 歸屬不明；bootstrap 落地自帶新
        // status，背景 autofetch 下一輪再補）。
        if (detectInFlight) return
        // stale-resolve 丟棄（#57 覆核）：背景 autofetch 的 status 若跨越了
        // workspace 切換才 resolve，不得蓋掉新 workspace 的快照（refresh 同款
        // root 比對；錯誤照舊往上拋給 checkRemote 設 remotePaused）。
        const rootAtFetch = readyRoot(get().environment)
        if (!rootAtFetch || get().snapshotStale) return
        const epochAtFetch = statusEpoch
        try {
            const status = await gitStatus(rootAtFetch, paths)
            if (statusRequestIsCurrent(get().environment, rootAtFetch, epochAtFetch)) {
                set((state) => ({ status, lastError: null, statusRevision: state.statusRevision + 1 }))
                syncLiveSnapshot(get())
            }
        } catch (e) {
            // Rethrow only while the request is still current so checkRemote can
            // pause. A pre-mutation or cross-root failure must not become last writer.
            if (statusRequestIsCurrent(get().environment, rootAtFetch, epochAtFetch)) {
                throw e
            }
        }
    },

    loadBranches: async () => {
        // detect 在飛期間跳過（Rust 端 repo 歸屬不明；bootstrap 落地自帶
        // branches 首載快照，不需要這一趟）。
        if (detectInFlight) return
        // stale-resolve 丟棄（#57 覆核）：git:state-changed 觸發的 loadBranches
        // 可能在 workspace 切換（detect 的 bootstrap 快照已落地）之後才 resolve，
        // 舊 workspace 的 branches 不得蓋掉新 workspace 的首載快照；stale 的
        // reject 同樣不得在新 workspace 冒 lastError（refresh F-1 同款語意）。
        const rootAtFetch = readyRoot(get().environment)
        if (!rootAtFetch || get().snapshotStale) return
        const epochAtFetch = branchEpoch
        const seqAtFetch = ++branchRequestSeq
        try {
            const branches = await gitBranches(rootAtFetch)
            if (branchRequestIsCurrent(get().environment, rootAtFetch, epochAtFetch, seqAtFetch)) {
                set({ branches, lastError: null })
                syncLiveSnapshot(get())
            }
        } catch (e) {
            if (branchRequestIsCurrent(get().environment, rootAtFetch, epochAtFetch, seqAtFetch)) {
                set({ lastError: String(e) })
            }
        }
    },

    runOp: async (name, fn, options) => {
        if (gitMutationsBlocked(get())) return false
        const capturedRoot = readyRoot(get().environment)
        if (!capturedRoot) return false
        set({ busy: name })
        // Single-point Console wiring: every runOp completion (success and
        // failure) records one entry here. The IPC layer returns no stdout, so
        // success shows "Done" and failure shows the error message (brief B1 —
        // do not touch the Rust side just to surface stdout).
        const cmd = consoleCmdLabel(name, options)
        try {
            await fn()
            const afterSuccess = get()
            if (
                !detectInFlight
                && !afterSuccess.snapshotStale
                && readyRoot(afterSuccess.environment) === capturedRoot
            ) {
                // Invalidate every gitStatus/gitBranches sampled before this mutation.
                bumpMutationEpochs()
                if (options?.afterMutationBeforeRefresh) {
                    try {
                        await options.afterMutationBeforeRefresh()
                    } catch {
                        // Mutation already succeeded; hook failure must not invert runOp.
                    }
                }
            }
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
        const before = get()
        if (
            before.busy
            || before.remotePaused
            || detectInFlight
            || before.snapshotStale
            || before.environment?.status !== "ready"
        ) return
        const root = before.environment.root
        const { mode } = before.remoteCheck
        if (mode === "off") return
        try {
            if (mode === "probe") {
                const remoteIncoming = await gitRemoteProbe(root)
                const current = get()
                if (!detectInFlight && !current.snapshotStale && readyRoot(current.environment) === root) {
                    set({ remoteIncoming })
                }
            } else {
                await gitFetch(root, true)
                const current = get()
                if (!detectInFlight && !current.snapshotStale && readyRoot(current.environment) === root) {
                    const epochAtQuiet = statusEpoch
                    try {
                        await current.refreshQuiet()
                    } catch {
                        const after = get()
                        if (
                            !detectInFlight
                            && !after.snapshotStale
                            && readyRoot(after.environment) === root
                            && statusEpoch === epochAtQuiet
                        ) {
                            set({ remotePaused: true })
                        }
                        return
                    }
                }
            }
        } catch {
            const current = get()
            if (!detectInFlight && !current.snapshotStale && readyRoot(current.environment) === root) {
                set({ remotePaused: true })
            }
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

const EMPTY_CHANGED_PATHS: ReadonlySet<string> = new Set()
const changedPathsByStatus = new WeakMap<GitStatus, ReadonlySet<string>>()

export function changedPathSet(status: GitStatus | null): ReadonlySet<string> {
    if (!status) return EMPTY_CHANGED_PATHS
    const cached = changedPathsByStatus.get(status)
    if (cached) return cached

    const set = new Set<string>()
    for (const entry of status.unstaged) set.add(entry.path)
    for (const path of status.untracked) set.add(path)
    for (const entry of status.conflicted) set.add(entry.path)
    changedPathsByStatus.set(status, set)
    return set
}
