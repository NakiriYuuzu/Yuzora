import { beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/ipc", () => ({
    sshConnect: vi.fn(),
    sshDisconnect: vi.fn(),
    sftpListDir: vi.fn(),
    sftpMkdir: vi.fn(),
    sftpRename: vi.fn(),
    sftpRemove: vi.fn(),
    sftpUpload: vi.fn(),
    sftpDownload: vi.fn(),
    sftpTransferTree: vi.fn(),
    sftpFileRevision: vi.fn(async () => null),
    sftpTransferPrepare: vi.fn(async () => `xfer-${crypto.randomUUID()}`),
    sftpTransferCancel: vi.fn(async () => {})
}))

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
import type { SftpEntry, SftpListing } from "@/lib/types"
import {
    baseName,
    physicalPointInRect,
    remoteJoin,
    useSftpStore
} from "./sftpStore"
import { useSshStore } from "./sshStore"
import { useAppDialogStore } from "./appDialogStore"

const mockList = vi.mocked(sftpListDir)
const mockMkdir = vi.mocked(sftpMkdir)
const mockRename = vi.mocked(sftpRename)
const mockRemove = vi.mocked(sftpRemove)
const mockUpload = vi.mocked(sftpUpload)
const mockDownload = vi.mocked(sftpDownload)

// The Bun-hosted test runtime injects an empty `localStorage` with no Storage
// methods; sshStore's module init reads it, so install a minimal one.
function installLocalStorage(): void {
    const store = new Map<string, string>()
    Object.defineProperty(globalThis, "localStorage", {
        value: {
            getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
            setItem: (k: string, v: string) => void store.set(k, String(v)),
            removeItem: (k: string) => void store.delete(k),
            clear: () => store.clear(),
            key: (i: number) => [...store.keys()][i] ?? null,
            get length() {
                return store.size
            }
        },
        configurable: true,
        writable: true
    })
}

const HOST = "ssh-h1"

function file(name: string, isDir = false): SftpEntry {
    return { name, path: `/home/u/${name}`, isDir, isSymlink: false, size: 10 }
}

function listing(cwd: string, entries: SftpEntry[]): SftpListing {
    return { cwd, entries }
}

function connectHost(cwd = "/home/u"): void {
    useSshStore.setState((s) => ({
        hosts: [
            ...s.hosts,
            { id: HOST, name: "h", host: "h.example.com", port: 22, user: "u", authKind: "password" }
        ],
        sessions: {
            ...s.sessions,
            [HOST]: {
                hostId: HOST,
                sessionId: "sess-1",
                status: "connected",
                fingerprint: null,
                knownHost: false,
                error: null
            }
        },
        activeHostId: HOST
    }))
    if (cwd) {
        useSftpStore.setState((s) => ({
            remote: { ...s.remote, [HOST]: { cwd, entries: [], loading: false, error: null } }
        }))
    }
}

beforeEach(() => {
    installLocalStorage()
    localStorage.clear()
    vi.clearAllMocks()
    useSshStore.setState({ hosts: [], sessions: {}, activeHostId: null, pendingAuthHostId: null })
    useSftpStore.getState().reset()
})

describe("sftpStore pure helpers", () => {
    it("remoteJoin uses POSIX separators and handles root", () => {
        expect(remoteJoin("/home/u", "f.txt")).toBe("/home/u/f.txt")
        expect(remoteJoin("/", "f.txt")).toBe("/f.txt")
        expect(remoteJoin("", "f.txt")).toBe("/f.txt")
        expect(remoteJoin("/home/u/", "f.txt")).toBe("/home/u/f.txt")
    })

    it("baseName strips both separators", () => {
        expect(baseName("/home/u/f.txt")).toBe("f.txt")
        expect(baseName("C:\\Users\\me\\f.txt")).toBe("f.txt")
        expect(baseName("solo")).toBe("solo")
    })

    it("physicalPointInRect divides the physical position by the DPR before hit-testing", () => {
        const rect = { left: 50, top: 50, right: 150, bottom: 150 }
        // At DPR 2 a physical (200,200) maps to logical (100,100) — inside.
        expect(physicalPointInRect({ x: 200, y: 200 }, rect, 2)).toBe(true)
        // At DPR 1 the same physical point is (200,200) — outside.
        expect(physicalPointInRect({ x: 200, y: 200 }, rect, 1)).toBe(false)
    })
})

