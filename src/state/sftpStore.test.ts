import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

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
    forgetSftpHosts,
    physicalPointInRect,
    remoteJoin,
    useSftpStore
} from "./sftpStore"
import { useSshStore } from "./sshStore"
import { requestAppConfirmation, useAppDialogStore } from "./appDialogStore"

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
        connectHost("")
        useSshStore.setState({ sessions: {} })
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

describe("sftpStore mutation refresh navigation", () => {
    function deferred<T>() {
        let resolve!: (value: T) => void
        let reject!: (reason: Error) => void
        const promise = new Promise<T>((ok, fail) => { resolve = ok; reject = fail })
        return { promise, resolve, reject }
    }

    const operations = ["mkdir", "rename", "remove"] as const
    function beginMutation(operation: typeof operations[number]) {
        const pending = deferred<void>()
        const store = useSftpStore.getState()
        if (operation === "mkdir") {
            mockMkdir.mockReturnValueOnce(pending.promise)
            return { ...pending, done: store.mkdir(HOST, "new") }
        }
        if (operation === "rename") {
            mockRename.mockReturnValueOnce(pending.promise)
            return { ...pending, done: store.rename(HOST, file("old"), "new") }
        }
        mockRemove.mockReturnValueOnce(pending.promise)
        return { ...pending, done: store.remove(HOST, file("old")) }
    }

    it.each(operations.flatMap(operation => [false, true].map(completed => ({ operation, completed }))))(
        "$operation keeps B navigation when completed=$completed", async ({ operation, completed }) => {
            connectHost()
            const mutation = beginMutation(operation)
            const next = deferred<SftpListing>()
            mockList.mockReturnValueOnce(next.promise)
            mockList.mockImplementation(async (_session, path) => listing(path!, [file("fresh")]))
            const navigation = useSftpStore.getState().listRemote(HOST, "/B")
            if (completed) { next.resolve(listing("/B", [])); await navigation }
            mutation.resolve()
            await mutation.done
            if (!completed) { next.resolve(listing("/B", [file("older")])); await navigation }
            expect(useSftpStore.getState().remote[HOST].cwd).toBe("/B")
            expect(mockList.mock.calls.every(([, path]) => path === "/B")).toBe(true)
            expect(useSftpStore.getState().remote[HOST].entries.map(entry => entry.name)).toEqual(["fresh"])
            if (operation === "mkdir") expect(mockMkdir).toHaveBeenCalledWith("sess-1", "/home/u/new")
            if (operation === "rename") expect(mockRename).toHaveBeenCalledWith("sess-1", "/home/u/old", "/home/u/new")
            if (operation === "remove") expect(mockRemove).toHaveBeenCalledWith("sess-1", "/home/u/old", false)
        }
    )

    it.each([false, true])("refreshes both same-directory mutations when first refresh completed=%s", async completed => {
        connectHost()
        const first = beginMutation("mkdir")
        const second = beginMutation("rename")
        const firstList = deferred<SftpListing>()
        mockList.mockReturnValueOnce(firstList.promise)
        first.resolve()
        await vi.waitFor(() => expect(mockList).toHaveBeenCalledTimes(1))
        if (completed) { firstList.resolve(listing("/home/u", [file("first")])); await first.done }
        mockList.mockResolvedValueOnce(listing("/home/u", [file("first"), file("second")]))
        second.resolve()
        await second.done
        if (!completed) { firstList.resolve(listing("/home/u", [file("first")])); await first.done }
        expect(mockList).toHaveBeenCalledTimes(2)
        expect(useSftpStore.getState().remote[HOST].entries.map(entry => entry.name)).toEqual(["first", "second"])
    })

    it("does not let an intervening same-directory manual refresh suppress a completed mutation", async () => {
        connectHost()
        const mutation = beginMutation("mkdir")
        mockList.mockResolvedValueOnce(listing("/home/u", []))
        await useSftpStore.getState().listRemote(HOST, "/home/u")
        mockList.mockResolvedValueOnce(listing("/home/u", [file("new")]))
        mutation.resolve()
        await mutation.done
        expect(mockList).toHaveBeenCalledTimes(2)
        expect(useSftpStore.getState().remote[HOST].entries[0].name).toBe("new")
    })

    it.each(["upload", "tree"])("%s completion preserves pending B navigation", async operation => {
        connectHost()
        const transfer = deferred<{ files: number; bytes: number }>()
        if (operation === "upload") mockUpload.mockImplementationOnce(() => transfer.promise.then(() => undefined))
        else vi.mocked(sftpTransferTree).mockReturnValueOnce(transfer.promise)
        const done = operation === "upload"
            ? useSftpStore.getState().upload(HOST, { kind: "selected", capabilityId: "owned", name: "new" })
            : useSftpStore.getState().transferTree(HOST, "owned", "upload", "/home/u", "new")
        await vi.waitFor(() => expect(operation === "upload" ? mockUpload : sftpTransferTree).toHaveBeenCalledTimes(1))
        const next = deferred<SftpListing>()
        mockList.mockReturnValueOnce(next.promise)
        mockList.mockImplementation(async (_session, path) => listing(path!, []))
        const navigating = useSftpStore.getState().listRemote(HOST, "/B")
        transfer.resolve({ files: 1, bytes: 1 })
        await done
        next.resolve(listing("/B", []))
        await navigating
        expect(mockList.mock.calls.every(([, path]) => path === "/B")).toBe(true)
        expect(useSftpStore.getState().remote[HOST].cwd).toBe("/B")
    })

    it.each(operations.flatMap(operation => ["reset", "replacement", "forget"].map(retirement => ({ operation, retirement }))))(
        "$operation cannot refresh a view retired by $retirement", async ({ operation, retirement }) => {
            connectHost()
            const mutation = beginMutation(operation)
            if (retirement === "reset") {
                useSftpStore.getState().reset()
                connectHost("/new-view")
            } else if (retirement === "replacement") {
                useSshStore.setState(s => ({ sessions: { ...s.sessions, [HOST]: { ...s.sessions[HOST], sessionId: "sess-2" } } }))
            } else {
                forgetSftpHosts([HOST])
            }
            mockList.mockResolvedValue(listing("/unexpected", []))
            mutation.resolve()
            await mutation.done
            expect(mockList).not.toHaveBeenCalled()
            if (retirement === "forget") expect(useSftpStore.getState().remote[HOST]).toBeUndefined()
            if (retirement === "reset") expect(useSftpStore.getState().remote[HOST].cwd).toBe("/new-view")
        }
    )

    it("refreshes the canonical destination after navigation resolves", async () => {
        connectHost()
        const mutation = beginMutation("mkdir")
        mockList.mockResolvedValueOnce(listing("/canonical", []))
        await useSftpStore.getState().listRemote(HOST, "/alias/..")
        mockList.mockImplementation(async (_session, path) => listing(path!, []))
        mutation.resolve()
        await mutation.done
        expect(mockList).toHaveBeenLastCalledWith("sess-1", "/canonical")
    })

    it.each([false, true])("stale listing rejected=%s cannot replace the latest refresh path", async rejected => {
        connectHost()
        const mutation = beginMutation("mkdir")
        const old = deferred<SftpListing>()
        mockList.mockReturnValueOnce(old.promise)
        const obsolete = useSftpStore.getState().listRemote(HOST, "/obsolete")
        mockList.mockResolvedValueOnce(listing("/B", []))
        await useSftpStore.getState().listRemote(HOST, "/B")
        if (rejected) old.reject(new Error("obsolete"))
        else old.resolve(listing("/obsolete-canonical", []))
        await obsolete
        mockList.mockImplementation(async (_session, path) => listing(path!, []))
        mutation.resolve()
        await mutation.done
        expect(mockList).toHaveBeenLastCalledWith("sess-1", "/B")
    })

    it("preserves a navigation error then refreshes the directory still displayed", async () => {
        connectHost()
        mockList.mockRejectedValueOnce(new Error("navigation failed"))
        await useSftpStore.getState().listRemote(HOST, "/B")
        expect(useSftpStore.getState().remote[HOST]).toMatchObject({ cwd: "/home/u", error: "Error: navigation failed" })
        mockList.mockResolvedValueOnce(listing("/home/u", []))
        await useSftpStore.getState().mkdir(HOST, "new")
        expect(mockList).toHaveBeenLastCalledWith("sess-1", "/home/u")
        expect(useSftpStore.getState().remote[HOST].error).toBeNull()
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

describe("SFTP concurrent overwrite confirmations", () => {
    let jobs: Promise<void>[]
    let finishUploads: (() => void)[]
    const pendingName = () => useAppDialogStore.getState().pending?.description ?? ""
    const start = (name: string) => {
        const job = useSftpStore.getState().upload(HOST, { kind: "selected", capabilityId: `picked-${name}`, name })
        jobs.push(job)
        return job
    }

    beforeEach(() => {
        jobs = []
        finishUploads = []
        useAppDialogStore.getState().respond(false)
        connectHost()
        mockList.mockResolvedValue(listing("/home/u", []))
        mockUpload.mockResolvedValue(undefined)
        vi.mocked(sftpFileRevision).mockImplementation(async (_, path) => path.endsWith("/new") ? null : `revision:${path}`)
    })

    afterEach(async () => {
        useSftpStore.getState().reset()
        useAppDialogStore.getState().respond(false)
        for (const finish of finishUploads) finish()
        await Promise.all(jobs)
    })

    it("asks about both files in order while approved transfers remain concurrent", async () => {
        mockUpload.mockImplementation(() => new Promise<void>(resolve => { finishUploads.push(resolve) }))
        start("first"); start("second")
        await vi.waitFor(() => expect(sftpFileRevision).toHaveBeenCalledTimes(2))
        expect(pendingName()).toContain("first")
        expect(mockUpload).not.toHaveBeenCalled()
        expect(sftpTransferCancel).not.toHaveBeenCalled()
        useAppDialogStore.getState().respond(true)
        await vi.waitFor(() => expect(pendingName()).toContain("second"))
        useAppDialogStore.getState().respond(true)
        await vi.waitFor(() => expect(mockUpload).toHaveBeenCalledTimes(2))
        expect(mockUpload.mock.calls.map(call => [call[2], call[4]])).toEqual([
            [{ kind: "selected", capabilityId: "picked-first", name: "first" }, "revision:/home/u/first"],
            [{ kind: "selected", capabilityId: "picked-second", name: "second" }, "revision:/home/u/second"]
        ])
        for (const finish of finishUploads) finish()
        await Promise.all(jobs)
        expect(Object.values(useSftpStore.getState().transfers).map(transfer => transfer.error)).toEqual([null, null])
    })

    it("declines one file without cancelling the next confirmation", async () => {
        start("first"); start("second")
        await vi.waitFor(() => expect(sftpFileRevision).toHaveBeenCalledTimes(2))
        expect(pendingName()).toContain("first")
        useAppDialogStore.getState().respond(false)
        await vi.waitFor(() => expect(pendingName()).toContain("second"))
        useAppDialogStore.getState().respond(true)
        await Promise.all(jobs)
        expect(mockUpload).toHaveBeenCalledTimes(1)
        expect(mockUpload.mock.calls[0][2]).toMatchObject({ name: "second" })
        expect(Object.values(useSftpStore.getState().transfers).find(transfer => transfer.name === "first")?.error).toBe("sftp-transfer-cancelled")
    })

    it("does not hold a new-file upload behind an overwrite prompt", async () => {
        start("first")
        await vi.waitFor(() => expect(pendingName()).toContain("first"))
        await start("new")
        expect(mockUpload).toHaveBeenCalledTimes(1)
        expect(mockUpload.mock.calls[0][4]).toBeNull()
        expect(pendingName()).toContain("first")
        useAppDialogStore.getState().respond(false)
        await Promise.all(jobs)
    })

    it.each(["cancel", "disconnect", "forget", "reset"] as const)("does not prompt or upload a queued file after %s", async action => {
        start("first"); start("second")
        await vi.waitFor(() => expect(sftpFileRevision).toHaveBeenCalledTimes(2))
        expect(pendingName()).toContain("first")
        if (action === "cancel") {
            const id = Object.keys(useSftpStore.getState().transfers).find(id => useSftpStore.getState().transfers[id].name === "second")!
            await useSftpStore.getState().cancelTransfer(id)
        } else if (action === "disconnect") useSshStore.setState({ sessions: {} })
        else if (action === "forget") forgetSftpHosts([HOST])
        else useSftpStore.getState().reset()
        useAppDialogStore.getState().respond(true)
        await Promise.all(jobs)
        expect(useAppDialogStore.getState().pending).toBeNull()
        expect(mockUpload.mock.calls.map(call => call[2])).toEqual(action === "cancel"
            ? [{ kind: "selected", capabilityId: "picked-first", name: "first" }]
            : [])
    })

    it("retires an accepted prompt when reset removed its reservation", async () => {
        start("first")
        await vi.waitFor(() => expect(pendingName()).toContain("first"))
        useSftpStore.getState().reset()
        useAppDialogStore.getState().respond(true)
        await Promise.all(jobs)
        expect(mockUpload).not.toHaveBeenCalled()
        expect(useSftpStore.getState().transfers).toEqual({})
    })

    it("starts a fresh queue after reset without reviving old requests", async () => {
        start("first"); start("second")
        await vi.waitFor(() => expect(sftpFileRevision).toHaveBeenCalledTimes(2))
        expect(pendingName()).toContain("first")
        useSftpStore.getState().reset()
        useSftpStore.setState({ remote: { [HOST]: { cwd: "/home/u", entries: [], loading: false, error: null } } })
        start("fresh")
        await vi.waitFor(() => expect(pendingName()).toContain("fresh"))
        useAppDialogStore.getState().respond(true)
        await Promise.all(jobs)
        expect(mockUpload).toHaveBeenCalledTimes(1)
        expect(mockUpload.mock.calls[0][2]).toMatchObject({ name: "fresh" })
    })

    it("does not block the next prompt when an approved upload reports a revision conflict", async () => {
        mockUpload.mockRejectedValueOnce(new Error("sftp-revision-conflict"))
        start("first"); start("second")
        await vi.waitFor(() => expect(sftpFileRevision).toHaveBeenCalledTimes(2))
        expect(pendingName()).toContain("first")
        useAppDialogStore.getState().respond(true)
        await vi.waitFor(() => expect(pendingName()).toContain("second"))
        useAppDialogStore.getState().respond(true)
        await Promise.all(jobs)
        expect(mockUpload).toHaveBeenCalledTimes(2)
        expect(Object.values(useSftpStore.getState().transfers).find(transfer => transfer.name === "first")?.error).toBe("sftp-revision-conflict")
        expect(Object.values(useSftpStore.getState().transfers).find(transfer => transfer.name === "second")?.error).toBeNull()
    })

    it.each(["respond", "reset", "cancel", "another-dialog"] as const)("preserves another feature's confirmation while queued uploads wait (%s)", async action => {
        start("first"); start("second")
        await vi.waitFor(() => expect(sftpFileRevision).toHaveBeenCalledTimes(2))
        expect(pendingName()).toContain("first")
        let externalSettled = false
        const external = requestAppConfirmation({ title: "Other feature", description: "Unrelated confirmation" })
        void external.then(() => { externalSettled = true })
        await vi.waitFor(() => expect(Object.values(useSftpStore.getState().transfers).find(transfer => transfer.name === "first")?.done).toBe(true))
        expect(useAppDialogStore.getState().pending?.title).toBe("Other feature")
        expect(externalSettled).toBe(false)
        if (action === "reset") useSftpStore.getState().reset()
        if (action === "cancel") {
            const id = Object.keys(useSftpStore.getState().transfers).find(id => useSftpStore.getState().transfers[id].name === "second")!
            await useSftpStore.getState().cancelTransfer(id)
        }
        if (action === "reset" || action === "cancel") await Promise.all(jobs)
        expect(useAppDialogStore.getState().pending?.title).toBe("Other feature")
        expect(externalSettled).toBe(false)
        useAppDialogStore.getState().respond(true)
        let another: Promise<boolean> | undefined
        if (action === "another-dialog") another = requestAppConfirmation({ title: "Third feature", description: "Another confirmation" })
        await expect(external).resolves.toBe(true)
        if (another) {
            await Promise.resolve(); await Promise.resolve()
            expect(useAppDialogStore.getState().pending?.title).toBe("Third feature")
            useAppDialogStore.getState().respond(false)
            await expect(another).resolves.toBe(false)
        }
        if (action === "respond" || action === "another-dialog") {
            await vi.waitFor(() => expect(pendingName()).toContain("second"))
            useAppDialogStore.getState().respond(true)
        }
        await Promise.all(jobs)
        expect(mockUpload).toHaveBeenCalledTimes(action === "reset" || action === "cancel" ? 0 : 1)
    })
    it.each(["respond", "reset", "cancel", "disconnect", "forget"] as const)("releases dialog-wait subscriptions after %s", async action => {
        let active = 0
        function observe<S>(store: { subscribe: (listener: (state: S, previous: S) => void) => () => void }) {
            const subscribe = store.subscribe
            return vi.spyOn(store, "subscribe").mockImplementation(listener => {
                active++
                const stop = subscribe(listener)
                let stopped = false
                return () => {
                    if (stopped) return
                    stopped = true
                    active--
                    stop()
                }
            })
        }
        const observers = [observe(useAppDialogStore), observe(useSftpStore), observe(useSshStore)]
        try {
            const external = requestAppConfirmation({ title: "Other feature", description: "Waiting" })
            start("first")
            await vi.waitFor(() => expect(active).toBeGreaterThan(0))
            expect(useAppDialogStore.getState().pending?.title).toBe("Other feature")
            if (action === "respond") {
                useAppDialogStore.getState().respond(true)
                await expect(external).resolves.toBe(true)
                await vi.waitFor(() => expect(pendingName()).toContain("first"))
                expect(active).toBe(0)
                useAppDialogStore.getState().respond(true)
            } else {
                if (action === "reset") useSftpStore.getState().reset()
                if (action === "disconnect") useSshStore.setState({ sessions: {} })
                if (action === "forget") forgetSftpHosts([HOST])
                if (action === "cancel") await useSftpStore.getState().cancelTransfer(Object.keys(useSftpStore.getState().transfers)[0])
                await Promise.all(jobs)
                expect(active).toBe(0)
                expect(useAppDialogStore.getState().pending?.title).toBe("Other feature")
                useAppDialogStore.getState().respond(false)
                await expect(external).resolves.toBe(false)
            }
            await Promise.all(jobs)
            expect(active).toBe(0)
        } finally {
            for (const observer of observers) observer.mockRestore()
        }
    })

})
