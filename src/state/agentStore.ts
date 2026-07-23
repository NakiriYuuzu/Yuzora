import { create } from "zustand"

import type {
    AgentConnection,
    AgentAuthMethod,
    ElicitationRequest,
    ElicitationResponsePayload,
    PromptBlock,
    SessionConfigOption,
    SessionConfigValue,
    SessionInfoUpdate,
    SessionMeta,
    SlashCommand,
    StopReason,
    UsageInfo
} from "@/agent/acpConnection"
import { isAgentAuthRequiredError, reduceSessionUpdate } from "@/agent/acpConnection"
import { newEntryId, type BlockEntry, type TranscriptAction, type TranscriptEntry } from "@/agent/acpTypes"
import { rememberAgentVersion } from "@/agent/agentVersions"
import {
    loadAgentSettings,
    rememberLastUsedCuratedAgent,
    resolveAgentCommand,
    resolveAgentCommandRoute
} from "@/app/workbench/settingsStorage"
import type { AgentCommandIdentity, AgentId, AgentPreset } from "@/lib/agentPresets"
import i18n from "@/lib/i18n"
import { isAbsolutePath } from "@/lib/paths"
import { normalizeWorkspacePath } from "@/state/recentWorkspaces"
import {
    loadSessionIndex,
    removeSessionIndexEntry,
    touchSessionIndexEntry,
    upsertSessionIndexEntry,
    type SessionIndexEntry
} from "@/state/sessionIndexStorage"
import { useTerminalStore, type TerminalSessionMeta } from "@/state/terminalStore"
import { useUiStore } from "@/state/uiStore"

export type AgentTone = "idle" | "run" | "done" | "wait" | "fail"
export type AgentConnectionState = "idle" | "connecting" | "ready" | "error"
type StopBadgeKind = "refusal" | "truncated"
type PermissionResolver = (optionId: string) => void

interface StopBadge {
    kind: StopBadgeKind
    label: string
    stopReason: StopReason
}

export interface SessionState {
    title: string
    agentId?: AgentPreset
    /** Adapter version reported by ACP initialize.agentInfo. */
    agentVersion?: string
    /** Adapter name from initialize.agentInfo（"pi-acp"／"yuzora-pi-acp"）——P5 runtime badge。 */
    agentName?: string
    /** SHA-256 identity for a custom command; the raw command is never stored. */
    customCommandFingerprint?: string
    agentLabel: string
    model: string | null
    tone: AgentTone
    transcript: TranscriptEntry[]
    availableCommands: SlashCommand[]
    mode?: string
    stopReason: StopReason | null
    stopBadge: StopBadge | null
    error: string | null
    queueDepth: number | null
    running: boolean | null
    pendingTurn: boolean
    metadataTitle: boolean
    agentTitle?: string
    sessionAlias?: string | null
    derivedTitle?: string
    cwd: string | null
    infoBanner?: string | null
    usage?: UsageInfo
    /** perm BlockEntry id → 已選 optionId（in-memory；restored replay 產生新 id，不持久化）。 */
    permissionOutcomes?: Record<string, string>
    /** Latest complete ACP config snapshot. Every update replaces this array. */
    configOptions?: SessionConfigOption[]
    /** Monotonic per-session authoritative config generation. */
    configRevision?: number
    /** The one setter currently in flight for this session, if any. */
    configRequest?: PendingSessionConfigRequest | null
    /** Last setter failure; rendered next to the composer controls and cleared on retry/update. */
    configError?: string | null
    // true 只在「由 Session Index hydrate 出來、尚未在本次 process 生命週期內
    // 成功 load/續聊過」的 session 上——continueSession 靠這個旗標決定要不要
    // 嘗試 session/load replay，而非僅呼叫 selectSession。
    restored?: boolean
    // Visible empty session, whether auto-created or explicitly requested. It
    // is promoted (and indexed) when the first prompt begins; a later explicit
    // session/new may replace it beforehand.
    ephemeral?: boolean
}

interface PendingSessionConfigRequest {
    token: number
    configId: string
    value: SessionConfigValue
}

export interface ConfirmRemoveSessionRequest {
    sessionId: string
    resolve: (confirmed: boolean) => void
}

export interface AuthRequiredState {
    cwd: string
    sessionId: string | null
    authMethods: AgentAuthMethod[]
    message: string
    agentId?: AgentId | null
    agentCommand?: string
    agentIdentity?: AgentCommandIdentity
}

export interface PendingPermission {
    // 對應 transcript 內 perm BlockEntry 的 stable id：答覆時以此記錄 outcome。
    entryId: string
    request: {
        text: string
        actions: TranscriptAction[]
        meta?: string
    }
    choose: PermissionResolver
}

// ACP form elicitation（P3）：獨立於 pendingPermissions 的通道（response 形狀與
// 語意不同）。每 session 一個 queue——後到的請求排隊而非覆蓋，避免 orphan 掉前
// 一個 resolver（永不 resolve 的 wire promise 正是 ask-question 卡死的形狀）。
export interface PendingElicitation {
    id: string
    request: ElicitationRequest
    respond: (response: ElicitationResponsePayload) => void
}

export type AgentSessionMeta = SessionMeta & {
    name?: string
    title?: string
    agentLabel?: string
    model?: string
}

export interface AgentStoreState {
    sessions: Map<string, SessionState>
    pendingPermissions: Map<string, PendingPermission>
    pendingElicitations: Map<string, PendingElicitation[]>
    activeSessionId: string | null
    connectionState: AgentConnectionState
    connectionError: string | null
    connection: AgentConnection | null
    authRequired: AuthRequiredState | null
    pendingNewSession: boolean
    hydratedWorkspaceCwds: Set<string>
    composerFocusRequest: { sessionId: string; token: number } | null
    // Sessions context menu（Phase 5）通道：menu executor 觸發 inline rename／移除
    // 確認，UI（AgentNavContent）讀取並渲染。
    renamingSessionId: string | null
    confirmRemoveRequest: ConfirmRemoveSessionRequest | null
    setConnection: (connection: AgentConnection | null) => void
    newSession: (cwd: string, agentId?: AgentId) => Promise<string>
    markWorkspaceHydrated: (cwd: string) => void
    activateDraftWorkspace: (cwd: string) => void
    ensureDraftSession: (cwd: string, agentId: AgentId) => Promise<string | null>
    beginTerminalLogin: () => void
    retryAfterLogin: () => Promise<string>
    loadSessions: (cwd: string) => Promise<void>
    sendPrompt: (cwd: string, prompt: string | PromptBlock[]) => Promise<StopReason>
    cancel: (sessionId?: string) => Promise<boolean>
    selectSession: (sessionId: string) => void
    // Session Index 持久化重啟後的入口：把索引條目建成 idle、空 transcript 的
    // restored sessions（不動 activeSessionId）。
    hydrateRestoredSessions: (entries: SessionIndexEntry[]) => void
    // Nav row 點擊的統一入口。Live session（restored 非 true）維持既有行為，只
    // selectSession；restored session 依 loadSession capability 嘗試 replay，
    // 不支援或失敗則降級為提示 block + 開新對話 action。
    continueSession: (sessionId: string) => Promise<void>
    // Sessions context menu 的三個 domain action：alias（trim；空字串清除還原）、
    // remove（先 cancel 進行中的 turn，再從 sessions／Session Index 移除，被移除者
    // 為 active 時切換至同 cwd 最近 lastActive 的其他 session）、rename 觸發通道。
    setSessionAlias: (sessionId: string, alias: string) => void
    removeSession: (sessionId: string) => Promise<boolean>
    beginRenameSession: (sessionId: string) => void
    endRenameSession: () => void
    // 移除確認 modal 的 imperative gate（沿 confirmDialogStore.requestUnsavedDecision
    // 的 promise 模式）：menu executor await 這支，UI 渲染 modal 並在使用者按下按鈕時
    // respond。
    requestRemoveSessionConfirm: (sessionId: string) => Promise<boolean>
    respondRemoveSessionConfirm: (confirmed: boolean) => void
    setAvailableCommands: (sessionId: string, commands: SlashCommand[]) => void
    filterCommands: (prefix: string, sessionId?: string) => SlashCommand[]
    appendUpdate: (sessionId: string, update: Record<string, unknown>) => void
    replaceTranscript: (sessionId: string, transcript: TranscriptEntry[]) => void
    onSessionInfo: (sessionId: string, info: SessionInfoUpdate) => void
    replaceConfigOptions: (sessionId: string, configOptions: SessionConfigOption[]) => void
    setSessionConfigOption: (
        sessionId: string,
        configId: string,
        value: SessionConfigValue
    ) => Promise<SessionConfigOption[]>
    setUsage: (sessionId: string, usage: UsageInfo) => void
    applyAgentTitle: (sessionId: string, title: string) => void
    onPermissionRequest: (sessionId: string, block: BlockEntry, choose: PermissionResolver) => void
    respondPermission: (sessionId: string, optionId: string) => void
    onElicitationRequest: (
        sessionId: string,
        request: ElicitationRequest,
        respond: (response: ElicitationResponsePayload) => void
    ) => void
    respondElicitation: (sessionId: string, elicitationId: string, response: ElicitationResponsePayload) => void
    markConnectionError: (sessionId: string, error: Error) => void
    upsertSessionMeta: (meta: AgentSessionMeta) => void
    reset: () => void
}

