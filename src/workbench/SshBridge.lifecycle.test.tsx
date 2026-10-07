import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { act, cleanup, render } from "@testing-library/react"
import { StrictMode } from "react"
import type { SftpListing } from "@/lib/types"

const events = vi.hoisted(() => ({ handlers: new Map<symbol, { name: string, handler: (event: { payload: unknown }) => void }>() }))
vi.mock("@tauri-apps/api/event", () => ({
    listen: async (name: string, handler: (event: { payload: unknown }) => void) => {
        const id = Symbol(name)
        events.handlers.set(id, { name, handler })
        return () => { events.handlers.delete(id) }
    }
}))
vi.mock("@/lib/ipc", () => ({
    sshConnect: vi.fn(), sshDisconnect: vi.fn(), sftpListDir: vi.fn(),
    sftpTransferPrepare: vi.fn(), sftpTransferCancel: vi.fn(), sftpFileRevision: vi.fn(),
    sftpUpload: vi.fn(), sftpDownload: vi.fn(), sftpTransferTree: vi.fn(),
    sftpMkdir: vi.fn(), sftpRename: vi.fn(), sftpRemove: vi.fn()
}))
vi.mock("@/state/appDialogStore", () => ({ requestAppConfirmation: vi.fn(async () => true) }))
import { sshDisconnect, sftpListDir, sftpTransferPrepare, sftpTransferCancel, sftpFileRevision, sftpUpload, sftpDownload, sftpTransferTree } from "@/lib/ipc"
import { requestAppConfirmation } from "@/state/appDialogStore"
import { useSshStore } from "@/state/sshStore"
import { useSftpStore } from "@/state/sftpStore"
import { SshBridge } from "./SshBridge"

const source = { kind: "workspace" as const, workspaceId: "owned-workspace", relativePath: "file.txt" }
const data = (cwd: string): SftpListing => ({ cwd, entries: [{ name: "file.txt", path: cwd + "/file.txt", isDir: false, isSymlink: false, size: 12 }] })
function connected(id: string, sessionId = "session-" + id) {
    useSshStore.setState(state => ({
        hosts: [...state.hosts, { id, name: id, host: "owned.invalid", port: 22, user: "owned", authKind: "password" }],
        sessions: { ...state.sessions, [id]: { hostId: id, sessionId, status: "connected", fingerprint: null, knownHost: false, error: null } }
    }))
}
function deferred<T>() {
    let resolve!: (value: T) => void
    const promise = new Promise<T>(done => { resolve = done })
    return { promise, resolve }
}
async function drain() { for (let i = 0; i < 8; i++) await Promise.resolve() }

beforeEach(() => {
    Object.defineProperty(globalThis, "localStorage", { value: { getItem: () => null, setItem() {}, removeItem() {} }, configurable: true })
    vi.resetAllMocks()
    useSftpStore.getState().reset()
    useSshStore.setState({ hosts: [], sessions: {}, activeHostId: null, pendingAuthHostId: null })
    expect(events.handlers.size).toBe(0)
    vi.mocked(sshDisconnect).mockResolvedValue(undefined)
    vi.mocked(sftpListDir).mockImplementation(async (_session, path) => data(path || "/owned"))
    let sequence = 0
    vi.mocked(sftpTransferPrepare).mockImplementation(async () => "owned-transfer-" + (++sequence))
    vi.mocked(sftpTransferCancel).mockResolvedValue(undefined)
    vi.mocked(sftpFileRevision).mockResolvedValue(null)
    vi.mocked(sftpUpload).mockResolvedValue(undefined)
    vi.mocked(sftpDownload).mockResolvedValue(undefined)
    vi.mocked(sftpTransferTree).mockResolvedValue({ files: 1, bytes: 12 })
    vi.mocked(requestAppConfirmation).mockResolvedValue(true)
})
afterEach(async () => {
    cleanup()
    await drain()
    expect(events.handlers.size).toBe(0)
    useSftpStore.getState().reset()
    useSshStore.setState({ hosts: [], sessions: {}, activeHostId: null, pendingAuthHostId: null })
    vi.restoreAllMocks()
})

