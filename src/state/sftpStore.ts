import { create } from "zustand"

import {
    sftpDownload,
    sftpFileRevision,
    sftpTransferPrepare,
    sftpTransferCancel,
    sftpListDir,
    sftpMkdir,
    sftpRemove,
    sftpRename,
    sftpUpload,
    sftpTransferTree
} from "@/lib/ipc"
import type { SftpDownloadDest, SftpEntry, SftpProgressEvent, SftpUploadSource } from "@/lib/types"
import { useSshStore } from "./sshStore"
import { requestAppConfirmation, useAppDialogStore } from "./appDialogStore"
import i18n from "@/lib/i18n"

export interface RemotePaneState {
    cwd: string
    entries: SftpEntry[]
    loading: boolean
    error: string | null
}

type TransferDirection = "upload" | "download"

export interface TransferState {
    hostId: string
    sessionId: string
    cancelling?: boolean
    direction: TransferDirection
    name: string
    transferred: number
    total: number
    done: boolean
    error: string | null
}

interface SftpStore {
    panelOpen: boolean
    /** Remote listing per host id (the SFTP subsystem rides the host's session). */
    remote: Record<string, RemotePaneState>
    /** In-flight / finished transfers keyed by the backend reservation. */
    transfers: Record<string, TransferState>