interface CreateAgentStoreOptions {
    connection?: AgentConnection | null
}

const DEFAULT_SESSION_TITLE = "New session"
const DEFAULT_AGENT_LABEL = "Agent"
const TITLE_LIMIT = 48
const TERMINAL_SETTINGS_STORAGE_KEY = "yuzora:terminal-settings"
const DEFAULT_TERMINAL_COLS = 80
const DEFAULT_TERMINAL_ROWS = 24
const CANCELLED_META = JSON.stringify({ stopReason: "cancelled", interrupted: true })
// elicitation UI 條目的單調流水號（in-memory；wire promise 的生命週期在連線層）。
let elicitationSeq = 0
// F4：continueSession 的 per-session in-flight guard——重複點擊（如雙擊）restored
// row 時，第二次呼叫直接 return，避免同一 session 併發觸發兩次 loadSession。
const continueSessionInFlight = new Set<string>()

export const agentInitialState = {
    sessions: new Map<string, SessionState>(),
    pendingPermissions: new Map<string, PendingPermission>(),
    pendingElicitations: new Map<string, PendingElicitation[]>(),
    activeSessionId: null as string | null,
    connectionState: "idle" as AgentConnectionState,
    connectionError: null as string | null,
    connection: null as AgentConnection | null,
    authRequired: null as AuthRequiredState | null,
    pendingNewSession: false,
    hydratedWorkspaceCwds: new Set<string>(),
    composerFocusRequest: null as { sessionId: string; token: number } | null,
    renamingSessionId: null as string | null,
    confirmRemoveRequest: null as ConfirmRemoveSessionRequest | null
}

