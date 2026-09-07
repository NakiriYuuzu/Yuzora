import { create } from "zustand"
import { connectHost, disconnectHost, prepareHost, requestHost } from "@/lib/hostIpc"
import type { ConnectedHost, HostTarget } from "@/lib/hostIpc"
import { registerRuntimeHost, unregisterRuntimeHost } from "@/lib/herdrProvider"
import { useSshStore } from "./sshStore"

interface HostConfig { hostId: string; label: string; kind: "ssh" | "wsl"; distro?: string; helper: string; binary: string }
interface HostStatus { connection: ConnectedHost | null; connecting: boolean; error: string | null; target: HostTarget; attempt: number; retryAt: number }
const STORAGE = "yuzora.runtime.hosts.v1"
function load(): Record<string, HostConfig> {
  try {
    const rows: unknown = JSON.parse(window.localStorage.getItem(STORAGE) ?? "[]")
    if (!Array.isArray(rows)) return {}
    return Object.fromEntries(rows.slice(0, 32).flatMap((row: Partial<HostConfig>) => {
      if (!row || typeof row.hostId !== "string" || typeof row.label !== "string" || typeof row.helper !== "string" || typeof row.binary !== "string") return []
      if (row.kind !== "ssh" && row.kind !== "wsl") return []
      if (row.kind === "wsl" && typeof row.distro !== "string") return []
      return [[row.hostId, { hostId:row.hostId, label:row.label, kind:row.kind, distro:row.distro, helper:row.helper, binary:row.binary }]]
    }))
  } catch { return {} }
}
interface HostState {
  configs: Record<string, HostConfig>
  hosts: Record<string, HostStatus>
  setup: (hostId: string, label: string, target: HostTarget, useManaged?: boolean) => Promise<ConnectedHost>
  reconcile: () => void
  disconnect: (hostId: string) => Promise<void>
}
interface HostOperation { kind: "setup" | "reconcile"; settled: Promise<void>; finish: () => void }
const pending = new Map<string, HostOperation>()
function begin(hostId: string, kind: HostOperation["kind"]): HostOperation {
  let finish!: () => void
  const settled = new Promise<void>((resolve) => { finish = resolve })
  const operation = { kind, settled, finish }
  pending.set(hostId, operation)
  return operation
}
const paused = new Set<string>()

