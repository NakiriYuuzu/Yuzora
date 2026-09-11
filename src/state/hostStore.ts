import { create } from "zustand"
import { checkHostRuntime, connectHost, disconnectHost, prepareHost, requestHost } from "@/lib/hostIpc"
import type { ConnectedHost, HostTarget } from "@/lib/hostIpc"
import { registerRuntimeHost, unregisterRuntimeHost } from "@/lib/herdrProvider"
import { useSshStore } from "./sshStore"
import { useRuntimePreferencesStore } from "./runtimePreferencesStore"
import type { HerdrRuntimeSelection } from "@/lib/herdrTypes"

export interface HostConfig { hostId: string; label: string; kind: "ssh" | "wsl"; distro?: string; helper: string; binary: string; selection?: HerdrRuntimeSelection; artifactIdentity?: string; verifiedAt?: number }
export function selectionForHost(config?: HostConfig): HerdrRuntimeSelection {
  if (config?.selection) return config.selection
  if (!config) return { source: "default" }
  const managed = config.binary.match(/^(.*\/\.local\/share\/yuzora\/runtimes\/[^/]+-(?:linux|macos)-(?:aarch64|x86_64)-[a-f0-9]{64})\/herdr$/)
  return managed && config.helper === managed[1] + "/yuzora-host" ? { source: "default" } : { source: "custom", customPath: config.binary }
}
interface HostStatus { connection: ConnectedHost | null; connecting: boolean; error: string | null; target: HostTarget; attempt: number; retryAt: number }
const STORAGE = "yuzora.runtime.hosts.v2"
function load(): Record<string, HostConfig> {
  try {
    const rows: unknown = JSON.parse(window.localStorage.getItem(STORAGE) ?? window.localStorage.getItem("yuzora.runtime.hosts.v1") ?? "[]")
    if (!Array.isArray(rows)) return {}
    return Object.fromEntries(rows.slice(0, 32).flatMap((row: Partial<HostConfig>) => {
      if (!row || typeof row.hostId !== "string" || typeof row.label !== "string" || typeof row.helper !== "string" || typeof row.binary !== "string") return []
      if (row.kind !== "ssh" && row.kind !== "wsl") return []
      if (row.kind === "wsl" && typeof row.distro !== "string") return []
      const config: HostConfig = { hostId:row.hostId, label:row.label, kind:row.kind, distro:row.distro, helper:row.helper, binary:row.binary }
      if (row.selection?.source === "default" || row.selection?.source === "global") config.selection = { source: row.selection.source }
      else if (row.selection?.source === "custom" && typeof row.selection.customPath === "string") config.selection = { source: "custom", customPath: row.selection.customPath }
      config.selection = selectionForHost(config)
      if (typeof row.artifactIdentity === "string" && /^[a-f0-9]{64}$/.test(row.artifactIdentity)) config.artifactIdentity = row.artifactIdentity
      if (typeof row.verifiedAt === "number") config.verifiedAt = row.verifiedAt
      return [[row.hostId, config]]
    }))
  } catch { return {} }
}
interface HostState {
  configs: Record<string, HostConfig>
  hosts: Record<string, HostStatus>
  setup: (hostId: string, label: string, target: HostTarget, selection?: HerdrRuntimeSelection) => Promise<ConnectedHost>
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
    && (target.kind !== "wsl" || useRuntimePreferencesStore.getState().wslEnabled)
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
    async setup(hostId, label, target, selection = { source: "default" }) {
      if (target.kind === "local") throw new Error("Native hosts already use the local runtime")
      if (target.kind === "wsl" && !useRuntimePreferencesStore.getState().wslEnabled) throw new Error("wsl-runtime-disabled-open-settings")
      const background = pending.get(hostId)
      if (background?.kind === "setup") throw new Error("Host setup is already in progress")
      paused.delete(hostId)
      // Reserve setup now so health checks cannot starve a user action. Let
      // the previous operation release its resources before replacing them.
      const token = begin(hostId, "setup")
      let preparedConnection: ConnectedHost | null = null
      const previous = get().hosts[hostId]
      let connectionWasClosed = false
      try {
        await background?.settled
        if (!current(hostId, token, target)) throw new Error("Host setup was cancelled or its identity changed")
        const inspection = await checkHostRuntime(hostId, target, selection)
        if (inspection.check && !inspection.check.canApply) throw new Error("runtime-incompatible: " + JSON.stringify(inspection.check))
        if (!current(hostId, token, target)) throw new Error("Host setup was cancelled or its identity changed")
        update(hostId, { connection: previous?.connection ?? null, connecting: true, error: null, target, attempt: 0, retryAt: 0 })
        const prepared = await prepareHost(hostId, target, selection)
        preparedConnection = prepared.connection
        if (!current(hostId, token, target)) throw new Error("Host setup was cancelled or its identity changed")
        connectionWasClosed = true
        await close(hostId)
        if (!current(hostId, token, target)) throw new Error("Host setup was cancelled or its identity changed")
        const config: HostConfig = { hostId, label, kind: target.kind, ...(target.kind === "wsl" ? { distro: target.distro } : {}), binary: prepared.binary, helper: prepared.helper, selection: { ...selection }, artifactIdentity: prepared.artifactIdentity, verifiedAt: Date.now() }
        const configs = { ...get().configs, [hostId]: config }
        window.localStorage.setItem(STORAGE, JSON.stringify(Object.values(configs)))
        registerRuntimeHost(prepared.connection, prepared.binary, label, target.kind)
        set({ configs })
        update(hostId, { connection: prepared.connection, connecting: false, error: null, target, attempt: 0, retryAt: 0 })
        restoreFiles(prepared.connection)
        return prepared.connection
      } catch (error) {
        if (preparedConnection) await disconnectHost(preparedConnection.owner).catch(() => undefined)
        if (current(hostId, token, target)) update(hostId, { connection: !connectionWasClosed ? previous?.connection ?? null : null, connecting: false, error: String(error), target, attempt: 1, retryAt: Date.now() + 4000 })
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
        if (config.kind === "wsl" && !useRuntimePreferencesStore.getState().wslEnabled) {
          if (get().hosts[hostId]?.connection) void close(hostId)
          continue
        }
        if (pending.has(hostId) || paused.has(hostId)) continue
        const descriptor = useSshStore.getState().hosts.find((host) => host.id === hostId)
        if (config.kind === "ssh" && descriptor && descriptor.name !== config.label) {
          config = { ...config, label: descriptor.name }
          const configs = { ...get().configs, [hostId]: config }
          set({ configs })
          try { window.localStorage.setItem(STORAGE, JSON.stringify(Object.values(configs))) } catch { /* Keep the current display name in memory. */ }
          const connection = get().hosts[hostId]?.connection
          if (connection) registerRuntimeHost(connection, config.binary, config.label, config.kind)
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
            if (config.kind === "wsl") {
              // Reconnecting the saved helper does not run host_prepare's runtime
              // startup. Ask the owning host to ensure its default server exists;
              // this operation preserves running servers and never starts Agents.
              if (!opened.hello.methods.includes("herdrStart"))
                throw new Error("herdr-start-unavailable: repair the saved WSL runtime helper")
              await requestHost(opened.owner, { method: "herdrStart", params: { binary: config.binary } })
              if (!current(hostId, token, target, config)) {
                await disconnectHost(opened.owner).catch(() => undefined)
                return
              }
            }
            registerRuntimeHost(opened, config.binary, config.label, config.kind)
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