export function createAgentStore(options: CreateAgentStoreOptions = {}) {
    const draftSessionInFlight = new Map<string, Promise<string | null>>()
    // Runtime callbacks may already be queued when a session is explicitly
    // removed or an unused draft is replaced. Keep an app-lifetime tombstone
    // so those callbacks cannot recreate the session through ensureSession.
    // Only an authoritative session/new result (or a full store reset) may
    // clear an id for legitimate reuse; a stale list result must not do so.
    const droppedSessionIds = new Set<string>()
    let draftWorkspaceCwd: string | null = null
    let draftWorkspaceGeneration = 0
    let nextConfigRequestToken = 0

    return create<AgentStoreState>()((set, get) => ({
        ...agentInitialState,
        connection: options.connection ?? null,
        connectionState: options.connection ? "ready" : "idle",

        setConnection: (connection) => set({
            connection,
            connectionState: connection ? "ready" : "idle",
            connectionError: null,
            authRequired: null
        }),

        markWorkspaceHydrated: (cwd) => {
            requireAbsoluteCwd(cwd)
            set((state) => {
                const hydratedWorkspaceCwds = new Set(state.hydratedWorkspaceCwds)
                hydratedWorkspaceCwds.add(normalizeWorkspacePath(cwd))
                return { hydratedWorkspaceCwds }
            })
        },

        activateDraftWorkspace: (cwd) => {
            requireAbsoluteCwd(cwd)
            const normalized = normalizeWorkspacePath(cwd)
            if (draftWorkspaceCwd === normalized) return
            draftWorkspaceCwd = normalized
            draftWorkspaceGeneration += 1
        },

        ensureDraftSession: (cwd, agentId) => {
            requireAbsoluteCwd(cwd)
            const normalizedCwd = normalizeWorkspacePath(cwd)
            if (draftWorkspaceCwd === null) {
                draftWorkspaceCwd = normalizedCwd
                draftWorkspaceGeneration += 1
            }
            if (draftWorkspaceCwd !== normalizedCwd) return Promise.resolve(null)

            const route = resolveAgentCommandRoute(agentId)
            const key = draftSessionKey(cwd, route)
            if (key) {
                const existingRequest = draftSessionInFlight.get(key)
                if (existingRequest) return existingRequest
            }

            const existingSessionId = visibleSessionForCwd(get(), cwd)
            if (existingSessionId) {
                set({ activeSessionId: existingSessionId })
                return Promise.resolve(existingSessionId)
            }
            if (!key) return Promise.resolve(null)

            const generation = draftWorkspaceGeneration
            const connection = requireConnection(get())
            const request = (async (): Promise<string | null> => {
                try {
                    const result = await connection.newSession(cwd, agentId)
                    const identity = result.agentIdentity ?? identityFromRoute(route)
                    const sessionId = result.sessionId
                    if (!identity.trustedAgentId || identity.commandMode === "custom") {
                        droppedSessionIds.add(sessionId)
                        discardSessionState(set, sessionId)
                        connection.dropSession?.(sessionId)
                        void connection.disposePrepared?.(cwd).catch(() => undefined)
                        return null
                    }
                    const existingAfterCreate = visibleSessionForCwd(get(), cwd)
                    const stale = draftWorkspaceGeneration !== generation
                        || draftWorkspaceCwd !== normalizedCwd
                    if (stale || existingAfterCreate) {
                        droppedSessionIds.add(sessionId)
                        discardSessionState(set, sessionId)
                        connection.dropSession?.(sessionId)
                        // If this was the final owner of a stale prepared sub, let
                        // the normal no-owner gate reclaim it. Other owners survive.
                        void connection.disposePrepared?.(cwd).catch(() => undefined)
                        return existingAfterCreate
                    }

                    const resolvedPreset: AgentPreset = identity.trustedAgentId ?? "custom"
                    let installed = false
                    set((state) => {
                        // Re-check inside the atomic update so an explicit New
                        // session that won the same tick cannot be overwritten.
                        const winner = visibleSessionForCwd(state, cwd)
                        if (draftWorkspaceGeneration !== generation
                            || draftWorkspaceCwd !== normalizedCwd
                            || winner) {
                            return {}
                        }
                        droppedSessionIds.delete(sessionId)
                        const sessions = new Map(state.sessions)
                        sessions.set(sessionId, ensureSession(sessions, sessionId, {
                            cwd,
                            agentId: resolvedPreset,
                            agentVersion: result.agentVersion,
                            infoBanner: result.startupInfo,
                            customCommandFingerprint: result.customCommandFingerprint,
                            ephemeral: true,
                            ...authoritativeConfigPatch(
                                sessions.get(sessionId),
                                result.configOptions ?? []
                            )
                        }))
                        installed = true
                        return {
                            sessions,
                            activeSessionId: sessionId,
                            connectionState: "ready",
                            connectionError: null,
                            authRequired: null
                        }
                    })
                    if (!installed) {
                        droppedSessionIds.add(sessionId)
                        discardSessionState(set, sessionId)
                        connection.dropSession?.(sessionId)
                        void connection.disposePrepared?.(cwd).catch(() => undefined)
                        return visibleSessionForCwd(get(), cwd)
                    }
                    rememberLastUsedCuratedAgent(identity)
                    if (identity.trustedAgentId) {
                        rememberAgentVersion(identity.trustedAgentId, result.agentVersion)
                    }
                    return sessionId
                } catch (error) {
                    if (draftWorkspaceGeneration === generation
                        && draftWorkspaceCwd === normalizedCwd
                        && !visibleSessionForCwd(get(), cwd)) {
                        const err = error instanceof Error ? error : new Error(String(error))
                        const authRequired = authRequiredFromError(error, cwd, null, agentId)
                        set({
                            connectionState: "error",
                            connectionError: err.message,
                            ...(authRequired ? { authRequired } : {})
                        })
                    }
                    throw error
                }
            })()
            draftSessionInFlight.set(key, request)
            const clear = () => {
                if (draftSessionInFlight.get(key) === request) draftSessionInFlight.delete(key)
            }
            void request.then(clear, clear)
            return request
        },

        newSession: async (cwd, agentId) => {
            const connection = requireConnection(get())
            requireAbsoluteCwd(cwd)
            const preset: AgentPreset = agentId ?? defaultAgentPreset()
            const requestedRoute = resolveAgentCommandRoute(agentId)
            const pendingDraftKey = draftSessionKey(cwd, requestedRoute)
            const pendingDraft = pendingDraftKey
                ? draftSessionInFlight.get(pendingDraftKey)
                : undefined
            set({ connectionState: "connecting", connectionError: null, pendingNewSession: true })
            try {
                // An explicit New arriving while the matching auto-draft is
                // still creating claims that same protocol session/new. The
                // result remains the one unused ephemeral draft; a later New,
                // after this registry entry settles, still creates afresh.
                const claimedSessionId = pendingDraft ? await pendingDraft : null
                if (claimedSessionId && get().sessions.has(claimedSessionId)) {
                    set({
                        activeSessionId: claimedSessionId,
                        connectionState: "ready",
                        connectionError: null,
                        authRequired: null,
                        pendingNewSession: false
                    })
                    return claimedSessionId
                }
                // 省略 agentId 時維持單參數呼叫，不讓既有一參數呼叫點的行為改變。
                const result = agentId === undefined
                    ? await connection.newSession(cwd)
                    : await connection.newSession(cwd, agentId)
                const { sessionId, startupInfo, agentVersion, agentName, customCommandFingerprint, agentIdentity } = result
                const resolvedPreset: AgentPreset = agentIdentity
                    ? agentIdentity.trustedAgentId ?? "custom"
                    : preset
                droppedSessionIds.delete(sessionId)
                const replacedDraftIds = unusedDraftSessionIdsForCwd(get().sessions, cwd, sessionId)
                for (const draftId of replacedDraftIds) {
                    droppedSessionIds.add(draftId)
                    connection.dropSession?.(draftId)
                    removeSessionIndexEntry(draftId)
                }
                if (replacedDraftIds.length > 0) {
                    void connection.disposePrepared?.(cwd).catch(() => undefined)
                }
                set((state) => {
                    const sessions = new Map(state.sessions)
                    for (const draftId of replacedDraftIds) sessions.delete(draftId)
                    sessions.set(sessionId, ensureSession(sessions, sessionId, {
                        cwd,
                        agentId: resolvedPreset,
                        agentVersion,
                        agentName,
                        infoBanner: startupInfo,
                        customCommandFingerprint,
                        ephemeral: true,
                        ...authoritativeConfigPatch(
                            sessions.get(sessionId),
                            result.configOptions ?? []
                        )
                    }))
                    return {
                        sessions,
                        activeSessionId: sessionId,
                        connectionState: "ready",
                        connectionError: null,
                        authRequired: null,
                        pendingNewSession: false
                    }
                })
                const identity = agentIdentity ?? identityFromRoute(requestedRoute)
                rememberLastUsedCuratedAgent(identity)
                if (identity.trustedAgentId) rememberAgentVersion(identity.trustedAgentId, agentVersion)
                // Explicit New session is still an unused visible draft until
                // its first prompt starts. Remove a same-id legacy entry
                // defensively instead of restoring an empty draft next launch.
                removeSessionIndexEntry(sessionId)
                return sessionId
            } catch (error) {
                const err = error instanceof Error ? error : new Error(String(error))
                const authRequired = authRequiredFromError(error, cwd, null, agentId ?? null)
                set({
                    connectionState: "error",
                    connectionError: err.message,
                    pendingNewSession: false,
                    ...(authRequired ? { authRequired } : {})
                })
                throw error
            }
        },

        beginTerminalLogin: () => {
            const authRequired = get().authRequired
            if (!authRequired) return
            const method = terminalAuthMethod(authRequired.authMethods)
            if (!method) return
            const meta = terminalLoginSessionMeta(
                authRequired.cwd,
                method,
                authRequired.agentId,
                authRequired.agentCommand
            )
            useTerminalStore.getState().addSession(authRequired.cwd, meta)
            if (!useUiStore.getState().terminalOpen) {
                useUiStore.getState().toggleTerminal()
            }
        },

        retryAfterLogin: async () => {
            const authRequired = get().authRequired
            if (!authRequired) throw new Error("No pending agent authentication")
            return get().newSession(authRequired.cwd, authRequired.agentId ?? undefined)
        },

        loadSessions: async (cwd) => {
            const connection = requireConnection(get())
            set({ connectionState: "connecting", connectionError: null })
            try {
                const metas = await connection.listSessions(cwd)
                set((state) => {
                    const sessions = new Map(state.sessions)
                    for (const meta of metas) {
                        if (droppedSessionIds.has(meta.id)) continue
                        sessions.set(meta.id, ensureSession(sessions, meta.id, sessionPatchFromMeta(meta)))
                    }
                    return { sessions, connectionState: "ready", connectionError: null }
                })
            } catch (error) {
                const err = error instanceof Error ? error : new Error(String(error))
                set({ connectionState: "error", connectionError: err.message })
                throw error
            }
        },

        sendPrompt: async (cwd, prompt) => {
            const connection = requireConnection(get())
            requireAbsoluteCwd(cwd)
            const blocks = promptBlocks(prompt)
            const promptTitle = titleFromPrompt(blocks)
            let sessionId = selectActiveSessionForCwd(get(), cwd)
            const wasImplicitCreate = !sessionId
            let implicitPreset = wasImplicitCreate ? defaultAgentPreset() : undefined
            const existingPreset = sessionId ? get().sessions.get(sessionId)?.agentId : undefined
            set({ connectionState: "connecting", connectionError: null })
            let implicitBanner: string | null = null
            let implicitAgentVersion: string | undefined
            let implicitAgentName: string | undefined
            let implicitCustomCommandFingerprint: string | undefined
            let implicitConfigOptions: SessionConfigOption[] = []
            const promotedDraft = sessionId ? get().sessions.get(sessionId)?.ephemeral === true : false
            try {
                if (!sessionId) {
                    const created = await connection.newSession(cwd)
                    sessionId = created.sessionId
                    droppedSessionIds.delete(sessionId)
                    implicitBanner = created.startupInfo
                    implicitAgentVersion = created.agentVersion
                    implicitAgentName = created.agentName
                    implicitCustomCommandFingerprint = created.customCommandFingerprint
                    implicitConfigOptions = created.configOptions ?? []
                    const identity = created.agentIdentity ?? identityFromRoute(resolveAgentCommandRoute())
                    rememberLastUsedCuratedAgent(identity)
                    if (identity.trustedAgentId) {
                        rememberAgentVersion(identity.trustedAgentId, created.agentVersion)
                    }
                    if (created.agentIdentity) {
                        implicitPreset = created.agentIdentity.trustedAgentId ?? "custom"
                    }
                }
                const activeSessionId = sessionId
                set((state) => {
                    const sessions = new Map(state.sessions)
                    const patch = wasImplicitCreate
                        ? {
                            cwd,
                            agentId: implicitPreset,
                            agentVersion: implicitAgentVersion,
                            agentName: implicitAgentName,
                            infoBanner: implicitBanner,
                            customCommandFingerprint: implicitCustomCommandFingerprint,
                            ...authoritativeConfigPatch(
                                sessions.get(activeSessionId),
                                implicitConfigOptions
                            )
                        }
                        : { cwd, ...(promotedDraft ? { ephemeral: false } : {}) }
                    const current = ensureSession(sessions, activeSessionId, patch)
                    sessions.set(activeSessionId, beginTurn(current, promptTitle, blocks))
                    return { sessions, activeSessionId, connectionState: "ready" }
                })
                if (wasImplicitCreate || promotedDraft) {
                    ensureSessionIndexEntry(activeSessionId, get().sessions.get(activeSessionId))
                }
                const stopReason = await connection.prompt(activeSessionId, blocks)
                set((state) => {
                    const pendingPermissions = new Map(state.pendingPermissions)
                    pendingPermissions.delete(activeSessionId)
                    const pendingElicitations = new Map(state.pendingElicitations)
                    pendingElicitations.delete(activeSessionId)
                    // session 可能在這輪 prompt in-flight 期間被 removeSession 移除——
                    // 不要用 ensureSession 讓它以 ghost 姿態復活。
                    if (!state.sessions.has(activeSessionId)) {
                        return { pendingPermissions, pendingElicitations, connectionState: "ready" }
                    }
                    const sessions = new Map(state.sessions)
                    const current = ensureSession(sessions, activeSessionId, { cwd })
                    sessions.set(activeSessionId, applyStopReason(current, stopReason))
                    return { sessions, pendingPermissions, pendingElicitations, connectionState: "ready" }
                })
                // 每輪 turn 結束（sendPrompt resolve）都 touch lastActiveAt，讓 Session
                // Index 的最近活躍排序反映真實使用時間。
                syncSessionIndex(activeSessionId, get().sessions.get(activeSessionId))
                return stopReason
            } catch (error) {
                const err = error instanceof Error ? error : new Error(String(error))
                const attemptedAgentId: AgentId | null =
                    existingPreset && existingPreset !== "custom" ? existingPreset : null
                const authRequired = authRequiredFromError(error, cwd, sessionId ?? null, attemptedAgentId)
                set((state) => {
                    if (!sessionId) {
                        return {
                            connectionState: "error",
                            connectionError: err.message,
                            ...(authRequired ? { authRequired } : {})
                        }
                    }
                    const pendingPermissions = new Map(state.pendingPermissions)
                    pendingPermissions.delete(sessionId)
                    const pendingElicitations = new Map(state.pendingElicitations)
                    pendingElicitations.delete(sessionId)
                    // 同上：session 可能在 in-flight 期間被移除，跳過 ensureSession 重建。
                    if (!state.sessions.has(sessionId)) {
                        return {
                            pendingPermissions,
                            pendingElicitations,
                            connectionState: "error",
                            connectionError: err.message,
                            ...(authRequired ? { authRequired } : {})
                        }
                    }
                    const sessions = new Map(state.sessions)
                    sessions.set(sessionId, failSession(ensureSession(sessions, sessionId, { cwd }), err))
                    return {
                        sessions,
                        pendingPermissions,
                        pendingElicitations,
                        connectionState: "error",
                        connectionError: err.message,
                        ...(authRequired ? { authRequired } : {})
                    }
                })
                throw error
            }
        },

        cancel: async (sessionId = get().activeSessionId ?? undefined) => {
            if (!sessionId) return false
            const target = get().sessions.get(sessionId)
            if (!target) return false
            const connection = get().connection
            if (!connection) {
                const error = new Error("Agent connection is not available")
                set({ connectionError: error.message })
                throw error
            }
            try {
                await connection.cancel(sessionId)
            } catch (error) {
                set({ connectionError: error instanceof Error ? error.message : String(error) })
                throw error
            }
            set((state) => {
                const sessions = new Map(state.sessions)
                const pendingPermissions = new Map(state.pendingPermissions)
                pendingPermissions.delete(sessionId)
                const pendingElicitations = new Map(state.pendingElicitations)
                pendingElicitations.delete(sessionId)
                const current = sessions.get(sessionId)
                if (!current) return { pendingPermissions, pendingElicitations, connectionError: null }
                // The turn may have completed naturally while cancel was in flight.
                // Preserve that terminal state instead of overwriting it as cancelled.
                if ((target.pendingTurn || target.running) && !current.pendingTurn && !current.running) {
                    return { pendingPermissions, pendingElicitations, connectionError: null }
                }
                sessions.set(sessionId, applyStopReason(current, "cancelled"))
                return { sessions, pendingPermissions, pendingElicitations, connectionError: null }
            })
            return get().sessions.has(sessionId)
        },

        selectSession: (sessionId) => {
            set((state) => {
                const sessions = new Map(state.sessions)
                sessions.set(sessionId, ensureSession(sessions, sessionId))
                return { sessions, activeSessionId: sessionId }
            })
        },

        hydrateRestoredSessions: (entries) => {
            set((state) => {
                const sessions = new Map(state.sessions)
                for (const entry of entries) {
                    // 已存在（如本次 process 已建立／已 hydrate 過）就不覆蓋，避免踩掉
                    // 進行中的 live session。
                    if (sessions.has(entry.sessionId) || droppedSessionIds.has(entry.sessionId)) continue
                    const patch: Partial<SessionState> = {
                        cwd: entry.cwd,
                        agentId: toAgentPreset(entry.agentId),
                        customCommandFingerprint: entry.customCommandFingerprint,
                        agentTitle: entry.agentTitle,
                        sessionAlias: entry.sessionAlias,
                        derivedTitle: entry.derivedTitle,
                        restored: true
                    }
                    sessions.set(entry.sessionId, ensureSession(sessions, entry.sessionId, patch))
                }
                return { sessions }
            })
        },

        continueSession: async (sessionId) => {
            // Live session（restored 非 true）維持既有點擊行為：只 selectSession，
            // 沒有 load 開銷。
            get().selectSession(sessionId)
            set((state) => ({
                composerFocusRequest: {
                    sessionId,
                    token: (state.composerFocusRequest?.token ?? 0) + 1
                }
            }))
            const session = get().sessions.get(sessionId)
            if (!session?.restored) return
            const cwd = session.cwd
            const connection = get().connection
            if (!cwd || !connection) return
            // F4：同一 session 的 load 已在進行中（如雙擊觸發兩次 continueSession）
            // 就直接跳過，避免併發重複 loadSession／transcript 重複。
            if (continueSessionInFlight.has(sessionId)) return
            // agentId undefined 時 commandFor 回退目前全域 settings；custom preset 沒有
            // 對應的 AgentId，同樣視為未指定。
            const agentId = session.agentId && session.agentId !== "custom" ? session.agentId : undefined
            const customCommandFingerprint = session.agentId === "custom"
                ? session.customCommandFingerprint
                : undefined
            // v1 entries created before custom routing identity existed cannot be
            // safely replayed: using today's global custom command could spawn a
            // different agent for the old session id.
            if (session.agentId === "custom" && !customCommandFingerprint) {
                patchSessionWith(set, sessionId, (current) =>
                    degradeContinueSession(current, cwd, session.agentId))
                return
            }
            continueSessionInFlight.add(sessionId)

            patchSession(set, sessionId, { tone: "run" })
            try {
                const supportsLoad = (await (customCommandFingerprint
                    ? connection.supportsLoadSession?.(cwd, undefined, customCommandFingerprint)
                    : connection.supportsLoadSession?.(cwd, agentId))) ?? false
                if (!supportsLoad) {
                    patchSessionWith(set, sessionId, (current) => degradeContinueSession(current, cwd, session.agentId))
                    return
                }
                // restored session 的 transcript 一律清空重建。Replay 期間也先撤掉
                // 舊的 trusted agentId，避免 available_commands 比 loadSession 回應更早
                // 抵達時，被 classifier 暫時當成 Codex/Pi skills。
                patchSession(set, sessionId, { transcript: [], agentId: undefined })
                const result = customCommandFingerprint
                    ? await connection.loadSession(sessionId, cwd, undefined, customCommandFingerprint)
                    : await connection.loadSession(sessionId, cwd, agentId)
                const agentIdentity = result && "agentIdentity" in result
                    ? result.agentIdentity
                    : undefined
                const authoritativeAgentId: AgentPreset | undefined = agentIdentity
                    ? agentIdentity.trustedAgentId ?? "custom"
                    : undefined
                // loadSession 回應若帶 startupInfo（沿 newSession 的 infoBanner 模式），
                // feature detection：沒有就不動 infoBanner。
                const startupInfo = result && "startupInfo" in result ? result.startupInfo : null
                const agentVersion = result && "agentVersion" in result
                    ? result.agentVersion
                    : undefined
                const agentName = result && "agentName" in result
                    ? result.agentName
                    : undefined
                patchSessionWith(set, sessionId, (current) => ({
                    ...current,
                    restored: false,
                    tone: "idle",
                    // Router 回傳的 route identity 才是 replay 實際啟動命令的 trust
                    // authority；缺少 identity 也不得沿用舊的 trusted preset。
                    agentId: authoritativeAgentId,
                    agentVersion,
                    agentName,
                    customCommandFingerprint: authoritativeAgentId === "custom"
                        ? current.customCommandFingerprint
                        : undefined,
                    ...(startupInfo ? { infoBanner: startupInfo } : {}),
                    ...(result && "configOptions" in result
                        ? authoritativeConfigPatch(current, result.configOptions ?? [])
                        : {})
                }))
                if (agentIdentity?.trustedAgentId) {
                    rememberAgentVersion(agentIdentity.trustedAgentId, agentVersion)
                }
                syncSessionIndex(sessionId, get().sessions.get(sessionId))
            } catch {
                patchSessionWith(set, sessionId, (current) => degradeContinueSession(current, cwd, session.agentId))
            } finally {
                continueSessionInFlight.delete(sessionId)
            }
        },

        setSessionAlias: (sessionId, alias) => {
            const trimmed = alias.trim()
            set((state) => {
                const current = state.sessions.get(sessionId)
                if (!current) return {}
                const sessions = new Map(state.sessions)
                const next = { ...current, sessionAlias: trimmed === "" ? null : trimmed }
                sessions.set(sessionId, { ...next, title: resolveSessionTitle(next) })
                return { sessions }
            })
            syncSessionIndex(sessionId, get().sessions.get(sessionId))
        },

        removeSession: async (sessionId) => {
            const target = get().sessions.get(sessionId)
            if (!target) return false
            if (target.pendingTurn || target.running) {
                const cancelled = await get().cancel(sessionId)
                if (!cancelled || !get().sessions.has(sessionId)) return false
            }
            removeSessionIndexEntry(sessionId)
            // F10：連線 runtime 內部（transcript／pending permission）也要跟著清掉，
            // 否則長駐 in-memory 累積。找不到對應 sub 時 dropSession 靜默略過。
            droppedSessionIds.add(sessionId)
            get().connection?.dropSession?.(sessionId)
            set((state) => {
                const sessions = new Map(state.sessions)
                const pendingPermissions = new Map(state.pendingPermissions)
                const pendingElicitations = new Map(state.pendingElicitations)
                sessions.delete(sessionId)
                pendingPermissions.delete(sessionId)
                pendingElicitations.delete(sessionId)
                const activeSessionId = state.activeSessionId === sessionId
                    ? nextActiveSessionForCwd(sessions, target.cwd, sessionId)
                    : state.activeSessionId
                return { sessions, pendingPermissions, pendingElicitations, activeSessionId }
            })
            return true
        },

        beginRenameSession: (sessionId) => {
            if (!get().sessions.has(sessionId)) return
            set({ renamingSessionId: sessionId })
        },

        endRenameSession: () => set({ renamingSessionId: null }),

        requestRemoveSessionConfirm: (sessionId) =>
            new Promise<boolean>((resolve) => {
                // 前一個尚未 respond 的請求（理論上不會發生——modal 是單例，menu 開新請求前
                // 前一個必已 resolve）以 false 收尾，避免 promise 洩漏。
                const prev = get().confirmRemoveRequest
                if (prev) prev.resolve(false)
                set({ confirmRemoveRequest: { sessionId, resolve } })
            }),

        respondRemoveSessionConfirm: (confirmed) => {
            const pending = get().confirmRemoveRequest
            if (!pending) return
            set({ confirmRemoveRequest: null })
            pending.resolve(confirmed)
        },

        setAvailableCommands: (sessionId, commands) => {
            set((state) => {
                if (droppedSessionIds.has(sessionId)) return {}
                const sessions = new Map(state.sessions)
                const current = ensureSession(sessions, sessionId)
                sessions.set(sessionId, { ...current, availableCommands: [...commands] })
                return { sessions }
            })
        },

        filterCommands: (prefix, sessionId = get().activeSessionId ?? undefined) => {
            if (!sessionId) return []
            const session = get().sessions.get(sessionId)
            if (!session) return []
            const query = commandPrefix(prefix)
            if (!query) return session.availableCommands
            return session.availableCommands.filter((command) =>
                command.name.toLowerCase().startsWith(query)
            )
        },

        appendUpdate: (sessionId, update) => {
            let derivedTitlePatched = false
            set((state) => {
                if (droppedSessionIds.has(sessionId)) return {}
                const sessions = new Map(state.sessions)
                const current = ensureSession(sessions, sessionId)
                const transcript = reduceSessionUpdate(current.transcript, update)
                const titlePatch = deriveTitle(current, transcript)
                derivedTitlePatched = titlePatch.derivedTitle !== undefined
                sessions.set(sessionId, {
                    ...current,
                    transcript,
                    mode: modeFromUpdate(update) ?? current.mode,
                    ...titlePatch
                })
                return { sessions }
            })
            if (derivedTitlePatched) syncSessionIndex(sessionId, get().sessions.get(sessionId))
        },

        replaceTranscript: (sessionId, transcript) => {
            let derivedTitlePatched = false
            set((state) => {
                // late onTranscript 可能在 removeSession 之後才送達——不要用 ensureSession
                // 讓已移除的 session 以 ghost 姿態復活。
                if (droppedSessionIds.has(sessionId) || !state.sessions.has(sessionId)) return {}
                const sessions = new Map(state.sessions)
                const current = ensureSession(sessions, sessionId)
                // P7：late onTranscript 可能在 cancel() 已 append CANCELLED_META 標記
                // 之後才送達，若直接覆蓋會把中斷標記抹掉。若目前 transcript 尾端是該
                // 標記、而 incoming 沒有，把它接回 incoming 尾端再存。
                const prevLast = current.transcript.at(-1)
                const marker = prevLast && "kind" in prevLast && prevLast.meta === CANCELLED_META ? prevLast : null
                const incoming = [...transcript]
                const next = marker && !incoming.some((entry) => "kind" in entry && entry.meta === CANCELLED_META)
                    ? [...incoming, marker]
                    : incoming
                const titlePatch = deriveTitle(current, next)
                derivedTitlePatched = titlePatch.derivedTitle !== undefined
                sessions.set(sessionId, {
                    ...current,
                    transcript: next,
                    ...titlePatch
                })
                return { sessions }
            })
            if (derivedTitlePatched) syncSessionIndex(sessionId, get().sessions.get(sessionId))
        },

        onSessionInfo: (sessionId, info) => {
            set((state) => {
                if (droppedSessionIds.has(sessionId)) return {}
                const sessions = new Map(state.sessions)
                const current = ensureSession(sessions, sessionId)
                let tone = current.tone
                if (info.running && tone !== "wait" && tone !== "fail") tone = "run"
                if (!info.running && current.pendingTurn && tone !== "fail") tone = "done"
                sessions.set(sessionId, {
                    ...current,
                    tone,
                    queueDepth: info.queueDepth,
                    running: info.running
                })
                return { sessions }
            })
        },

        replaceConfigOptions: (sessionId, configOptions) => {
            set((state) => {
                const current = state.sessions.get(sessionId)
                if (!current) return {}
                const sessions = new Map(state.sessions)
                sessions.set(sessionId, {
                    ...current,
                    ...authoritativeConfigPatch(current, configOptions)
                })
                return { sessions }
            })
        },

        setSessionConfigOption: async (sessionId, configId, value) => {
            const current = get().sessions.get(sessionId)
            if (!current) throw new Error(`Unknown session ${sessionId}`)
            // soak 回饋 #1/#5：turn 進行中允許改 config（pi 隨時可切、下一次
            // LLM 呼叫生效）；只保留 setter 單飛鎖。
            if (current.configRequest) {
                throw new Error("A session configuration change is already pending")
            }
            const connection = requireConnection(get())
            if (!connection.setSessionConfigOption) {
                throw new Error("Agent connection does not support session config options")
            }

            const token = ++nextConfigRequestToken
            const requestRevision = current.configRevision ?? 0
            set((state) => {
                const latest = state.sessions.get(sessionId)
                if (!latest) return {}
                const sessions = new Map(state.sessions)
                sessions.set(sessionId, {
                    ...latest,
                    configRequest: { token, configId, value },
                    configError: null
                })
                return { sessions }
            })

            try {
                const configOptions = await connection.setSessionConfigOption(sessionId, configId, value)
                set((state) => {
                    const latest = state.sessions.get(sessionId)
                    if (!latest || latest.configRequest?.token !== token) return {}
                    const sessions = new Map(state.sessions)
                    sessions.set(sessionId, latest.configRevision === requestRevision
                        ? {
                            ...latest,
                            ...authoritativeConfigPatch(latest, configOptions),
                            configRequest: null
                        }
                        : { ...latest, configRequest: null })
                    return { sessions }
                })
                return get().sessions.get(sessionId)?.configOptions ?? configOptions
            } catch (error) {
                const failure = error instanceof Error ? error : new Error(String(error))
                set((state) => {
                    const latest = state.sessions.get(sessionId)
                    if (!latest || latest.configRequest?.token !== token) return {}
                    const sessions = new Map(state.sessions)
                    sessions.set(sessionId, {
                        ...latest,
                        configRequest: null,
                        configError: failure.message
                    })
                    return { sessions }
                })
                throw error
            }
        },

        setUsage: (sessionId, usage) => {
            set((state) => {
                // usage 是附屬資料：session 不存在（已關閉／從未建立）就忽略，不應憑空造出 ghost session。
                const current = state.sessions.get(sessionId)
                if (!current) return {}
                const sessions = new Map(state.sessions)
                const cost = usage.cost ?? current.usage?.cost
                sessions.set(sessionId, {
                    ...current,
                    usage: {
                        used: usage.used,
                        size: usage.size,
                        ...(cost ? { cost: { ...cost } } : {})
                    }
                })
                return { sessions }
            })
        },

        applyAgentTitle: (sessionId, title) => {
            set((state) => {
                // title 同為附屬資料：session 不存在就忽略，理由同 setUsage。
                const current = state.sessions.get(sessionId)
                if (!current) return {}
                const sessions = new Map(state.sessions)
                const next = { ...current, agentTitle: truncateTitle(title) }
                sessions.set(sessionId, { ...next, title: resolveSessionTitle(next) })
                return { sessions }
            })
            syncSessionIndex(sessionId, get().sessions.get(sessionId))
        },

        onPermissionRequest: (sessionId, block, choose) => {
            set((state) => {
                if (droppedSessionIds.has(sessionId)) return {}
                const sessions = new Map(state.sessions)
                const pendingPermissions = new Map(state.pendingPermissions)
                const current = ensureSession(sessions, sessionId)
                pendingPermissions.set(sessionId, {
                    entryId: block.id,
                    request: permissionRequestSummary(block),
                    choose
                })
                sessions.set(sessionId, {
                    ...current,
                    tone: "wait"
                })
                return { sessions, pendingPermissions }
            })
        },

        respondPermission: (sessionId, optionId) => {
            const pending = get().pendingPermissions.get(sessionId)
            if (!pending) return
            try {
                pending.choose(optionId)
            } finally {
                set((state) => {
                    const sessions = new Map(state.sessions)
                    const pendingPermissions = new Map(state.pendingPermissions)
                    pendingPermissions.delete(sessionId)
                    const current = sessions.get(sessionId)
                    if (current) {
                        sessions.set(sessionId, {
                            ...current,
                            tone: "run",
                            pendingTurn: true,
                            running: true,
                            // 記錄所選 option（P3）：perm 卡據此鎖定按鈕並顯示結果。
                            // in-memory only——restored replay 會產生新 entry id，不持久化。
                            permissionOutcomes: {
                                ...current.permissionOutcomes,
                                [pending.entryId]: optionId
                            }
                        })
                    }
                    return { sessions, pendingPermissions }
                })
            }
        },

        onElicitationRequest: (sessionId, request, respond) => {
            set((state) => {
                if (droppedSessionIds.has(sessionId)) return {}
                const pendingElicitations = new Map(state.pendingElicitations)
                const queue = [...(pendingElicitations.get(sessionId) ?? [])]
                elicitationSeq += 1
                queue.push({ id: `el${elicitationSeq}`, request, respond })
                pendingElicitations.set(sessionId, queue)
                const sessions = new Map(state.sessions)
                const current = ensureSession(sessions, sessionId)
                sessions.set(sessionId, { ...current, tone: "wait" })
                return { sessions, pendingElicitations }
            })
        },

        respondElicitation: (sessionId, elicitationId, response) => {
            const queue = get().pendingElicitations.get(sessionId) ?? []
            const pending = queue.find((entry) => entry.id === elicitationId)
            if (!pending) return
            try {
                pending.respond(response)
            } finally {
                set((state) => {
                    const pendingElicitations = new Map(state.pendingElicitations)
                    const remaining = (pendingElicitations.get(sessionId) ?? [])
                        .filter((entry) => entry.id !== elicitationId)
                    if (remaining.length > 0) pendingElicitations.set(sessionId, remaining)
                    else pendingElicitations.delete(sessionId)
                    const sessions = new Map(state.sessions)
                    const current = sessions.get(sessionId)
                    // queue 清空才把 tone 收回 run（仍有排隊中的請求就維持 wait）。
                    if (current && remaining.length === 0) {
                        sessions.set(sessionId, {
                            ...current,
                            tone: "run",
                            pendingTurn: true,
                            running: true
                        })
                    }
                    return { sessions, pendingElicitations }
                })
            }
        },

        markConnectionError: (sessionId, error) => {
            set((state) => {
                if (droppedSessionIds.has(sessionId)) return {}
                const sessions = new Map(state.sessions)
                // agent process 已斷線：這個 session 只能靠 respawn＋load 續聊，沿用
                // restored 語意，讓 continueSession 走既有 replay／降級路徑，而不是
                // 誤以為它還是活著的 live session。
                sessions.set(sessionId, { ...failSession(ensureSession(sessions, sessionId), error), restored: true })
                return { sessions, connectionState: "error", connectionError: error.message }
            })
        },

        upsertSessionMeta: (meta) => {
            set((state) => {
                if (droppedSessionIds.has(meta.id)) return {}
                const sessions = new Map(state.sessions)
                sessions.set(meta.id, ensureSession(sessions, meta.id, sessionPatchFromMeta(meta)))
                return { sessions }
            })
        },

        reset: () => {
            draftWorkspaceCwd = null
            draftWorkspaceGeneration += 1
            draftSessionInFlight.clear()
            droppedSessionIds.clear()
            nextConfigRequestToken += 1
            set({
                ...agentInitialState,
                sessions: new Map<string, SessionState>(),
                pendingPermissions: new Map<string, PendingPermission>(),
                pendingElicitations: new Map<string, PendingElicitation[]>(),
                hydratedWorkspaceCwds: new Set<string>(),
                connection: options.connection ?? null,
                connectionState: options.connection ? "ready" : "idle",
                authRequired: null
            })
        }
    }))
}

