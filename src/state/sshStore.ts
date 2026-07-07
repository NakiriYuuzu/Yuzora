import { create } from "zustand"

import { sshConnect, sshDisconnect } from "@/lib/ipc"
import type { SshAuthInput, SshAuthKind } from "@/lib/types"

// Persisted host book. Secrets (password / key passphrase) are NEVER stored —
// only the non-sensitive connection descriptor lands in localStorage.
export const SSH_HOSTS_STORAGE_KEY = "yuzora.ssh.hosts.v1"

export interface SshHost {
    id: string
    name: string
    host: string
    port: number
    user: string
    authKind: SshAuthKind
    /** Path to the private key file; only meaningful when authKind === "key". */
    keyPath?: string
}

/** Fields the "New host" form provides (everything but the generated id). */
export type NewSshHost = Omit<SshHost, "id">

export type SshSessionStatus = "connecting" | "connected" | "error" | "disconnected"

export interface SshSessionState {
    hostId: string
    sessionId: string | null
    status: SshSessionStatus
    fingerprint: string | null
    /** True when the host key matched a previously-pinned known-hosts entry. */
    knownHost: boolean
    error: string | null
}

interface SshStore {
    hosts: SshHost[]
    /** Session state keyed by host id (one live session per host in the MVP). */
    sessions: Record<string, SshSessionState>
    activeHostId: string | null
    /** Host awaiting a password prompt (password auth only). */
    pendingAuthHostId: string | null

    addHost: (input: NewSshHost) => SshHost
    /** Edit an existing host descriptor in place. Live sessions are untouched —
     * the new settings apply on the next connect. */
    updateHost: (id: string, input: NewSshHost) => void
    removeHost: (id: string) => void
    /** Row click / cmOpenSsh entry point: prompts for a password or connects. */
    beginConnect: (id: string) => void
    connect: (id: string, secret?: string) => Promise<void>
    cancelPendingAuth: () => void
    disconnect: (id: string) => Promise<void>
    setActiveHost: (id: string) => void
    /** Called when a shell's ssh://exit fires — mark the session dead. */
    markExit: (sessionId: string) => void
    reset: () => void
}

function newHostId(): string {
    const c = globalThis.crypto
    if (c && typeof c.randomUUID === "function") return `ssh-${c.randomUUID()}`
    return `ssh-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`
}

function isSshHost(value: unknown): value is SshHost {
    if (typeof value !== "object" || value === null) return false
    const v = value as Record<string, unknown>
    return (
        typeof v.id === "string" &&
        typeof v.name === "string" &&
        typeof v.host === "string" &&
        typeof v.port === "number" &&
        typeof v.user === "string" &&
        (v.authKind === "password" || v.authKind === "key")
    )
}

export function loadSshHosts(): SshHost[] {
    try {
        const raw = localStorage.getItem(SSH_HOSTS_STORAGE_KEY)
        if (!raw) return []
        const parsed = JSON.parse(raw)
        if (!Array.isArray(parsed)) return []
        return parsed.filter(isSshHost).map(sanitizeHost)
    } catch {
        return []
    }
}

// Re-project through the field whitelist so nothing outside the descriptor
// (in particular any secret) can survive a load/save round-trip.
function sanitizeHost(h: SshHost): SshHost {
    return {
        id: h.id,
        name: h.name,
        host: h.host,
        port: h.port,
        user: h.user,
        authKind: h.authKind,
        ...(h.authKind === "key" && h.keyPath ? { keyPath: h.keyPath } : {})
    }
}

function saveSshHosts(hosts: SshHost[]): void {
    try {
        localStorage.setItem(SSH_HOSTS_STORAGE_KEY, JSON.stringify(hosts.map(sanitizeHost)))
    } catch {
        // private mode / quota — in-memory list stays authoritative
    }
}

function buildAuth(host: SshHost, secret?: string): SshAuthInput {
    if (host.authKind === "password") {
        return { kind: "password", password: secret ?? "" }
    }
    return {
        kind: "key",
        keyPath: host.keyPath ?? "",
        passphrase: secret && secret.length > 0 ? secret : undefined
    }
}

