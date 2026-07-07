import { create } from "zustand"

import type {
    AgentConnection,
    AgentAuthMethod,
    PromptBlock,
    SessionInfoUpdate,
    SessionMeta,
    SlashCommand,
    StopReason
} from "@/agent/acpConnection"
import { isAgentAuthRequiredError, reduceSessionUpdate } from "@/agent/acpConnection"
import type { BlockEntry, TranscriptAction, TranscriptEntry } from "@/agent/acpTypes"
import i18n from "@/lib/i18n"
import { isAbsolutePath } from "@/lib/paths"
import { useTerminalStore, type TerminalSessionMeta } from "@/state/terminalStore"
import { useUiStore } from "@/state/uiStore"

export type AgentTone = "idle" | "run" | "done" | "wait" | "fail"
export type AgentConnectionState = "idle" | "connecting" | "ready" | "error"
export type StopBadgeKind = "refusal" | "truncated"
export type PermissionResolver = (optionId: string) => void

export interface StopBadge {
    kind: StopBadgeKind
    label: string
    stopReason: StopReason
}

export interface SessionState {
    title: string
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
    cwd: string | null
}

export interface AuthRequiredState {
    cwd: string
    sessionId: string | null
    authMethods: AgentAuthMethod[]
    message: string
}

export interface PendingPermission {
    request: {
        text: string
        actions: TranscriptAction[]
        meta?: string
    }
    choose: PermissionResolver
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
    activeSessionId: string | null
    connectionState: AgentConnectionState
    connectionError: string | null
    connection: AgentConnection | null
    authRequired: AuthRequiredState | null
    setConnection: (connection: AgentConnection | null) => void
    newSession: (cwd: string) => Promise<string>
    beginTerminalLogin: () => void
    retryAfterLogin: () => Promise<string>
    loadSessions: (cwd: string) => Promise<void>
    sendPrompt: (cwd: string, prompt: string | PromptBlock[]) => Promise<StopReason>
    cancel: (sessionId?: string) => void
    selectSession: (sessionId: string) => void
    setAvailableCommands: (sessionId: string, commands: SlashCommand[]) => void
    filterCommands: (prefix: string, sessionId?: string) => SlashCommand[]
    appendUpdate: (sessionId: string, update: Record<string, unknown>) => void
    replaceTranscript: (sessionId: string, transcript: TranscriptEntry[]) => void
    onSessionInfo: (sessionId: string, info: SessionInfoUpdate) => void
    onPermissionRequest: (sessionId: string, block: BlockEntry, choose: PermissionResolver) => void
    respondPermission: (sessionId: string, optionId: string) => void
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
const DEFAULT_AGENT_COMMAND = "bunx pi-acp@0.0.31"
const AGENT_SETTINGS_STORAGE_KEY = "yuzora:agent-settings"
const TERMINAL_SETTINGS_STORAGE_KEY = "yuzora:terminal-settings"
const DEFAULT_TERMINAL_COLS = 80
const DEFAULT_TERMINAL_ROWS = 24

export const agentInitialState = {
    sessions: new Map<string, SessionState>(),
    pendingPermissions: new Map<string, PendingPermission>(),
    activeSessionId: null as string | null,
    connectionState: "idle" as AgentConnectionState,
    connectionError: null as string | null,
    connection: null as AgentConnection | null,
    authRequired: null as AuthRequiredState | null
}

export function createAgentStore(options: CreateAgentStoreOptions = {}) {
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

        newSession: async (cwd) => {
            const connection = requireConnection(get())
            requireAbsoluteCwd(cwd)
            set({ connectionState: "connecting", connectionError: null })
            try {
                const sessionId = await connection.newSession(cwd)
                set((state) => {
                    const sessions = new Map(state.sessions)
                    sessions.set(sessionId, ensureSession(sessions, sessionId, { cwd }))
                    return {
                        sessions,
                        activeSessionId: sessionId,
                        connectionState: "ready",
                        connectionError: null,
                        authRequired: null
                    }
                })
                return sessionId
            } catch (error) {
                const err = error instanceof Error ? error : new Error(String(error))
                const authRequired = authRequiredFromError(error, cwd, null)
                set({
                    connectionState: "error",
                    connectionError: err.message,
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
            const meta = terminalLoginSessionMeta(authRequired.cwd, method)
            useTerminalStore.getState().addSession(authRequired.cwd, meta)
            if (!useUiStore.getState().terminalOpen) {
                useUiStore.getState().toggleTerminal()
            }
        },

        retryAfterLogin: async () => {
            const authRequired = get().authRequired
            if (!authRequired) throw new Error("No pending agent authentication")
            return get().newSession(authRequired.cwd)
        },

        loadSessions: async (cwd) => {
            const connection = requireConnection(get())
            set({ connectionState: "connecting", connectionError: null })
            try {
                const metas = await connection.listSessions(cwd)
                set((state) => {
                    const sessions = new Map(state.sessions)
                    for (const meta of metas) {
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
            let sessionId = get().activeSessionId
            set({ connectionState: "connecting", connectionError: null })
            try {
                if (!sessionId) {
                    sessionId = await connection.newSession(cwd)
                }
                const activeSessionId = sessionId
                set((state) => {
                    const sessions = new Map(state.sessions)
                    const current = ensureSession(sessions, activeSessionId, { cwd })
                    sessions.set(activeSessionId, beginTurn(current, promptTitle, blocks))
                    return { sessions, activeSessionId, connectionState: "ready" }
                })
                const stopReason = await connection.prompt(activeSessionId, blocks)
                set((state) => {
                    const sessions = new Map(state.sessions)
                    const pendingPermissions = new Map(state.pendingPermissions)
                    pendingPermissions.delete(activeSessionId)
                    const current = ensureSession(sessions, activeSessionId, { cwd })
                    sessions.set(activeSessionId, applyStopReason(current, stopReason))
                    return { sessions, pendingPermissions, connectionState: "ready" }
                })
                return stopReason
            } catch (error) {
                const err = error instanceof Error ? error : new Error(String(error))
                const authRequired = authRequiredFromError(error, cwd, sessionId ?? null)
                set((state) => {
                    if (!sessionId) {
                        return {
                            connectionState: "error",
                            connectionError: err.message,
                            ...(authRequired ? { authRequired } : {})
                        }
                    }
                    const sessions = new Map(state.sessions)
                    const pendingPermissions = new Map(state.pendingPermissions)
                    pendingPermissions.delete(sessionId)
                    sessions.set(sessionId, failSession(ensureSession(sessions, sessionId, { cwd }), err))
                    return {
                        sessions,
                        pendingPermissions,
                        connectionState: "error",
                        connectionError: err.message,
                        ...(authRequired ? { authRequired } : {})
                    }
                })
                throw error
            }
        },

        cancel: (sessionId = get().activeSessionId ?? undefined) => {
            if (!sessionId) return
            get().connection?.cancel(sessionId)
            set((state) => {
                const sessions = new Map(state.sessions)
                const pendingPermissions = new Map(state.pendingPermissions)
                pendingPermissions.delete(sessionId)
                const current = sessions.get(sessionId)
                if (!current) return { pendingPermissions }
                sessions.set(sessionId, applyStopReason(current, "cancelled"))
                return { sessions, pendingPermissions }
            })
        },

        selectSession: (sessionId) => {
            set((state) => {
                const sessions = new Map(state.sessions)
                sessions.set(sessionId, ensureSession(sessions, sessionId))
                return { sessions, activeSessionId: sessionId }
            })
        },

        setAvailableCommands: (sessionId, commands) => {
            set((state) => {
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
            set((state) => {
                const sessions = new Map(state.sessions)
                const current = ensureSession(sessions, sessionId)
                const transcript = reduceSessionUpdate(current.transcript, update)
                sessions.set(sessionId, {
                    ...current,
                    transcript,
                    mode: modeFromUpdate(update) ?? current.mode,
                    title: deriveTitle(current, transcript)
                })
                return { sessions }
            })
        },

        replaceTranscript: (sessionId, transcript) => {
            set((state) => {
                const sessions = new Map(state.sessions)
                const current = ensureSession(sessions, sessionId)
                sessions.set(sessionId, {
                    ...current,
                    transcript: [...transcript],
                    title: deriveTitle(current, transcript)
                })
                return { sessions }
            })
        },

        onSessionInfo: (sessionId, info) => {
            set((state) => {
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

        onPermissionRequest: (sessionId, block, choose) => {
            set((state) => {
                const sessions = new Map(state.sessions)
                const pendingPermissions = new Map(state.pendingPermissions)
                const current = ensureSession(sessions, sessionId)
                pendingPermissions.set(sessionId, {
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
                            running: true
                        })
                    }
                    return { sessions, pendingPermissions }
                })
            }
        },

        markConnectionError: (sessionId, error) => {
            set((state) => {
                const sessions = new Map(state.sessions)
                sessions.set(sessionId, failSession(ensureSession(sessions, sessionId), error))
                return { sessions, connectionState: "error", connectionError: error.message }
            })
        },

        upsertSessionMeta: (meta) => {
            set((state) => {
                const sessions = new Map(state.sessions)
                sessions.set(meta.id, ensureSession(sessions, meta.id, sessionPatchFromMeta(meta)))
                return { sessions }
            })
        },

        reset: () => set({
            ...agentInitialState,
            sessions: new Map<string, SessionState>(),
            pendingPermissions: new Map<string, PendingPermission>(),
            connection: options.connection ?? null,
            connectionState: options.connection ? "ready" : "idle",
            authRequired: null
        })
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

function authRequiredFromError(
    error: unknown,
    fallbackCwd: string,
    fallbackSessionId: string | null
): AuthRequiredState | null {
    if (!isAgentAuthRequiredError(error)) return null
    return {
        cwd: error.cwd ?? fallbackCwd,
        sessionId: error.sessionId ?? fallbackSessionId,
        authMethods: error.authMethods,
        message: error.message
    }
}

type TerminalLoginSessionMeta = TerminalSessionMeta & { shellArgs?: string[] }

function terminalLoginSessionMeta(workspace: string, method: AgentAuthMethod): TerminalLoginSessionMeta {
    const terminalSettings = loadTerminalSettings()
    const sessionId = terminalSessionId()
    return {
        sessionId,
        title: method.name,
        workspace,
        shell: terminalSettings.shellPath,
        shellArgs: ["-c", terminalLoginShellCommand(resolveAgentCommand(), method)],
        cols: DEFAULT_TERMINAL_COLS,
        rows: DEFAULT_TERMINAL_ROWS
    }
}

function terminalAuthMethod(methods: AgentAuthMethod[]): AgentAuthMethod | null {
    return methods.find((method) => method.type === "terminal")
        ?? methods[0]
        ?? null
}

function resolveAgentCommand(): string {
    const raw = readJsonSetting<{ preset?: string; command?: string }>(AGENT_SETTINGS_STORAGE_KEY)
    const command = typeof raw.command === "string" ? raw.command.trim() : ""
    return raw.preset === "custom" && command ? command : DEFAULT_AGENT_COMMAND
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

function shellQuote(value: string): string {
    if (/^[A-Za-z0-9_./:=@%+-]+$/.test(value)) return value
    return `'${value.replace(/'/g, "'\\''")}'`
}

function emptySession(patch: Partial<SessionState> = {}): SessionState {
    return {
        title: DEFAULT_SESSION_TITLE,
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
        ...patch
    }
}

function ensureSession(
    sessions: Map<string, SessionState>,
    sessionId: string,
    patch: Partial<SessionState> = {}
): SessionState {
    const existing = sessions.get(sessionId)
    return existing ? { ...existing, ...patch } : emptySession(patch)
}

function promptBlocks(prompt: string | PromptBlock[]): PromptBlock[] {
    return typeof prompt === "string" ? [{ type: "text", text: prompt }] : prompt
}

function textFromBlocks(blocks: PromptBlock[]): string {
    return blocks.flatMap((block) => {
        if (block.type === "text") return [block.text]
        return [block.title ?? block.name]
    }).join(" ").trim()
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
    const userEntry = promptText ? [{ who: "you" as const, text: promptText, streaming: true }] : []
    return {
        ...session,
        title: session.metadataTitle || session.title !== DEFAULT_SESSION_TITLE ? session.title : promptTitle,
        tone: "run",
        transcript: [...session.transcript, ...userEntry],
        stopReason: null,
        stopBadge: null,
        error: null,
        pendingTurn: true,
        running: true
    }
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
                    kind: "tool",
                    text: stopReasonLabel(stopReason),
                    meta: JSON.stringify({ stopReason, truncated: true })
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

function deriveTitle(session: SessionState, transcript: TranscriptEntry[]): string {
    if (session.metadataTitle || session.title !== DEFAULT_SESSION_TITLE) return session.title
    const firstUser = transcript.find((entry) => "who" in entry && entry.who === "you")
    return firstUser ? truncateTitle(firstUser.text) : session.title
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
        patch.title = truncateTitle(title)
        patch.metadataTitle = true
    }
    if (meta.agentLabel) patch.agentLabel = meta.agentLabel
    if (meta.model) patch.model = meta.model
    return patch
}