export const useAgentStore = createAgentStore()

function requireConnection(state: Pick<AgentStoreState, "connection">): AgentConnection {
    if (!state.connection) throw new Error("Agent connection is not configured")
    return state.connection
}

function requireAbsoluteCwd(cwd: string): void {
    if (isAbsolutePath(cwd)) return
    throw new Error(i18n.t("agentZonePanel.relativeCwdError", { ns: "panels" }))
}

const defaultAgentPreset = (): AgentPreset => loadAgentSettings().preset

function identityFromRoute(route: ReturnType<typeof resolveAgentCommandRoute>): AgentCommandIdentity {
    return {
        selectedPreset: route.selectedPreset,
        commandMode: route.commandMode,
        trustedAgentId: route.trustedAgentId
    }
}

function draftSessionKey(
    cwd: string,
    route: ReturnType<typeof resolveAgentCommandRoute>
): string | null {
    return route.trustedAgentId
        ? `${normalizeWorkspacePath(cwd)}\0${route.trustedAgentId}:${route.commandMode}`
        : null
}

function visibleSessionForCwd(
    state: Pick<AgentStoreState, "activeSessionId" | "sessions">,
    cwd: string
): string | null {
    const active = selectActiveSessionForCwd(state, cwd)
    if (active) return active
    const normalizedCwd = normalizeWorkspacePath(cwd)
    for (const [sessionId, session] of state.sessions) {
        if (session.cwd && normalizeWorkspacePath(session.cwd) === normalizedCwd) return sessionId
    }
    return null
}