describe("sftpStore remote browsing", () => {
    it("listRemote stores the canonical cwd + entries from the backend", async () => {
        connectHost("")
        mockList.mockResolvedValueOnce(listing("/home/u", [file("a.txt"), file("dir", true)]))
        await useSftpStore.getState().listRemote(HOST, "")
        const remote = useSftpStore.getState().remote[HOST]
        expect(mockList).toHaveBeenCalledWith("sess-1", "")
        expect(remote.cwd).toBe("/home/u")
        expect(remote.entries).toHaveLength(2)
        expect(remote.loading).toBe(false)
        expect(remote.error).toBeNull()
    })

    it("listRemote records an error when the host has no live session", async () => {
        await useSftpStore.getState().listRemote(HOST, "")
        expect(mockList).not.toHaveBeenCalled()
        expect(useSftpStore.getState().remote[HOST].error).toBe("尚未連線")
    })

    it("navigateUp appends '/..' so the backend resolves the parent", async () => {
        connectHost("/home/u/sub")
        mockList.mockResolvedValueOnce(listing("/home/u", []))
        await useSftpStore.getState().navigateUp(HOST)
        expect(mockList).toHaveBeenCalledWith("sess-1", "/home/u/sub/..")
    })

    it("mkdir joins the cwd then refreshes the listing", async () => {
        connectHost("/home/u")
        mockMkdir.mockResolvedValueOnce(undefined)
        mockList.mockResolvedValueOnce(listing("/home/u", []))
        await useSftpStore.getState().mkdir(HOST, "newdir")
        expect(mockMkdir).toHaveBeenCalledWith("sess-1", "/home/u/newdir")
        expect(mockList).toHaveBeenCalledWith("sess-1", "/home/u")
    })

    it("rename targets the new name in the same cwd then refreshes", async () => {
        connectHost("/home/u")
        mockRename.mockResolvedValueOnce(undefined)
        mockList.mockResolvedValueOnce(listing("/home/u", []))
        await useSftpStore.getState().rename(HOST, file("old.txt"), "new.txt")
        expect(mockRename).toHaveBeenCalledWith("sess-1", "/home/u/old.txt", "/home/u/new.txt")
        expect(mockList).toHaveBeenCalledWith("sess-1", "/home/u")
    })

    it("remove passes the entry path + isDir then refreshes", async () => {
        connectHost("/home/u")
        mockRemove.mockResolvedValueOnce(undefined)
        mockList.mockResolvedValueOnce(listing("/home/u", []))
        await useSftpStore.getState().remove(HOST, file("dir", true))
        expect(mockRemove).toHaveBeenCalledWith("sess-1", "/home/u/dir", true)
        expect(mockList).toHaveBeenCalledWith("sess-1", "/home/u")
    })
})

