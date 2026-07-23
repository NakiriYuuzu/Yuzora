import { create } from "zustand"

import {
    sftpDownload,
    sftpListDir,
    sftpMkdir,
    sftpRemove,
    sftpRename,
    sftpUpload
} from "@/lib/ipc"
import type { SftpEntry, SftpProgressEvent } from "@/lib/types"
import { useSshStore } from "./sshStore"

// Which pane of the SSH panel is showing. cmOpenSftp flips this to "sftp" so the
// context-menu entry lands the user on the browser; the SSH terminal tab stays
// the default.
type SshPanelTab = "ssh" | "sftp"

export interface RemotePaneState {
    cwd: string
    entries: SftpEntry[]
    loading: boolean
    error: string | null
}

type TransferDirection = "upload" | "download"

export interface TransferState {
    hostId: string
    direction: TransferDirection
    name: string
    transferred: number
    total: number
    done: boolean
    error: string | null
}

interface SftpStore {
    activeTab: SshPanelTab
    /** Remote listing per host id (the SFTP subsystem rides the host's session). */
    remote: Record<string, RemotePaneState>
    /** In-flight / finished transfers keyed by the front-end-minted transferId. */
    transfers: Record<string, TransferState>

    setActiveTab: (tab: SshPanelTab) => void
    /** cmOpenSftp entry point: reveal the SFTP tab and (re)connect the host. */
    openSftp: (hostId: string) => void
    listRemote: (hostId: string, path: string) => Promise<void>
    navigateUp: (hostId: string) => Promise<void>
    mkdir: (hostId: string, name: string) => Promise<void>
    rename: (hostId: string, entry: SftpEntry, newName: string) => Promise<void>
    remove: (hostId: string, entry: SftpEntry) => Promise<void>
    // destDir（可選）：拖放到遠端資料夾 row 時的目標目錄；預設遠端 cwd。
    upload: (hostId: string, localPath: string, destDir?: string) => Promise<void>
    download: (hostId: string, entry: SftpEntry, localPath: string) => Promise<void>
    applyProgress: (evt: SftpProgressEvent) => void
    clearTransfer: (transferId: string) => void
    reset: () => void
}

// SFTP always speaks POSIX paths on the wire regardless of the local platform.
export function remoteJoin(dir: string, name: string): string {
    if (dir === "" || dir === "/") return `/${name}`
    return dir.endsWith("/") ? `${dir}${name}` : `${dir}/${name}`
}

// Leaf name of a local OR remote path (handles both separators so it also works
// on Windows local paths coming out of the file picker).
export function baseName(path: string): string {
    const parts = path.split(/[\\/]/).filter((p) => p.length > 0)
    return parts.length > 0 ? parts[parts.length - 1] : path
}

// Retina hit-test: the drag-drop event reports a PhysicalPosition, so divide by
// devicePixelRatio before comparing against a DOM getBoundingClientRect (logical
// px) — otherwise the hit region is offset on any HiDPI display.
export function physicalPointInRect(
    pos: { x: number; y: number },
    rect: { left: number; top: number; right: number; bottom: number },
    dpr: number
): boolean {
    const x = pos.x / dpr
    const y = pos.y / dpr
    return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom
}

function newTransferId(): string {
    const c = globalThis.crypto
    if (c && typeof c.randomUUID === "function") return `xfer-${c.randomUUID()}`
    return `xfer-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`
}

// The live session id for a host, or null when it is not currently connected.
function sessionIdOf(hostId: string): string | null {
    return useSshStore.getState().sessions[hostId]?.sessionId ?? null
}