function unusedDraftSessionIdsForCwd(
    sessions: Map<string, SessionState>,
    cwd: string,
    excludeSessionId: string
): string[] {
    const normalizedCwd = normalizeWorkspacePath(cwd)
    return [...sessions.entries()].flatMap(([sessionId, session]) => (
        sessionId !== excludeSessionId
        && session.ephemeral === true
        && !session.pendingTurn
        && session.running !== true
        && session.cwd
        && normalizeWorkspacePath(session.cwd) === normalizedCwd
            ? [sessionId]
            : []
    ))
}

function authRequiredFromError(
    error: unknown,
    fallbackCwd: string,
    fallbackSessionId: string | null,
    attemptedAgentId: AgentId | null
): AuthRequiredState | null {
    if (!isAgentAuthRequiredError(error)) return null
    const fallbackRoute = resolveAgentCommandRoute(attemptedAgentId ?? undefined)
    const routedIdentity = error.agentIdentity
    const agentIdentity = routedIdentity ?? {
        selectedPreset: fallbackRoute.selectedPreset,
        commandMode: fallbackRoute.commandMode,
        trustedAgentId: fallbackRoute.trustedAgentId
    }
    const selectedAgentId = routedIdentity && routedIdentity.selectedPreset !== "custom"
        ? routedIdentity.selectedPreset
        : attemptedAgentId
    return {
        cwd: error.cwd ?? fallbackCwd,
        sessionId: error.sessionId ?? fallbackSessionId,
        authMethods: error.authMethods,
        message: error.message,
        agentId: selectedAgentId,
        agentCommand: error.agentCommand ?? fallbackRoute.command,
        agentIdentity
    }
}