describe("sftpStore transfers", () => {
    it("drops a late listing from a previous session or a superseded navigation", async () => {
        connectHost()
        let finish!: (listing: SftpListing) => void
        mockList.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve }))
        const first = useSftpStore.getState().listRemote(HOST, "/old")
        mockList.mockResolvedValueOnce(listing("/current", []))
        await useSftpStore.getState().listRemote(HOST, "/current")
        finish(listing("/old", []))
        await first
        expect(useSftpStore.getState().remote[HOST].cwd).toBe("/current")
        mockList.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve }))
        const replaced = useSftpStore.getState().listRemote(HOST, "/previous-session")
        useSshStore.setState({ sessions: {} })
        finish(listing("/previous-session", []))
        await replaced
        expect(useSftpStore.getState().remote[HOST].cwd).toBe("/current")
    })

    it("requires overwrite confirmation and sends the exact checked revision", async () => {
        connectHost()
        vi.mocked(sftpFileRevision).mockResolvedValueOnce("checked-revision")
        const uploading = useSftpStore.getState().upload(HOST, { kind: "selected", capabilityId: "picked", name: "file" })
        await vi.waitFor(() => expect(useAppDialogStore.getState().pending).not.toBeNull())
        expect(mockUpload).not.toHaveBeenCalled()
        useAppDialogStore.getState().respond(true)
        await uploading
        expect(mockUpload).toHaveBeenCalledWith("sess-1", expect.any(String), { kind: "selected", capabilityId: "picked", name: "file" }, "/home/u", "checked-revision")
    })

    it("declining an overwrite preserves the source capability and releases the transfer reservation", async () => {
        connectHost()
        vi.mocked(sftpFileRevision).mockResolvedValueOnce("checked-revision")
        const uploading = useSftpStore.getState().upload(HOST, { kind: "selected", capabilityId: "picked", name: "file" })
        await vi.waitFor(() => expect(useAppDialogStore.getState().pending).not.toBeNull())
        useAppDialogStore.getState().respond(false)
        await uploading
        expect(mockUpload).not.toHaveBeenCalled()
        const [id, transfer] = Object.entries(useSftpStore.getState().transfers)[0]
        expect(transfer).toMatchObject({ done: true, error: "sftp-transfer-cancelled" })
        expect(sftpTransferCancel).toHaveBeenCalledWith("sess-1", id)
    })

    it("cancels the owning session and ignores late or cross-session progress", async () => {
        connectHost()
        let fail!: (error: string) => void
        mockDownload.mockImplementationOnce(() => new Promise((_, reject) => { fail = reject }))
        const downloading = useSftpStore.getState().download(HOST, file("data.csv"), { capabilityId: "dest", leaf: "data.csv" })
        await vi.waitFor(() => expect(mockDownload).toHaveBeenCalled())
        const id = mockDownload.mock.calls[0][1]
        useSftpStore.getState().applyProgress({ sessionId: "different-host", transferId: id, transferred: 99, total: 100, done: true })
        expect(useSftpStore.getState().transfers[id]).toMatchObject({ done: false, transferred: 0 })
        useSftpStore.getState().applyProgress({ sessionId: "sess-1", transferId: id, transferred: 50, total: 100, done: false })
        await useSftpStore.getState().cancelTransfer(id)
        expect(sftpTransferCancel).toHaveBeenCalledWith("sess-1", id)
        fail("sftp-transfer-cancelled")
        await downloading
        useSftpStore.getState().applyProgress({ sessionId: "sess-1", transferId: id, transferred: 100, total: 100, done: true })
        expect(useSftpStore.getState().transfers[id]).toMatchObject({ done: true, transferred: 50, error: "sftp-transfer-cancelled" })
    })

    it("releases a reservation returned after reset without starting a transfer", async () => {
        connectHost()
        let finish!: (id: string) => void
        vi.mocked(sftpTransferPrepare).mockImplementationOnce(() => new Promise((resolve) => { finish = resolve }))
        const downloading = useSftpStore.getState().download(HOST, file("data.csv"), { capabilityId: "dest", leaf: "data.csv" })
        useSftpStore.getState().reset()
        finish("late")
        await downloading
        expect(sftpTransferCancel).toHaveBeenCalledWith("sess-1", "late")
        expect(mockDownload).not.toHaveBeenCalled()
        expect(useSftpStore.getState().transfers).toEqual({})
    })

    it("upload registers a transfer, calls sftp_upload with the remote cwd, and refreshes", async () => {
        connectHost("/home/u")
        mockUpload.mockResolvedValueOnce(undefined)
        mockList.mockResolvedValueOnce(listing("/home/u", []))
        await useSftpStore.getState().upload(HOST, {
            kind: "workspace",
            workspaceId: "ws-opaque",
            relativePath: "report.pdf"
        })

        const call = mockUpload.mock.calls[0]
        expect(call[0]).toBe("sess-1")
        expect(call[2]).toEqual({
            kind: "workspace",
            workspaceId: "ws-opaque",
            relativePath: "report.pdf"
        })
        expect(call[3]).toBe("/home/u")
        const transferId = call[1]
        const tr = useSftpStore.getState().transfers[transferId]
        expect(tr.direction).toBe("upload")
        expect(tr.name).toBe("report.pdf")
        expect(tr.done).toBe(true)
        expect(tr.error).toBeNull()
        expect(mockList).toHaveBeenCalledWith("sess-1", "/home/u")
    })

    it("upload marks the transfer failed when the backend rejects", async () => {
        connectHost("/home/u")
        mockUpload.mockRejectedValueOnce("寫入遠端檔案失敗：boom")
        await useSftpStore.getState().upload(HOST, {
            kind: "selected",
            capabilityId: "sel-1",
            name: "f.bin"
        })
        const tr = Object.values(useSftpStore.getState().transfers)[0]
        expect(tr.error).toContain("寫入遠端檔案失敗")
        expect(tr.done).toBe(true)
    })

    it("download registers a transfer and calls sftp_download with the picked local path", async () => {
        connectHost("/home/u")
        mockDownload.mockResolvedValueOnce(undefined)
        await useSftpStore.getState().download(HOST, file("data.csv"), {
            capabilityId: "download-1",
            leaf: "data.csv"
        })
        const call = mockDownload.mock.calls[0]
        expect(call).toEqual([
            "sess-1",
            expect.any(String),
            "/home/u/data.csv",
            { capabilityId: "download-1", leaf: "data.csv" }
        ])
        const tr = Object.values(useSftpStore.getState().transfers)[0]
        expect(tr.direction).toBe("download")
        expect(tr.done).toBe(true)
    })

    it("applyProgress updates transferred/total and preserves a known total on the terminal tick", () => {
        useSftpStore.setState({
            transfers: {
                x1: {
                    hostId: HOST, sessionId: "sess-1",
                    direction: "download",
                    name: "f",
                    transferred: 0,
                    total: 1000,
                    done: false,
                    error: null
                }
            }
        })
        useSftpStore
            .getState()
            .applyProgress({ sessionId: "sess-1", transferId: "x1", transferred: 500, total: 1000, done: false })
        expect(useSftpStore.getState().transfers.x1.transferred).toBe(500)
        // A terminal tick may carry total 0 — keep the previously-known total.
        useSftpStore
            .getState()
            .applyProgress({ sessionId: "sess-1", transferId: "x1", transferred: 500, total: 0, done: true })
        expect(useSftpStore.getState().transfers.x1.total).toBe(1000)
        expect(useSftpStore.getState().transfers.x1.done).toBe(true)
    })

    it("clearTransfer drops the entry", () => {
        useSftpStore.setState({
            transfers: {
                x1: { hostId: HOST, sessionId: "sess-1", direction: "upload", name: "f", transferred: 0, total: 0, done: true, error: null }
            }
        })
        useSftpStore.getState().clearTransfer("x1")
        expect(useSftpStore.getState().transfers.x1).toBeUndefined()
    })
})