export const useSshStore = create<SshStore>()((set, get) => ({
    hosts: loadSshHosts(),
    sessions: {},
    activeHostId: null,
    pendingAuthHostId: null,

    addHost: (input) => {
        const host = sanitizeHost({ id: newHostId(), ...input })
        set((s) => {
            const hosts = [...s.hosts, host]
            saveSshHosts(hosts)
            return { hosts }
        })
        return host
    },

    updateHost: (id, input) => {
        set((s) => {
            const hosts = s.hosts.map((h) => (h.id === id ? sanitizeHost({ id, ...input }) : h))
            saveSshHosts(hosts)
            return { hosts }
        })
    },

    removeHost: (id) => {
        const session = get().sessions[id]
        if (session?.sessionId) void sshDisconnect(session.sessionId).catch(() => undefined)
        set((s) => {
            const hosts = s.hosts.filter((h) => h.id !== id)
            saveSshHosts(hosts)
            const sessions = { ...s.sessions }
            delete sessions[id]
            return {
                hosts,
                sessions,
                activeHostId: s.activeHostId === id ? null : s.activeHostId,
                pendingAuthHostId: s.pendingAuthHostId === id ? null : s.pendingAuthHostId
            }
        })
    },

    beginConnect: (id) => {
        const host = get().hosts.find((h) => h.id === id)
        if (!host) return
        const existing = get().sessions[id]
        if (existing?.status === "connecting") return
        if (existing?.status === "connected") {
            get().setActiveHost(id)
            return
        }
        if (host.authKind === "password") {
            // Password is prompted per connection and never persisted.
            set({ pendingAuthHostId: id })
        } else {
            void get().connect(id)
        }
    },

    connect: async (id, secret) => {
        const host = get().hosts.find((h) => h.id === id)
        if (!host) return
        const auth = buildAuth(host, secret)
        set((s) => ({
            // Focus the host as soon as the attempt starts so the panel reflects
            // connecting → connected/error instead of the empty state.
            activeHostId: id,
            pendingAuthHostId: s.pendingAuthHostId === id ? null : s.pendingAuthHostId,
            sessions: {
                ...s.sessions,
                [id]: {
                    hostId: id,
                    sessionId: null,
                    status: "connecting",
                    fingerprint: null,
                    knownHost: false,
                    error: null
                }
            }
        }))
        try {
            const res = await sshConnect(host.host, host.port, host.user, auth)
            set((s) => ({
                activeHostId: id,
                sessions: {
                    ...s.sessions,
                    [id]: {
                        hostId: id,
                        sessionId: res.sessionId,
                        status: "connected",
                        fingerprint: res.fingerprint,
                        knownHost: res.knownHost ?? false,
                        error: null
                    }
                }
            }))
        } catch (e) {
            set((s) => ({
                sessions: {
                    ...s.sessions,
                    [id]: {
                        hostId: id,
                        sessionId: null,
                        status: "error",
                        fingerprint: null,
                        knownHost: false,
                        error: String(e)
                    }
                }
            }))
        }
    },

    cancelPendingAuth: () => set({ pendingAuthHostId: null }),

    disconnect: async (id) => {
        const session = get().sessions[id]
        if (session?.sessionId) {
            try {
                await sshDisconnect(session.sessionId)
            } catch {
                // Best-effort: mark the UI disconnected even if the backend call errored.
            }
        }
        set((s) => {
            const prev = s.sessions[id]
            if (!prev) return {}
            return {
                sessions: {
                    ...s.sessions,
                    [id]: { ...prev, status: "disconnected", sessionId: null }
                }
            }
        })
    },

    setActiveHost: (id) => {
        if (!get().hosts.some((h) => h.id === id)) return
        set({ activeHostId: id })
    },

    markExit: (sessionId) => {
        const entry = Object.values(get().sessions).find((sess) => sess.sessionId === sessionId)
        if (!entry) return
        // The shell ended server-side; free the now-idle SSH handle in the
        // backend so it doesn't linger until app exit (best-effort).
        void sshDisconnect(sessionId).catch(() => undefined)
        set((s) => {
            const prev = s.sessions[entry.hostId]
            if (!prev || prev.sessionId !== sessionId) return {}
            return {
                sessions: {
                    ...s.sessions,
                    [entry.hostId]: { ...prev, status: "disconnected", sessionId: null }
                }
            }
        })
    },

    reset: () =>
        set({ hosts: loadSshHosts(), sessions: {}, activeHostId: null, pendingAuthHostId: null })
}))