export const useSftpStore = create<SftpStore>()((set, get) => ({
    activeTab: "ssh",
    remote: {},
    transfers: {},

    setActiveTab: (tab) => set({ activeTab: tab }),

    openSftp: (hostId) => {
        set({ activeTab: "sftp" })
        useSshStore.getState().setActiveHost(hostId)
        useSshStore.getState().beginConnect(hostId)
    },

    listRemote: async (hostId, path) => {
        const sessionId = sessionIdOf(hostId)
        if (!sessionId) {
            set((s) => ({
                remote: {
                    ...s.remote,
                    [hostId]: {
                        cwd: s.remote[hostId]?.cwd ?? "",
                        entries: s.remote[hostId]?.entries ?? [],
                        loading: false,
                        error: "尚未連線"
                    }
                }
            }))
            return
        }
        set((s) => ({
            remote: {
                ...s.remote,
                [hostId]: {
                    cwd: s.remote[hostId]?.cwd ?? "",
                    entries: s.remote[hostId]?.entries ?? [],
                    loading: true,
                    error: null
                }
            }
        }))
        try {
            const listing = await sftpListDir(sessionId, path)
            set((s) => ({
                remote: {
                    ...s.remote,
                    [hostId]: {
                        cwd: listing.cwd,
                        entries: listing.entries,
                        loading: false,
                        error: null
                    }
                }
            }))
        } catch (e) {
            set((s) => ({
                remote: {
                    ...s.remote,
                    [hostId]: {
                        cwd: s.remote[hostId]?.cwd ?? "",
                        entries: s.remote[hostId]?.entries ?? [],
                        loading: false,
                        error: String(e)
                    }
                }
            }))
        }
    },

    navigateUp: async (hostId) => {
        const cwd = get().remote[hostId]?.cwd
        if (!cwd) return
        // The backend canonicalizes, so a trailing "/.." resolves to the parent.
        await get().listRemote(hostId, remoteJoin(cwd, ".."))
    },

    mkdir: async (hostId, name) => {
        const sessionId = sessionIdOf(hostId)
        const cwd = get().remote[hostId]?.cwd
        if (!sessionId || cwd === undefined) return
        await sftpMkdir(sessionId, remoteJoin(cwd, name))
        await get().listRemote(hostId, cwd)
    },

    rename: async (hostId, entry, newName) => {
        const sessionId = sessionIdOf(hostId)
        const cwd = get().remote[hostId]?.cwd
        if (!sessionId || cwd === undefined) return
        await sftpRename(sessionId, entry.path, remoteJoin(cwd, newName))
        await get().listRemote(hostId, cwd)
    },

    remove: async (hostId, entry) => {
        const sessionId = sessionIdOf(hostId)
        const cwd = get().remote[hostId]?.cwd
        if (!sessionId || cwd === undefined) return
        await sftpRemove(sessionId, entry.path, entry.isDir)
        await get().listRemote(hostId, cwd)
    },

    upload: async (hostId, localPath, destDir) => {
        const sessionId = sessionIdOf(hostId)
        const cwd = destDir || get().remote[hostId]?.cwd
        // Reject an empty cwd too: the loading placeholder seeds cwd:"" before the
        // first listing resolves, and remoteJoin("", name) would target the remote
        // root "/" instead of the real home directory.
        if (!sessionId || !cwd) return
        const transferId = newTransferId()
        set((s) => ({
            transfers: {
                ...s.transfers,
                [transferId]: {
                    hostId,
                    direction: "upload",
                    name: baseName(localPath),
                    transferred: 0,
                    total: 0,
                    done: false,
                    error: null
                }
            }
        }))
        try {
            await sftpUpload(sessionId, transferId, localPath, cwd)
            get().applyProgress({ sessionId, transferId, transferred: 0, total: 0, done: true })
            // Refresh the pane the user is looking at — uploading into a folder
            // row (destDir) must not navigate the view into that folder.
            const viewCwd = get().remote[hostId]?.cwd
            if (viewCwd) await get().listRemote(hostId, viewCwd)
        } catch (e) {
            set((s) => {
                const prev = s.transfers[transferId]
                if (!prev) return {}
                return {
                    transfers: {
                        ...s.transfers,
                        [transferId]: { ...prev, done: true, error: String(e) }
                    }
                }
            })
        }
    },

    download: async (hostId, entry, localPath) => {
        const sessionId = sessionIdOf(hostId)
        if (!sessionId) return
        const transferId = newTransferId()
        set((s) => ({
            transfers: {
                ...s.transfers,
                [transferId]: {
                    hostId,
                    direction: "download",
                    name: entry.name,
                    transferred: 0,
                    total: entry.size,
                    done: false,
                    error: null
                }
            }
        }))
        try {
            await sftpDownload(sessionId, transferId, entry.path, localPath)
            get().applyProgress({ sessionId, transferId, transferred: 0, total: 0, done: true })
        } catch (e) {
            set((s) => {
                const prev = s.transfers[transferId]
                if (!prev) return {}
                return {
                    transfers: {
                        ...s.transfers,
                        [transferId]: { ...prev, done: true, error: String(e) }
                    }
                }
            })
        }
    },

    applyProgress: (evt) => {
        set((s) => {
            const prev = s.transfers[evt.transferId]
            if (!prev) return {}
            return {
                transfers: {
                    ...s.transfers,
                    [evt.transferId]: {
                        ...prev,
                        transferred: evt.transferred,
                        // A terminal tick carries total 0 (upload) — keep the known total.
                        total: evt.total > 0 ? evt.total : prev.total,
                        done: evt.done || prev.done
                    }
                }
            }
        })
    },

    clearTransfer: (transferId) =>
        set((s) => {
            const transfers = { ...s.transfers }
            delete transfers[transferId]
            return { transfers }
        }),

    reset: () => set({ activeTab: "ssh", remote: {}, transfers: {} })
}))