type TerminalLoginSessionMeta = TerminalSessionMeta & { shellArgs?: string[] }

function terminalLoginSessionMeta(
    workspace: string,
    method: AgentAuthMethod,
    agentId?: AgentId | null,
    attemptedCommand?: string
): TerminalLoginSessionMeta {
    const terminalSettings = loadTerminalSettings()
    const sessionId = terminalSessionId()
    // 優先使用 router 記錄的本次 exact command；fallback 也走與 Settings／
    // picker／router 相同的 effective resolver，不再假設 curated 必定走固定版本。
    const agentCommand = attemptedCommand ?? resolveAgentCommandRoute(agentId ?? undefined).command
    return {
        sessionId,
        title: method.name,
        launchStatus: "opening",
        workspace,
        shell: terminalSettings.shellPath,
        shellArgs: ["-c", terminalLoginShellCommand(agentCommand, method)],
        cols: DEFAULT_TERMINAL_COLS,
        rows: DEFAULT_TERMINAL_ROWS
    }
}

function terminalAuthMethod(methods: AgentAuthMethod[]): AgentAuthMethod | null {
    return methods.find((method) => method.type === "terminal")
        ?? methods[0]
        ?? null
}

function loadTerminalSettings(): { shellPath: string } {
    const raw = readJsonSetting<{ shellPath?: string }>(TERMINAL_SETTINGS_STORAGE_KEY)
    return { shellPath: typeof raw.shellPath === "string" ? raw.shellPath.trim() : "" }
}