export const useHostStore = create<HostState>((set, get) => {
  const close = async (hostId: string) => {
    const current = get().hosts[hostId]
    if (!current?.connection) return
    unregisterRuntimeHost(current.connection.owner)
    set((state) => ({ hosts: { ...state.hosts, [hostId]: { ...current, connection: null } } }))
    await disconnectHost(current.connection.owner).catch(() => undefined)
  }
  const current = (hostId: string, token: HostOperation, target: HostTarget, config?: HostConfig) =>
    pending.get(hostId) === token && !paused.has(hostId)
    && (!config || get().configs[hostId] === config)
    && (target.kind !== "ssh" || useSshStore.getState().sessions[hostId]?.sessionId === target.sessionId)
  const update = (hostId: string, status: HostStatus) =>
    set((state) => ({ hosts: { ...state.hosts, [hostId]: status } }))
  const done = (hostId: string, token: HostOperation) => {
    if (pending.get(hostId) === token) pending.delete(hostId)
    token.finish()
  }
  const restoreFiles = (connection: ConnectedHost) => {
    const isCurrent = () => get().hosts[connection.owner.hostId]?.connection === connection
    void import("@/lib/remoteFiles").then((remote) => {
      if (isCurrent()) return remote.reconnectRemoteWorkspaces(connection.owner, isCurrent)
    }).catch((error) => console.warn("remote workspace reconnect failed", error))
  }
  return {
    configs: load(), hosts: {},
    async setup(hostId, label, target, useManaged = false) {
      if (target.kind === "local") throw new Error("Native hosts already use the local runtime")
      const background = pending.get(hostId)
      if (background?.kind === "setup") throw new Error("Host setup is already in progress")
      paused.delete(hostId)
      // Reserve setup now so health checks cannot starve a user action. Let
      // the previous operation release its resources before replacing them.
      const token = begin(hostId, "setup")
      let preparedConnection: ConnectedHost | null = null
      try {
        await background?.settled
        if (!current(hostId, token, target)) throw new Error("Host setup was cancelled or its identity changed")
        await close(hostId)
        if (!current(hostId, token, target)) throw new Error("Host setup was cancelled or its identity changed")
        update(hostId, { connection: null, connecting: true, error: null, target, attempt: 0, retryAt: 0 })
        const prepared = await prepareHost(hostId, target, useManaged)
        preparedConnection = prepared.connection
        if (!current(hostId, token, target)) throw new Error("Host setup was cancelled or its identity changed")
        const config: HostConfig = { hostId, label, kind: target.kind, ...(target.kind === "wsl" ? { distro: target.distro } : {}), binary: prepared.binary, helper: prepared.helper }
        const configs = { ...get().configs, [hostId]: config }
        window.localStorage.setItem(STORAGE, JSON.stringify(Object.values(configs)))
        registerRuntimeHost(prepared.connection, prepared.binary, label)
        set({ configs })
        update(hostId, { connection: prepared.connection, connecting: false, error: null, target, attempt: 0, retryAt: 0 })
        restoreFiles(prepared.connection)
        return prepared.connection
      } catch (error) {
        if (preparedConnection) await disconnectHost(preparedConnection.owner).catch(() => undefined)
        if (current(hostId, token, target)) update(hostId, { connection: null, connecting: false, error: String(error), target, attempt: 1, retryAt: Date.now() + 4000 })
        throw error
      } finally { done(hostId, token) }
    },
    async disconnect(hostId) {
      paused.add(hostId)
      pending.delete(hostId)
      const previous = get().hosts[hostId]
      await close(hostId)
      if (paused.has(hostId) && previous && !get().hosts[hostId]?.connection) update(hostId, { ...previous, connection: null, connecting: false, retryAt: 0 })
    },
    reconcile() {
      for (let config of Object.values(get().configs)) {
        const hostId = config.hostId
        if (pending.has(hostId) || paused.has(hostId)) continue
        const descriptor = useSshStore.getState().hosts.find((host) => host.id === hostId)
        if (config.kind === "ssh" && descriptor && descriptor.name !== config.label) {
          config = { ...config, label: descriptor.name }
          const configs = { ...get().configs, [hostId]: config }
          set({ configs })
          try { window.localStorage.setItem(STORAGE, JSON.stringify(Object.values(configs))) } catch { /* Keep the current display name in memory. */ }
          const connection = get().hosts[hostId]?.connection
          if (connection) registerRuntimeHost(connection, config.binary, config.label)
        }
        const sshSession = useSshStore.getState().sessions[hostId]
        const target: HostTarget | null = config.kind === "wsl"
          ? { kind: "wsl", distro: config.distro! }
          : sshSession?.status === "connected" && sshSession.sessionId ? { kind: "ssh", sessionId: sshSession.sessionId } : null
        const previous = get().hosts[hostId]
        if (!target) { if (previous?.connection) void close(hostId); continue }
        if (previous?.retryAt && previous.retryAt > Date.now()) continue
        const token = begin(hostId, "reconcile")
        void (async () => {
          let opened: ConnectedHost | null = null
          try {
            if (previous?.connection && JSON.stringify(previous.target) === JSON.stringify(target)) {
              try { await requestHost(previous.connection.owner, { method: "hello" }) }
              catch (error) {
                // A full request queue has not touched the transport. Keep its
                // generation and try the health check on the next interval.
                if (!["host-request-limit", "host-request-wait-timeout"].includes(error instanceof Error ? error.message : String(error))) throw error
              }
              return
            }
            await close(hostId)
            if (!current(hostId, token, target, config)) return
            update(hostId, { connection: null, connecting: true, error: null, target, attempt: previous?.attempt ?? 0, retryAt: 0 })
            opened = await connectHost(hostId, target, config.helper)
            if (!current(hostId, token, target, config)) {
              await disconnectHost(opened.owner).catch(() => undefined)
              return
            }
            registerRuntimeHost(opened, config.binary, config.label)
            update(hostId, { connection: opened, connecting: false, error: null, target, attempt: 0, retryAt: 0 })
            restoreFiles(opened)
          } catch (error) {
            if (opened) await disconnectHost(opened.owner).catch(() => undefined)
            if (!current(hostId, token, target, config)) return
            await close(hostId)
            if (!current(hostId, token, target, config)) return
            const attempt = (previous?.attempt ?? 0) + 1
            update(hostId, { connection: null, connecting: false, error: String(error), target, attempt, retryAt: Date.now() + Math.min(30000, 1000 * 2 ** Math.min(attempt, 5)) })
          } finally { done(hostId, token) }
        })()
      }
    }
  }
})
