import type { AgentCommandIdentity, AgentCommandResolution, AgentId } from "@/lib/agentPresets"
import { resolveAgentCommandRoute } from "@/app/workbench/settingsStorage"
import {
    AgentAuthRequiredError,
    classifySessionNewFailure,
    createAcpConnection,
    isAgentAuthRequiredError,
    isAgentColdStartCancelledError,
    type AcpClientRuntimeDeps,
    type AgentConnection,
    type PromptBlock,
    type SessionConfigOption,
    type SessionConfigValue,
    type SessionMeta,
    type StopReason
} from "./acpConnection"
import { isAgentRuntimePrerequisiteError } from "./agentRuntime"

type SubFactory = (command: string, cwd: string) => AgentConnection

export interface AgentRouter extends AgentConnection {
    ownedProcessIds(): Promise<string[]>
}

const SEP = "\0"
const RETRY_BASE_MS = 1_000
const RETRY_MAX_MS = 30_000
const TEARDOWN_GATE_TIMEOUT_MS = 5_000

interface AgentRouterDeps extends AcpClientRuntimeDeps {
    now?: () => number
}

interface SubState {
    command: string
    cwd: string
    connection: AgentConnection
    sessionIds: Set<string>
    pendingOwners: number
}

interface RetryState {
    attempt: number
    nextAllowedAt: number
    inFlight: ReturnType<AgentConnection["newSession"]> | null
}

export class AgentRetryCooldownError extends Error {
    readonly nextAllowedAt: number

    constructor(nextAllowedAt: number) {
        super(`Agent retry is cooling down until ${nextAllowedAt}`)
        this.name = "AgentRetryCooldownError"
        this.nextAllowedAt = nextAllowedAt
    }
}

export async function fingerprintAgentCommand(command: string): Promise<string | undefined> {
    const subtle = globalThis.crypto?.subtle
    if (!subtle) return undefined
    const digest = await subtle.digest("SHA-256", new TextEncoder().encode(command))
    const hex = [...new Uint8Array(digest)]
        .map((byte) => byte.toString(16).padStart(2, "0"))
        .join("")
    return `sha256:${hex}`
}

