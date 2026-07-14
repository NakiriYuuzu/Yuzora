import type { AgentCommandIdentity, AgentCommandResolution, AgentId } from "@/lib/agentPresets"
import { resolveAgentCommandRoute } from "@/app/workbench/settingsStorage"
import {
    AgentAuthRequiredError,
    createAcpConnection,
    isAgentAuthRequiredError,
    type AcpClientRuntimeDeps,
    type AgentConnection,
    type PromptBlock,
    type SessionConfigOption,
    type SessionConfigValue,
    type SessionMeta,
    type StopReason
} from "./acpConnection"

type SubFactory = (command: string, cwd: string) => AgentConnection

export interface AgentRouter extends AgentConnection {
    ownedProcessIds(): Promise<string[]>
}

const SEP = "\0"

interface SubState {
    command: string
    cwd: string
    connection: AgentConnection
    sessionIds: Set<string>
    pendingOwners: number
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
    deps: AcpClientRuntimeDeps,
    factory: SubFactory = (command) => createAcpConnection({ ...deps, command })
): AgentRouter {
    const subs = new Map<string, SubState>() // key = `${command}${SEP}${cwd}`
    const sessionKey = new Map<string, string>() // sessionId -> subs key (same `${command}${SEP}${cwd}` format)

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

    const ensureSub = (command: string, cwd: string): [string, SubState] => {
        const key = `${command}${SEP}${cwd}`
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
            const [, sub] = ensureSub(route.command, cwd)
            if (!sub.connection.prepare) throw new Error("Agent connection does not support prepare")
            await sub.connection.prepare(cwd)
        },
        async newSession(cwd, agentId) {
            const route = await routeFor(agentId)
            const [key, sub] = ensureSub(route.command, cwd)
            sub.pendingOwners += 1
            try {
                let result
                try {
                    result = await sub.connection.newSession(cwd, agentId)
                } catch (error) {
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
                : ensureSub(route!.command, cwd)
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
            const [, sub] = ensureSub(route.command, cwd)
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