it("releases removed-host listings and transfers over 100 warm and 100 measured lifecycles", async () => {
    render(<SshBridge />)
    connected("kept")
    await useSftpStore.getState().listRemote("kept", "/kept")
    await useSftpStore.getState().upload("kept", source)
    const keptRemote = useSftpStore.getState().remote.kept
    const keptId = Object.keys(useSftpStore.getState().transfers)[0]
    const keptTransfer = useSftpStore.getState().transfers[keptId]
    const original = Map.prototype.set
    let tokenCount: (() => number) | undefined
    vi.spyOn(Map.prototype, "set").mockImplementation(function (this: Map<unknown, unknown>, key, value) {
        if (typeof key === "string" && typeof value === "symbol" && value.description === key) tokenCount = () => this.size
        return original.call(this, key, value)
    })
    for (let i = 0; i < 200; i++) {
        const host = useSshStore.getState().addHost({ name: "owned", host: "owned.invalid", port: 22, user: "owned", authKind: "password" })
        useSshStore.setState(state => ({ sessions: { ...state.sessions, [host.id]: { hostId: host.id, sessionId: "session-" + host.id, status: "connected", fingerprint: null, knownHost: false, error: null } } }))
        await useSftpStore.getState().listRemote(host.id, "/owned")
        await useSftpStore.getState().upload(host.id, source)
        act(() => useSshStore.getState().removeHost(host.id))
        expect(Object.keys(useSftpStore.getState().remote)).toEqual(["kept"])
        expect(Object.keys(useSftpStore.getState().transfers)).toEqual([keptId])
        expect(useSftpStore.getState().remote.kept).toBe(keptRemote)
        expect(useSftpStore.getState().transfers[keptId]).toBe(keptTransfer)
        expect(tokenCount?.()).toBe(1)
    }
    act(() => useSshStore.getState().removeHost("kept"))
    expect(tokenCount?.()).toBe(0)
    expect(useSftpStore.getState().remote).toEqual({})
    expect(useSftpStore.getState().transfers).toEqual({})
    expect(sftpTransferCancel).not.toHaveBeenCalled()
})

it("preserves configured-host data during metadata edits and ordinary disconnects", async () => {
    connected("kept")
    render(<SshBridge />)
    await useSftpStore.getState().listRemote("kept", "/kept")
    await useSftpStore.getState().upload("kept", source)
    const before = useSftpStore.getState()
    act(() => useSshStore.getState().updateHost("kept", { name: "renamed", host: "owned.invalid", port: 22, user: "owned", authKind: "password" }))
    expect(useSftpStore.getState()).toBe(before)
    await useSshStore.getState().disconnect("kept")
    expect(useSftpStore.getState()).toBe(before)
})

it("prunes orphan projections at mount without touching a surviving host", async () => {
    connected("removed")
    connected("kept")
    await useSftpStore.getState().listRemote("removed", "/removed")
    await useSftpStore.getState().upload("removed", source)
    await useSftpStore.getState().listRemote("kept", "/kept")
    const kept = useSftpStore.getState().remote.kept
    useSshStore.getState().removeHost("removed")
    expect(useSftpStore.getState().remote.removed).toBeDefined()
    render(<SshBridge />)
    expect(useSftpStore.getState().remote.removed).toBeUndefined()
    expect(useSftpStore.getState().remote.kept).toBe(kept)
    expect(Object.values(useSftpStore.getState().transfers).some(transfer => transfer.hostId === "removed")).toBe(false)
})

it("retires the old host identity when connection details change without changing the profile count", async () => {
    connected("replaced")
    connected("kept")
    render(<SshBridge />)
    await useSftpStore.getState().listRemote("replaced", "/old")
    await useSftpStore.getState().upload("replaced", source)
    await useSftpStore.getState().listRemote("kept", "/kept")
    const kept = useSftpStore.getState().remote.kept
    act(() => useSshStore.getState().updateHost("replaced", { name: "new address", host: "changed.invalid", port: 22, user: "owned", authKind: "password" }))
    expect(useSshStore.getState().hosts).toHaveLength(2)
    expect(useSshStore.getState().hosts.some(host => host.id === "replaced")).toBe(false)
    expect(useSftpStore.getState().remote.replaced).toBeUndefined()
    expect(useSftpStore.getState().transfers).toEqual({})
    expect(useSftpStore.getState().remote.kept).toBe(kept)
    expect(sshDisconnect).toHaveBeenCalledWith("session-replaced")
})