function readJsonSetting<T extends object>(key: string): Partial<T> {
    try {
        const raw = localStorage.getItem(key)
        return raw ? JSON.parse(raw) as Partial<T> : {}
    } catch {
        return {}
    }
}

function terminalSessionId(): string {
    return typeof crypto !== "undefined" && "randomUUID" in crypto
        ? `agent-login-${crypto.randomUUID()}`
        : `agent-login-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`
}

function terminalLoginShellCommand(agentCommand: string, method: AgentAuthMethod): string {
    const env = Object.entries(method.env)
        .filter(([name]) => /^[A-Za-z_][A-Za-z0-9_]*$/.test(name))
        .map(([name, value]) => `${name}=${shellQuote(value)}`)
    const envPrefix = env.length > 0 ? `env ${env.join(" ")}` : ""
    return [envPrefix, agentCommand, ...method.args.map(shellQuote)].filter(Boolean).join(" ")
}

export const __test_terminalLoginShellCommand = (method: AgentAuthMethod) =>
    terminalLoginShellCommand(resolveAgentCommand(), method)

function shellQuote(value: string): string {
    if (/^[A-Za-z0-9_./:=@%+-]+$/.test(value)) return value
    return `'${value.replace(/'/g, "'\\''")}'`
}

function emptySession(patch: Partial<SessionState> = {}): SessionState {
    return {
        title: DEFAULT_SESSION_TITLE,
        // agentId 刻意不給預設值（Phase 2 review 裁決的延欠修正）：判色與顯示皆已
        // 走「agentId 未知/undefined → agentLabel/fallback」語意，硬編 "pi" 只會讓
        // 從未指定 agent 的 session（如 upsertSessionMeta 建立的舊資料）被誤判成 pi。
        agentLabel: DEFAULT_AGENT_LABEL,
        model: null,
        tone: "idle",
        transcript: [],
        availableCommands: [],
        stopReason: null,
        stopBadge: null,
        error: null,
        queueDepth: null,
        running: null,
        pendingTurn: false,
        metadataTitle: false,
        cwd: null,
        infoBanner: null,
        configOptions: [],
        configRevision: 0,
        configRequest: null,
        configError: null,
        ...patch
    }
}

function authoritativeConfigPatch(
    session: SessionState | undefined,
    configOptions: SessionConfigOption[]
): Pick<SessionState, "configOptions" | "configRevision" | "configError"> {
    return {
        configOptions: [...configOptions],
        configRevision: (session?.configRevision ?? 0) + 1,
        configError: null
    }
}

// 三欄位（sessionAlias／agentTitle／derivedTitle）任一變動時的顯示名稱解析：
// sessionAlias（null 或空視為無 alias）優先，其次 agentTitle，其次 derivedTitle，
// 都沒有則回退 DEFAULT_SESSION_TITLE。
export function resolveSessionTitle(fields: {
    sessionAlias?: string | null
    agentTitle?: string
    derivedTitle?: string
}): string {
    return fields.sessionAlias || fields.agentTitle || fields.derivedTitle || DEFAULT_SESSION_TITLE
}

function ensureSession(
    sessions: Map<string, SessionState>,
    sessionId: string,
    patch: Partial<SessionState> = {}
): SessionState {
    const existing = sessions.get(sessionId)
    const merged = existing ? { ...existing, ...patch } : emptySession(patch)
    return { ...merged, title: resolveSessionTitle(merged) }
}

const VALID_AGENT_PRESETS: AgentPreset[] = ["pi", "claude", "codex", "custom"]

function toAgentPreset(value: string | undefined): AgentPreset | undefined {
    return VALID_AGENT_PRESETS.includes(value as AgentPreset) ? (value as AgentPreset) : undefined
}

type StoreSet = (fn: (state: AgentStoreState) => Partial<AgentStoreState>) => void

// A rejected/stale session/new result may already have emitted pre-registration
// callbacks that created a placeholder. Remove that placeholder together with
// any pending permission; callers record the tombstone before invoking this.
function discardSessionState(set: StoreSet, sessionId: string): void {
    set((state) => {
        if (!state.sessions.has(sessionId) && !state.pendingPermissions.has(sessionId)) return {}
        const sessions = new Map(state.sessions)
        const pendingPermissions = new Map(state.pendingPermissions)
        sessions.delete(sessionId)
        pendingPermissions.delete(sessionId)
        return {
            sessions,
            pendingPermissions,
            activeSessionId: state.activeSessionId === sessionId ? null : state.activeSessionId
        }
    })
}

// continueSession 的兩個小 set() helper：一個是靜態 patch，一個需要既有 session
// 才能算出下一個值（如 append 一個 block）。都對不存在的 session 安靜跳過。
function patchSession(set: StoreSet, sessionId: string, patch: Partial<SessionState>): void {
    set((state) => {
        const current = state.sessions.get(sessionId)
        if (!current) return {}
        const sessions = new Map(state.sessions)
        sessions.set(sessionId, { ...current, ...patch })
        return { sessions }
    })
}

function patchSessionWith(
    set: StoreSet,
    sessionId: string,
    fn: (current: SessionState) => SessionState
): void {
    set((state) => {
        const current = state.sessions.get(sessionId)
        if (!current) return {}
        const sessions = new Map(state.sessions)
        sessions.set(sessionId, fn(current))
        return { sessions }
    })
}

// Continue 流程的降級路徑（agent 未宣告 loadSession，或 load 失敗）：append 一個
// notice block，帶「以同 agent 開新對話」action；session 本身（含 restored 標記、
// Session Index 條目）原樣保留，不清除、不移除。
function degradeContinueSession(session: SessionState, cwd: string, agentId: AgentPreset | undefined): SessionState {
    const text = i18n.t("agentZonePanel.continueUnsupported", { ns: "panels" })
    // F8：連點兩次未支援 load 的 restored row 不該疊出兩條一樣的 notice——尾端已是
    // 同文案的 notice 就跳過 append，只更新 tone。
    const last = session.transcript.at(-1)
    if (last && "kind" in last && last.kind === "notice" && last.text === text) {
        return { ...session, tone: "idle" }
    }
    const notice: BlockEntry = {
        id: newEntryId(),
        kind: "notice",
        text,
        actions: [{
            label: i18n.t("agentZonePanel.continueNewSession", { ns: "panels" }),
            kind: "start_new_session",
            payload: { cwd, ...(agentId && agentId !== "custom" ? { agentId } : {}) }
        }]
    }
    return { ...session, tone: "idle", transcript: [...session.transcript, notice] }
}

// Session Index 同步：把三欄位（agentTitle／sessionAlias／derivedTitle）與 agentId
// 對齊到 localStorage。無 cwd 的 session（尚未綁定 workspace）不索引。
function sessionIndexPatch(session: SessionState) {
    return {
        agentId: session.agentId,
        customCommandFingerprint: session.customCommandFingerprint,
        agentTitle: session.agentTitle,
        sessionAlias: session.sessionAlias,
        derivedTitle: session.derivedTitle
    }
}

function ensureSessionIndexEntry(sessionId: string, session: SessionState | undefined): void {
    if (!session?.cwd) return
    if (session.ephemeral) {
        removeSessionIndexEntry(sessionId)
        return
    }
    const now = Date.now()
    upsertSessionIndexEntry({
        sessionId,
        cwd: session.cwd,
        createdAt: now,
        lastActiveAt: now,
        ...sessionIndexPatch(session)
    })
}

function syncSessionIndex(sessionId: string, session: SessionState | undefined): void {
    if (!session?.cwd) return
    if (session.ephemeral) {
        removeSessionIndexEntry(sessionId)
        return
    }
    touchSessionIndexEntry(sessionId, { ...sessionIndexPatch(session), lastActiveAt: Date.now() })
}