    setPanelOpen: (open: boolean) => void
    /** cmOpenSftp entry point: reveal the SFTP tab and (re)connect the host. */
    openSftp: (hostId: string) => void
    /** Omit path to refresh the latest requested directory, including pending navigation. */
    listRemote: (hostId: string, path?: string) => Promise<void>
    navigateUp: (hostId: string) => Promise<void>
    mkdir: (hostId: string, name: string) => Promise<void>
    rename: (hostId: string, entry: SftpEntry, newName: string) => Promise<void>
    remove: (hostId: string, entry: SftpEntry) => Promise<void>
    // destDir（可選）：拖放到遠端資料夾 row 時的目標目錄；預設遠端 cwd。
    upload: (hostId: string, source: SftpUploadSource, destDir?: string) => Promise<void>
    download: (hostId: string, entry: SftpEntry, dest: SftpDownloadDest) => Promise<void>
    transferTree: (hostId: string, selectionId: string, direction: TransferDirection, remotePath: string, name: string) => Promise<void>
    applyProgress: (evt: SftpProgressEvent) => void
    clearTransfer: (transferId: string) => void
    cancelTransfer: (transferId: string) => Promise<void>
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

async function reserveTransfer(sessionId: string): Promise<{ id: string; error: string | null }> {
    try { return { id: await sftpTransferPrepare(sessionId), error: null } }
    catch (error) { return { id: newTransferId(), error: String(error) } }
}

const listings = new Map<string, { sessionId: string | null; path: string }>()
let transferEpoch = 0
// The app has one confirmation surface. Serialize SFTP prompts, while letting
// approved transfers and uploads without an existing destination run in parallel.
let overwriteConfirmations: Promise<void> = Promise.resolve()
let activeOverwritePrompt: ReturnType<typeof useAppDialogStore.getState>["pending"] = null

function waitForOverwriteSurface(assertCurrent: () => void): Promise<void> {
    return new Promise((resolve, reject) => {
        const check = () => {
            try {
                assertCurrent()
                if (useAppDialogStore.getState().pending) return
            } catch (error) {
                for (const unsubscribe of unsubscribers) unsubscribe()
                reject(error)
                return
            }
            for (const unsubscribe of unsubscribers) unsubscribe()
            resolve()
        }
        const unsubscribers = [
            useAppDialogStore.subscribe(check),
            useSftpStore.subscribe(check),
            useSshStore.subscribe(check)
        ]
        check()
    })
}

export const useSftpStore = create<SftpStore>()((set, get) => ({
    panelOpen: false,
    remote: {},
    transfers: {},

    setPanelOpen: (open) => set({ panelOpen: open }),

    openSftp: (hostId) => {
        set({ panelOpen: true })
        useSshStore.getState().setActiveHost(hostId)
        useSshStore.getState().beginConnect(hostId)
    },

    listRemote: async (hostId, path) => {
        const sessionId = sessionIdOf(hostId)
        // A stale UI callback after profile removal must not recreate its cache.
        if (!sessionId && !useSshStore.getState().hosts.some(host => host.id === hostId)) {
            listings.delete(hostId)
            return
        }
        const previous = listings.get(hostId)
        const requestedPath = path ?? (previous?.sessionId === sessionId ? previous.path : undefined)
            ?? get().remote[hostId]?.cwd ?? ""
        const token = { sessionId, path: requestedPath }
        listings.set(hostId, token)
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
            const listing = await sftpListDir(sessionId, requestedPath)
            if (listings.get(hostId) !== token || sessionIdOf(hostId) !== sessionId) return
            token.path = listing.cwd
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
            if (listings.get(hostId) !== token || sessionIdOf(hostId) !== sessionId) return
            // A failed navigation leaves the previous directory on screen.
            token.path = get().remote[hostId]?.cwd ?? ""
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
        if (!sessionId || !cwd) return
        const epoch = transferEpoch
        await sftpMkdir(sessionId, remoteJoin(cwd, name))
        if (epoch !== transferEpoch || sessionIdOf(hostId) !== sessionId || !get().remote[hostId]) return
        await get().listRemote(hostId)
    },

    rename: async (hostId, entry, newName) => {
        const sessionId = sessionIdOf(hostId)
        const cwd = get().remote[hostId]?.cwd
        if (!sessionId || cwd === undefined) return
        const epoch = transferEpoch
        const parent = entry.path.slice(0, entry.path.lastIndexOf("/")) || "/"
        await sftpRename(sessionId, entry.path, remoteJoin(parent, newName))
        if (epoch !== transferEpoch || sessionIdOf(hostId) !== sessionId || !get().remote[hostId]) return
        await get().listRemote(hostId)
    },

    remove: async (hostId, entry) => {
        const sessionId = sessionIdOf(hostId)
        const cwd = get().remote[hostId]?.cwd
        if (!sessionId || cwd === undefined) return
        const epoch = transferEpoch
        await sftpRemove(sessionId, entry.path, entry.isDir && !entry.isSymlink)
        if (epoch !== transferEpoch || sessionIdOf(hostId) !== sessionId || !get().remote[hostId]) return
        await get().listRemote(hostId)
    },

    upload: async (hostId, source, destDir) => {
        const sessionId = sessionIdOf(hostId)
        const cwd = destDir || get().remote[hostId]?.cwd
        // Reject an empty cwd too: the loading placeholder seeds cwd:"" before the
        // first listing resolves, and remoteJoin("", name) would target the remote
        // root "/" instead of the real home directory.
        if (!sessionId || !cwd) return
        const epoch = transferEpoch
        const reservation = await reserveTransfer(sessionId)
        if (epoch !== transferEpoch || sessionIdOf(hostId) !== sessionId) { if (!reservation.error) await sftpTransferCancel(sessionId, reservation.id).catch(() => undefined); return }
        const transferId = reservation.id
        const name = source.kind === "workspace" ? baseName(source.relativePath) : source.name
        set((s) => ({
            transfers: {
                ...s.transfers,
                [transferId]: {
                    hostId,
                    sessionId,
                    direction: "upload",
                    name,
                    transferred: 0,
                    total: 0,
                    done: !!reservation.error,
                    error: reservation.error
                }
            }
        }))
        if (reservation.error) return
        const assertCurrentTransfer = () => {
            if (sessionIdOf(hostId) !== sessionId) throw new Error("sftp-connection-changed")
            const transfer = get().transfers[transferId]
            if (epoch !== transferEpoch || !transfer || transfer.cancelling || transfer.done) {
                throw new Error("sftp-transfer-cancelled")
            }
        }
        try {
            assertCurrentTransfer()
            const expectedRevision = await sftpFileRevision(sessionId, remoteJoin(cwd, name), transferId)
            assertCurrentTransfer()
            if (expectedRevision !== null) {
                const confirmation = overwriteConfirmations.then(async () => {
                    assertCurrentTransfer()
                    // Another feature can supersede our previous prompt. Do not
                    // let the next queued upload supersede that feature in turn.
                    while (useAppDialogStore.getState().pending) {
                        await waitForOverwriteSurface(assertCurrentTransfer)
                        assertCurrentTransfer()
                    }
                    const response = requestAppConfirmation({
                        title: i18n.t("panels:sshPanel.sftpOverwriteRemoteTitle"),
                        description: i18n.t("panels:sshPanel.sftpOverwriteRemoteConfirm", { name }),
                        kind: "warning"
                    })
                    const prompt = useAppDialogStore.getState().pending
                    activeOverwritePrompt = prompt
                    try { return await response }
                    finally { if (activeOverwritePrompt === prompt) activeOverwritePrompt = null }
                })
                // Keep the queue usable after a retired request; the original
                // promise below still reports its error to transfer cleanup.
                overwriteConfirmations = confirmation.then(() => undefined, () => undefined)
                if (!await confirmation) throw new Error("sftp-transfer-cancelled")
            }
            assertCurrentTransfer()
            await sftpUpload(sessionId, transferId, source, cwd, expectedRevision)
            get().applyProgress({ sessionId, transferId, transferred: 0, total: 0, done: true })
            // Refresh the pane the user is looking at — uploading into a folder
            // row (destDir) must not navigate the view into that folder.
            if (epoch === transferEpoch && get().remote[hostId]?.cwd && sessionIdOf(hostId) === sessionId) await get().listRemote(hostId)
        } catch (e) {
            await sftpTransferCancel(sessionId, transferId).catch(() => undefined)
            set((s) => {
                const prev = s.transfers[transferId]
                if (!prev) return {}
                return {
                    transfers: {
                        ...s.transfers,
                        [transferId]: { ...prev, done: true, error: e instanceof Error ? e.message : String(e) }
                    }
                }
            })
        }
    },

    download: async (hostId, entry, dest) => {
        const sessionId = sessionIdOf(hostId)
        if (!sessionId) return
        const epoch = transferEpoch
        const reservation = await reserveTransfer(sessionId)
        if (epoch !== transferEpoch || sessionIdOf(hostId) !== sessionId) { if (!reservation.error) await sftpTransferCancel(sessionId, reservation.id).catch(() => undefined); return }
        const transferId = reservation.id
        set((s) => ({
            transfers: {
                ...s.transfers,
                [transferId]: {
                    hostId,
                    sessionId,
                    direction: "download",
                    name: entry.name,
                    transferred: 0,
                    total: entry.size,
                    done: !!reservation.error,
                    error: reservation.error
                }
            }
        }))
        if (reservation.error) return
        try {
            if (sessionIdOf(hostId) !== sessionId) throw new Error("sftp-connection-changed")
            await sftpDownload(sessionId, transferId, entry.path, dest)
            get().applyProgress({ sessionId, transferId, transferred: 0, total: 0, done: true })
        } catch (e) {
            await sftpTransferCancel(sessionId, transferId).catch(() => undefined)
            set((s) => {
                const prev = s.transfers[transferId]
                if (!prev) return {}
                return {
                    transfers: {
                        ...s.transfers,
                        [transferId]: { ...prev, done: true, error: e instanceof Error ? e.message : String(e) }
                    }
                }
            })
        }
    },

    transferTree: async (hostId, selectionId, direction, remotePath, name) => {
        const sessionId = sessionIdOf(hostId)
        if (!sessionId) return
        const epoch = transferEpoch
        const reservation = await reserveTransfer(sessionId)
        if (epoch !== transferEpoch || sessionIdOf(hostId) !== sessionId) {
            if (!reservation.error) await sftpTransferCancel(sessionId, reservation.id).catch(() => undefined)
            return
        }
        const transferId = reservation.id
        set((s) => ({ transfers: { ...s.transfers, [transferId]: {
            hostId, sessionId, direction, name, transferred: 0, total: 0, done: !!reservation.error, error: reservation.error
        } } }))
        if (reservation.error) return
        try {
            if (sessionIdOf(hostId) !== sessionId) throw new Error("sftp-connection-changed")
            const result = await sftpTransferTree(sessionId, { selectionId, transferId, direction, remotePath, name })
            get().applyProgress({ sessionId, transferId, transferred: result.bytes, total: result.bytes, done: true })
            if (direction === "upload" && epoch === transferEpoch && get().remote[hostId]?.cwd && sessionIdOf(hostId) === sessionId) await get().listRemote(hostId)
        } catch (error) {
            await sftpTransferCancel(sessionId, transferId).catch(() => undefined)
            set((s) => {
                const previous = s.transfers[transferId]
                return previous ? { transfers: { ...s.transfers, [transferId]: { ...previous, done: true, error: String(error instanceof Error ? error.message : error) } } } : {}
            })
        }
    },

    applyProgress: (evt) => {
        set((s) => {
            const prev = s.transfers[evt.transferId]
            if (!prev || prev.sessionId !== evt.sessionId || prev.done) return {}
            return {
                transfers: {
                    ...s.transfers,
                    [evt.transferId]: {
                        ...prev,
                        transferred: Math.max(prev.transferred, evt.transferred),
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
            if (!s.transfers[transferId]?.done) return {}
            const transfers = { ...s.transfers }
            delete transfers[transferId]
            return { transfers }
        }),

    cancelTransfer: async (transferId) => {
        const transfer = get().transfers[transferId]
        if (!transfer || transfer.done || transfer.cancelling) return
        set((s) => ({ transfers: { ...s.transfers, [transferId]: { ...transfer, cancelling: true } } }))
        try { await sftpTransferCancel(transfer.sessionId, transferId) }
        catch {
            // Completion can race Cancel; the running operation owns its outcome.
            set((s) => s.transfers[transferId] ? { transfers: { ...s.transfers, [transferId]: { ...s.transfers[transferId], cancelling: false } } } : {})
        }
    },

    reset: () => {
        transferEpoch++
        overwriteConfirmations = Promise.resolve()
        const prompt = activeOverwritePrompt
        activeOverwritePrompt = null
        if (prompt && useAppDialogStore.getState().pending === prompt) {
            useAppDialogStore.getState().respond(false)
        }
        listings.clear()
        for (const [id, transfer] of Object.entries(get().transfers)) if (!transfer.done) void sftpTransferCancel(transfer.sessionId, id).catch(() => undefined)
        set({ panelOpen: false, remote: {}, transfers: {} })
    }
}))

/** Drop projections for removed host profiles; SSH owns native cancellation. */
export function forgetSftpHosts(hostIds: readonly string[]): void {
    if (hostIds.length === 0) return
    const removed = new Set(hostIds)
    for (const hostId of removed) listings.delete(hostId)
    useSftpStore.setState((state) => {
        let remote = state.remote
        for (const hostId of removed) {
            if (!Object.hasOwn(remote, hostId)) continue
            if (remote === state.remote) remote = { ...remote }
            delete remote[hostId]
        }
        let transfers = state.transfers
        for (const [id, transfer] of Object.entries(state.transfers)) {
            if (!removed.has(transfer.hostId)) continue
            if (transfers === state.transfers) transfers = { ...transfers }
            delete transfers[id]
        }
        return remote === state.remote && transfers === state.transfers
            ? state
            : { remote, transfers }
    })
}