it("retires a disconnected host cache when its profile is removed", async () => {
    connected("removed")
    render(<SshBridge />)
    await useSftpStore.getState().listRemote("removed", "/owned")
    await useSftpStore.getState().upload("removed", source)
    await useSshStore.getState().disconnect("removed")
    expect(useSftpStore.getState().remote.removed).toBeDefined()
    act(() => useSshStore.getState().removeHost("removed"))
    expect(useSftpStore.getState().remote).toEqual({})
    expect(useSftpStore.getState().transfers).toEqual({})
})

it("preserves host history when only credentials change", async () => {
    connected("kept")
    render(<SshBridge />)
    await useSftpStore.getState().listRemote("kept", "/kept")
    await useSftpStore.getState().upload("kept", source)
    const before = useSftpStore.getState()
    act(() => useSshStore.getState().updateHost("kept", { name: "kept", host: "owned.invalid", port: 22, user: "owned", authKind: "key", keyPath: "/owned/key" }))
    expect(useSshStore.getState().hosts[0].id).toBe("kept")
    expect(useSftpStore.getState()).toBe(before)
    expect(sshDisconnect).toHaveBeenCalledWith("session-kept")
})

it("rejects late listings without invalidating a new listing for the same host id", async () => {
    connected("reopened", "old-session")
    render(<SshBridge />)
    const old = deferred<SftpListing>(), fresh = deferred<SftpListing>()
    vi.mocked(sftpListDir).mockReturnValueOnce(old.promise).mockReturnValueOnce(fresh.promise)
    const previous = useSftpStore.getState().listRemote("reopened", "/old")
    act(() => useSshStore.getState().removeHost("reopened"))
    connected("reopened", "new-session")
    const current = useSftpStore.getState().listRemote("reopened", "/new")
    old.resolve(data("/old"))
    await previous
    expect(useSftpStore.getState().remote.reopened).toMatchObject({ loading: true, cwd: "" })
    fresh.resolve(data("/new"))
    await current
    expect(useSftpStore.getState().remote.reopened).toMatchObject({ loading: false, cwd: "/new" })
})

it.each(["upload", "download", "tree"] as const)("cancels a late %s reservation instead of recreating a removed host", async kind => {
    connected("removed")
    render(<SshBridge />)
    const ticket = deferred<string>()
    vi.mocked(sftpTransferPrepare).mockReturnValueOnce(ticket.promise)
    const state = useSftpStore.getState()
    const pending = kind === "upload" ? state.upload("removed", source, "/owned")
        : kind === "download" ? state.download("removed", data("/owned").entries[0], { capabilityId: "owned-destination", leaf: "file.txt" })
        : state.transferTree("removed", "owned-selection", "upload", "/owned", "folder")
    act(() => useSshStore.getState().removeHost("removed"))
    ticket.resolve("late-ticket")
    await pending
    expect(sftpTransferCancel).toHaveBeenCalledWith("session-removed", "late-ticket")
    expect(useSftpStore.getState().transfers).toEqual({})
    expect(useSftpStore.getState().remote).toEqual({})
    expect(sftpUpload).not.toHaveBeenCalled()
    expect(sftpDownload).not.toHaveBeenCalled()
    expect(sftpTransferTree).not.toHaveBeenCalled()
})