export function createAgentRouter(
    deps: AgentRouterDeps,
    factory: SubFactory = (command) => createAcpConnection({ ...deps, command })
): AgentRouter {
    const subs = new Map<string, SubState>() // key = `${command}${SEP}${cwd}`
    const sessionKey = new Map<string, string>() // sessionId -> subs key (same `${command}${SEP}${cwd}` format)
    const pendingOwnerlessDisposals = new Map<string, Promise<boolean>>()
    const retryStates = new Map<string, RetryState>()
    const now = deps.now ?? Date.now

    const routeFor = async (agentId?: AgentId, expectedCustomFingerprint?: string) => {
        const route = resolveAgentCommandRoute(agentId)
        if (route.trustedAgentId) {
            if (expectedCustomFingerprint) throw new Error("The custom agent command is no longer selected")
            return { ...route, customCommandFingerprint: undefined }
        }

        const customCommandFingerprint = await fingerprintAgentCommand(route.command)
        if (!customCommandFingerprint) {
            if (expectedCustomFingerprint) throw new Error("Custom agent command identity is unavailable")
            return { ...route, customCommandFingerprint: undefined }
        }
        if (expectedCustomFingerprint && customCommandFingerprint !== expectedCustomFingerprint) {
            throw new Error("The custom agent command has changed")
        }
        return { ...route, customCommandFingerprint }
    }

    const ensureSub = async (command: string, cwd: string): Promise<[string, SubState]> => {
        const key = `${command}${SEP}${cwd}`
        const pendingDisposal = pendingOwnerlessDisposals.get(key)
        if (pendingDisposal) {
            await withTeardownGate(pendingDisposal, TEARDOWN_GATE_TIMEOUT_MS)
        }
        const existing = subs.get(key)
        if (existing) return [key, existing]
        const created: SubState = {
            command,
            cwd,
            connection: factory(command, cwd),
            sessionIds: new Set<string>(),
            pendingOwners: 0
        }
        subs.set(key, created)
        return [key, created]
    }

    const rememberOwnerlessDisposal = (
        key: string,
        sub: SubState,
        disposal: Promise<boolean>
    ) => {
        const outcome = disposal.catch(() => false)
        pendingOwnerlessDisposals.set(key, outcome)
        void outcome.then((disposed) => {
            if (!disposed
                || subs.get(key) !== sub
                || sub.sessionIds.size > 0
                || sub.pendingOwners > 0) {
                return
            }
            subs.delete(key)
        }).finally(() => {
            if (pendingOwnerlessDisposals.get(key) === outcome) {
                pendingOwnerlessDisposals.delete(key)
            }
        })
    }

    const retryStateFor = (key: string): RetryState => {
        const existing = retryStates.get(key)
        if (existing) return existing
        const created: RetryState = { attempt: 0, nextAllowedAt: 0, inFlight: null }
        retryStates.set(key, created)
        return created
    }

    const subForSession = (sessionId: string): SubState => {
        const key = sessionKey.get(sessionId)
        const sub = key ? subs.get(key) : undefined
        if (!sub) throw new Error(`Unknown session ${sessionId}`)
        return sub
    }

    return {
        async prepare(cwd, agentId) {
            const route = await routeFor(agentId)
            if (!route.trustedAgentId) {
                throw new Error("Custom agent commands cannot be prepared")
            }
            const [, sub] = await ensureSub(route.command, cwd)
            if (!sub.connection.prepare) throw new Error("Agent connection does not support prepare")
            await sub.connection.prepare(cwd)
        },
        async newSession(cwd, agentId) {
            const route = await routeFor(agentId)
            const retryKey = `${route.command}${SEP}${cwd}`
            const retryState = retryStateFor(retryKey)
            if (retryState.inFlight) return retryState.inFlight
            if (now() < retryState.nextAllowedAt) {
                throw new AgentRetryCooldownError(retryState.nextAllowedAt)
            }

            const request = (async () => {
                const [key, sub] = await ensureSub(route.command, cwd)
                sub.pendingOwners += 1
                let failureKind: ReturnType<typeof classifySessionNewFailure> | undefined
                try {
                    let result
                    try {
                        result = await sub.connection.newSession(cwd, agentId)
                    } catch (error) {
                        failureKind = classifySessionNewFailure(error)
                        throw withRouteOnAuthError(error, route)
                    }
                    sub.sessionIds.add(result.sessionId)
                    sessionKey.set(result.sessionId, key)
                    return {
                        ...result,
                        agentIdentity: identityFromRoute(route),
                        ...(route.customCommandFingerprint
                            ? { customCommandFingerprint: route.customCommandFingerprint }
                            : {})
                    }
                } finally {
                    sub.pendingOwners -= 1
                    if (failureKind
                        && failureKind !== "auth-required"
                        && subs.get(key) === sub
                        && sub.sessionIds.size === 0
                        && sub.pendingOwners === 0) {
                        const disposal = sub.connection.disposeOwnerless?.(
                            "session_new_failed"
                        )
                        if (disposal) rememberOwnerlessDisposal(key, sub, disposal)
                    }
                }
            })()
            retryState.inFlight = request
            try {
                const result = await request
                retryState.attempt = 0
                retryState.nextAllowedAt = 0
                return result
            } catch (error) {
                // #37 S3：使用者主動取消冷啟動下載不是連線失敗，比照 auth-required
                // 豁免 backoff——否則連按取消會被罰站到 30 秒。
                // #15：缺 runtime 同理——那是使用者幾秒內就能修好的外部條件，不是
                // agent 掛掉；退讓曲線只會把 prerequisite banner 自己的 Retry 鎖死。
                if (isAgentAuthRequiredError(error)
                    || isAgentColdStartCancelledError(error)
                    || isAgentRuntimePrerequisiteError(error)) {
                    retryState.attempt = 0
                    retryState.nextAllowedAt = 0
                } else {
                    retryState.attempt += 1
                    retryState.nextAllowedAt = now() + Math.min(
                        RETRY_BASE_MS * 2 ** (retryState.attempt - 1),
                        RETRY_MAX_MS
                    )
                }
                throw error
            } finally {
                if (retryState.inFlight === request) retryState.inFlight = null
            }
        },
        async loadSession(id, cwd, agentId, customCommandFingerprint) {
            // 已知 session → 路由；未知（如 restore）→ 依 agentId 選 sub（無 agentId
            // 則沿用目前預設 command）。先抓 key 再 await，避免 in-flight 期間 settings
            // 改變造成 TOCTOU 誤路由。
            const known = sessionKey.get(id)
            const route = known ? null : await routeFor(agentId, customCommandFingerprint)
            const [key, sub] = known
                ? [known, subForSession(id)]
                : await ensureSub(route!.command, cwd)
            sub.pendingOwners += 1
            try {
                let result
                try {
                    result = await sub.connection.loadSession(id, cwd)
                } catch (error) {
                    throw route ? withRouteOnAuthError(error, route) : error
                }
                sub.sessionIds.add(id)
                if (!sessionKey.has(id)) sessionKey.set(id, key)
                return result && route
                    ? { ...result, agentIdentity: identityFromRoute(route) }
                    : result
            } finally {
                sub.pendingOwners -= 1
            }
        },
        async supportsLoadSession(cwd, agentId, customCommandFingerprint) {
            // 鏡射 loadSession(id, cwd, agentId) 的路由決策：sessionId 未在本次程序被
            // 路由過（如重啟後的 restored session）時，依 agentId 選 sub（無 agentId
            // 則沿用目前預設 command）。
            const route = await routeFor(agentId, customCommandFingerprint)
            const [, sub] = await ensureSub(route.command, cwd)
            return (await sub.connection.supportsLoadSession?.(cwd)) ?? false
        },
        async listSessions(cwd): Promise<SessionMeta[]> {
            const all = await Promise.all([...subs.values()].map((sub) => sub.connection.listSessions(cwd)))
            return all.flat()
        },
        async prompt(sessionId, blocks: PromptBlock[]): Promise<StopReason> {
            return subForSession(sessionId).connection.prompt(sessionId, blocks)
        },
        supportsImagePrompt(sessionId) {
            // 未知 session（如尚未 respawn 的 restored session）視同不支援：
            // composer 以 feature detection 隱藏入口，而非猜測（C3）。
            try {
                return subForSession(sessionId).connection.supportsImagePrompt?.(sessionId) ?? false
            } catch {
                return false
            }
        },
        async cancel(sessionId) {
            if (!sessionKey.has(sessionId)) throw new Error(`Unknown session ${sessionId}`)
            await subForSession(sessionId).connection.cancel(sessionId)
        },
        async setSessionConfigOption(
            sessionId: string,
            configId: string,
            value: SessionConfigValue
        ): Promise<SessionConfigOption[]> {
            const sub = subForSession(sessionId)
            if (!sub.connection.setSessionConfigOption) {
                throw new Error("Agent connection does not support session config options")
            }
            return sub.connection.setSessionConfigOption(sessionId, configId, value)
        },
        retryCooldownUntil(cwd, agentId) {
            const route = resolveAgentCommandRoute(agentId)
            return retryStates.get(`${route.command}${SEP}${cwd}`)?.nextAllowedAt ?? 0
        },
        async disposePrepared(cwd) {
            let disposed = false
            for (const [key, sub] of [...subs]) {
                if ((cwd && sub.cwd !== cwd) || sub.sessionIds.size > 0 || sub.pendingOwners > 0) continue
                const didDispose = await sub.connection.disposePrepared?.(sub.cwd) ?? false
                if (!didDispose || sub.sessionIds.size > 0 || sub.pendingOwners > 0) continue
                subs.delete(key)
                disposed = true
            }
            return disposed
        },
        dropSession(sessionId) {
            // F10：session 找不到對應 sub（如從未路由過）就靜默略過。
            const key = sessionKey.get(sessionId)
            const sub = key ? subs.get(key) : undefined
            sub?.connection.dropSession?.(sessionId)
            sub?.sessionIds.delete(sessionId)
            sessionKey.delete(sessionId)
        },
        async ownedProcessIds() {
            const ids = await Promise.all(
                [...subs.values()].map((sub) => sub.connection.processId?.())
            )
            return ids.filter((id): id is string => Boolean(id))
        }
    }
}

function identityFromRoute(route: AgentCommandResolution): AgentCommandIdentity {
    return {
        selectedPreset: route.selectedPreset,
        commandMode: route.commandMode,
        trustedAgentId: route.trustedAgentId
    }
}

function withRouteOnAuthError(error: unknown, route: AgentCommandResolution): unknown {
    if (!isAgentAuthRequiredError(error)) return error
    return new AgentAuthRequiredError({
        authMethods: error.authMethods,
        cwd: error.cwd,
        sessionId: error.sessionId,
        agentCommand: route.command,
        agentIdentity: identityFromRoute(route),
        cause: error
    })
}

function withTeardownGate(teardown: Promise<unknown>, ms: number): Promise<void> {
    let timeout: ReturnType<typeof setTimeout> | undefined
    const timeoutPromise = new Promise<void>((resolve) => {
        timeout = setTimeout(resolve, ms)
    })
    return Promise.race([
        teardown.then(() => undefined, () => undefined),
        timeoutPromise
    ]).finally(() => {
        if (timeout) clearTimeout(timeout)
    })
}