function promptBlocks(prompt: string | PromptBlock[]): PromptBlock[] {
    return typeof prompt === "string" ? [{ type: "text", text: prompt }] : prompt
}

function textFromBlocks(blocks: PromptBlock[]): string {
    return blocks.flatMap((block) => {
        if (block.type === "text") return [block.text]
        if (block.type === "image") return ["[image]"]
        return [block.title ?? block.name]
    }).join(" ").trim()
}

function imagesFromBlocks(blocks: PromptBlock[]): { mimeType: string; dataUrl: string }[] {
    return blocks.flatMap((block) =>
        block.type === "image"
            ? [{ mimeType: block.mimeType, dataUrl: `data:${block.mimeType};base64,${block.data}` }]
            : []
    )
}

function titleFromPrompt(blocks: PromptBlock[]): string {
    return truncateTitle(textFromBlocks(blocks))
}

function truncateTitle(title: string): string {
    const normalized = title.replace(/\s+/g, " ").trim()
    if (!normalized) return DEFAULT_SESSION_TITLE
    return normalized.length > TITLE_LIMIT ? `${normalized.slice(0, TITLE_LIMIT - 1)}…` : normalized
}

function beginTurn(session: SessionState, promptTitle: string, blocks: PromptBlock[]): SessionState {
    const promptText = textFromBlocks(blocks)
    const images = imagesFromBlocks(blocks)
    const userEntry =
        promptText || images.length > 0
            ? [{
                id: newEntryId(),
                who: "you" as const,
                text: promptText,
                streaming: true,
                ...(images.length > 0 ? { images } : {})
            }]
            : []
    // 首句 prompt 衍生標題路徑：只在三欄位都還沒有值時才寫 derivedTitle，不再直接寫 title。
    const hasTitle = Boolean(session.sessionAlias || session.agentTitle || session.derivedTitle)
    const next: SessionState = {
        ...session,
        tone: "run",
        transcript: [...session.transcript, ...userEntry],
        stopReason: null,
        stopBadge: null,
        error: null,
        pendingTurn: true,
        running: true,
        ...(hasTitle ? {} : { derivedTitle: promptTitle })
    }
    return { ...next, title: resolveSessionTitle(next) }
}

function applyStopReason(session: SessionState, stopReason: StopReason): SessionState {
    const base: SessionState = {
        ...session,
        tone: stopReason === "cancelled" ? "idle" : "done",
        transcript: settleTranscript(session.transcript),
        stopReason,
        stopBadge: null,
        error: null,
        pendingTurn: false,
        running: false
    }
    if (stopReason === "refusal") {
        return {
            ...base,
            stopBadge: { kind: "refusal", label: "Refused", stopReason },
            transcript: [
                ...base.transcript,
                {
                    id: newEntryId(),
                    kind: "error",
                    text: "Agent refused the request.",
                    meta: JSON.stringify({ stopReason })
                }
            ]
        }
    }
    if (stopReason === "max_tokens" || stopReason === "max_turn_requests") {
        return {
            ...base,
            stopBadge: { kind: "truncated", label: stopReasonLabel(stopReason), stopReason },
            transcript: [
                ...base.transcript,
                {
                    id: newEntryId(),
                    kind: "tool",
                    text: stopReasonLabel(stopReason),
                    meta: JSON.stringify({ stopReason, truncated: true })
                }
            ]
        }
    }
    if (stopReason === "cancelled") {
        const already = base.transcript.some((e) => "kind" in e && e.meta === CANCELLED_META)
        if (already) return base
        return {
            ...base,
            transcript: [
                ...base.transcript,
                {
                    id: newEntryId(),
                    kind: "tool",
                    text: i18n.t("agentZonePanel.interrupted", { ns: "panels" }),
                    meta: CANCELLED_META
                }
            ]
        }
    }
    return base
}

function permissionRequestSummary(block: BlockEntry): PendingPermission["request"] {
    const request: PendingPermission["request"] = {
        text: block.text,
        actions: block.actions ? [...block.actions] : []
    }
    if (block.meta !== undefined) request.meta = block.meta
    return request
}

function stopReasonLabel(stopReason: "max_tokens" | "max_turn_requests"): string {
    return stopReason === "max_tokens" ? "Stopped at token limit" : "Stopped at turn limit"
}

function settleTranscript(transcript: TranscriptEntry[]): TranscriptEntry[] {
    return transcript.map((entry) => ("streaming" in entry ? { ...entry, streaming: false } : entry))
}

function failSession(session: SessionState, error: Error): SessionState {
    return {
        ...session,
        tone: "fail",
        transcript: settleTranscript(session.transcript),
        error: error.message,
        pendingTurn: false,
        running: false
    }
}

// 首句 prompt 的另一條衍生標題路徑（transcript 落地後才補算，例如 loadSession 復原對話）：
// 同樣只在三欄位都還沒有值時才補上 derivedTitle，並回傳需要合併的 patch（可能為空物件）。
function deriveTitle(session: SessionState, transcript: TranscriptEntry[]): Partial<SessionState> {
    if (session.sessionAlias || session.agentTitle || session.derivedTitle) return {}
    const firstUser = transcript.find((entry) => "who" in entry && entry.who === "you")
    if (!firstUser) return {}
    const derivedTitle = truncateTitle(firstUser.text)
    return { derivedTitle, title: resolveSessionTitle({ ...session, derivedTitle }) }
}

function commandPrefix(prefix: string): string {
    return prefix.trim().replace(/^\//, "").toLowerCase()
}

function modeFromUpdate(update: Record<string, unknown>): string | undefined {
    return update.sessionUpdate === "current_mode_update" && typeof update.currentModeId === "string"
        ? update.currentModeId
        : undefined
}

function sessionPatchFromMeta(meta: AgentSessionMeta): Partial<SessionState> {
    const title = meta.title ?? meta.name
    const patch: Partial<SessionState> = {
        cwd: meta.cwd,
    }
    if (title) {
        // agent／meta 提供的標題歸類為 agentTitle；title 顯示值交由 ensureSession 統一以
        // resolveSessionTitle 重算，不在這裡直接寫。
        patch.agentTitle = truncateTitle(title)
    }
    if (meta.agentLabel) patch.agentLabel = meta.agentLabel
    if (meta.model) patch.model = meta.model
    return patch
}

// removeSession：被移除者為 active session 時，接替者取同 cwd（normalizeWorkspacePath
// 比對）其餘 session 中 Session Index 記錄 lastActiveAt 最新的一筆；沒有記錄的候選
// 一律視為最舊（0），沒有候選則回傳 null（切回無 active session）。
function nextActiveSessionForCwd(
    sessions: Map<string, SessionState>,
    cwd: string | null,
    excludeSessionId: string
): string | null {
    if (!cwd) return null
    const normalizedCwd = normalizeWorkspacePath(cwd)
    const candidates = [...sessions.entries()].filter(
        ([id, session]) => id !== excludeSessionId
            && session.cwd
            && normalizeWorkspacePath(session.cwd) === normalizedCwd
    )
    if (candidates.length === 0) return null
    const lastActiveById = new Map(loadSessionIndex().map((entry) => [entry.sessionId, entry.lastActiveAt]))
    candidates.sort(([a], [b]) => (lastActiveById.get(b) ?? 0) - (lastActiveById.get(a) ?? 0))
    return candidates[0]?.[0] ?? null
}

// P10-B：切到 workspace B 時，A 的 active session 不應被沿用。回傳目前
// activeSessionId 僅在其 cwd 與傳入的 cwd 相符時才視為有效，否則回傳 null。
function selectActiveSessionForCwd(
    state: Pick<AgentStoreState, "activeSessionId" | "sessions">,
    cwd: string
): string | null {
    const id = state.activeSessionId
    if (!id) return null
    const s = state.sessions.get(id)
    if (!s || !s.cwd) return null
    return normalizeWorkspacePath(s.cwd) === normalizeWorkspacePath(cwd) ? id : null
}

// Workspace rail 徽章用：依 normalizeWorkspacePath 分組統計每個 workspace 的
// agent 總數與執行中數。純函式，呼叫端須自行以 useMemo 包裝以避免每次
// render 都回傳新 Map。
export function selectWorkspaceAgentCounts(
    sessions: Map<string, SessionState>
): Map<string, { total: number; running: number }> {
    const counts = new Map<string, { total: number; running: number }>()
    for (const session of sessions.values()) {
        if (!session.cwd) continue
        const key = normalizeWorkspacePath(session.cwd)
        const cur = counts.get(key) ?? { total: 0, running: 0 }
        cur.total += 1
        if (session.running === true || session.tone === "run") cur.running += 1
        counts.set(key, cur)
    }
    return counts
}