it("does not create an overwrite prompt after host removal during revision preflight", async () => {
    connected("removed")
    render(<SshBridge />)
    const revision = deferred<string | null>()
    vi.mocked(sftpFileRevision).mockReturnValueOnce(revision.promise)
    const pending = useSftpStore.getState().upload("removed", source, "/owned")
    await drain()
    expect(sftpFileRevision).toHaveBeenCalledOnce()
    act(() => useSshStore.getState().removeHost("removed"))
    revision.resolve("existing-revision")
    await pending
    expect(requestAppConfirmation).not.toHaveBeenCalled()
    expect(useSftpStore.getState().transfers).toEqual({})
    expect(sftpUpload).not.toHaveBeenCalled()
})

it("keeps overwrite confirmation when the session still owns the upload", async () => {
    connected("kept")
    render(<SshBridge />)
    vi.mocked(sftpFileRevision).mockResolvedValueOnce("existing-revision")
    await useSftpStore.getState().upload("kept", source, "/owned")
    expect(requestAppConfirmation).toHaveBeenCalledOnce()
    expect(sftpUpload).toHaveBeenCalledWith("session-kept", expect.any(String), source, "/owned", "existing-revision")
})

it("does not recreate an unknown host cache from a stale browse callback", async () => {
    render(<SshBridge />)
    await useSftpStore.getState().listRemote("removed", "/old")
    expect(useSftpStore.getState().remote).toEqual({})
    expect(sftpListDir).not.toHaveBeenCalled()
})

it("ignores removed-host progress and completion while another transfer remains active", async () => {
    connected("removed")
    connected("kept")
    render(<SshBridge />)
    const removed = deferred<void>(), kept = deferred<void>()
    vi.mocked(sftpUpload).mockReturnValueOnce(removed.promise).mockReturnValueOnce(kept.promise)
    const first = useSftpStore.getState().upload("removed", source, "/removed")
    await drain()
    const second = useSftpStore.getState().upload("kept", source, "/kept")
    await drain()
    const keptTransfer = useSftpStore.getState().transfers["owned-transfer-2"]
    expect(keptTransfer.done).toBe(false)
    act(() => useSshStore.getState().removeHost("removed"))
    for (const { name, handler } of events.handlers.values()) {
        expect(name).toBe("sftp://progress")
        handler({ payload: { sessionId: "session-removed", transferId: "owned-transfer-1", transferred: 12, total: 12, done: true } })
    }
    removed.resolve(undefined)
    await first
    expect(Object.keys(useSftpStore.getState().transfers)).toEqual(["owned-transfer-2"])
    expect(useSftpStore.getState().transfers["owned-transfer-2"]).toBe(keptTransfer)
    kept.resolve(undefined)
    await second
    expect(useSftpStore.getState().transfers["owned-transfer-2"].done).toBe(true)
    expect(sftpTransferCancel).not.toHaveBeenCalled()
})

it("cancels an unpublished reservation after ordinary disconnect while preserving history", async () => {
    connected("kept")
    render(<SshBridge />)
    await useSftpStore.getState().listRemote("kept", "/kept")
    await useSftpStore.getState().upload("kept", source)
    const before = useSftpStore.getState()
    const ticket = deferred<string>()
    vi.mocked(sftpTransferPrepare).mockReturnValueOnce(ticket.promise)
    const pending = before.upload("kept", source)
    await useSshStore.getState().disconnect("kept")
    ticket.resolve("late-ticket")
    await pending
    expect(useSftpStore.getState()).toBe(before)
    expect(sftpTransferCancel).toHaveBeenCalledWith("session-kept", "late-ticket")
    expect(sftpUpload).toHaveBeenCalledTimes(1)
})

it("releases subscriptions across repeated StrictMode mounts and early unmounts", async () => {
    let active = 0
    const subscribe = useSshStore.subscribe
    vi.spyOn(useSshStore, "subscribe").mockImplementation(listener => {
        active++
        const off = subscribe(listener)
        return () => { active--; off() }
    })
    for (let i = 0; i < 100; i++) {
        const mounted = render(<StrictMode><SshBridge /></StrictMode>)
        expect(active).toBe(1)
        if (i % 2 === 0) {
            await drain()
            expect(events.handlers.size).toBe(1)
        }
        mounted.unmount()
        expect(active).toBe(0)
        await drain()
        expect(events.handlers.size).toBe(0)
    }
})