describe("sftpStore openSftp", () => {
    it("reveals the SFTP tab and focuses/connects the host", () => {
        useSshStore.getState().addHost({
            name: "h",
            host: "h.example.com",
            port: 22,
            user: "u",
            authKind: "password"
        })
        const id = useSshStore.getState().hosts[0].id
        useSftpStore.getState().openSftp(id)
        expect(useSftpStore.getState().panelOpen).toBe(true)
        expect(useSshStore.getState().activeHostId).toBe(id)
        // Password host → begins the connect flow (pending auth prompt).
        expect(useSshStore.getState().pendingAuthHostId).toBe(id)
    })
})

describe("SFTP folder transfer ownership", () => {
    it("keeps Cancel bound to the original session and never replays a folder transfer", async () => {
        connectHost("/home/u")
        let fail!: (error: Error) => void
        vi.mocked(sftpTransferTree).mockImplementationOnce(() => new Promise((_, reject) => { fail = reject }))
        const transfer = useSftpStore.getState().transferTree(HOST, "selected-tree", "upload", "/home/u", "folder")
        await vi.waitFor(() => expect(sftpTransferTree).toHaveBeenCalledOnce())
        const [id] = Object.keys(useSftpStore.getState().transfers)
        useSshStore.setState((s) => ({ sessions: { ...s.sessions, [HOST]: { ...s.sessions[HOST], sessionId: "replacement" } } }))
        await useSftpStore.getState().cancelTransfer(id)
        expect(sftpTransferCancel).toHaveBeenCalledWith("sess-1", id)
        fail(new Error("sftp-transfer-cancelled"))
        await transfer
        expect(sftpTransferTree).toHaveBeenCalledOnce()
        expect(useSftpStore.getState().transfers[id]).toMatchObject({ done: true, error: "sftp-transfer-cancelled" })
    })
})
