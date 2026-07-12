import { beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/ipc", () => ({
    sshConnect: vi.fn(),
    sshDisconnect: vi.fn()
}))

import { sshConnect, sshDisconnect } from "@/lib/ipc"
import { SSH_HOSTS_STORAGE_KEY, loadSshHosts, useSshStore, type NewSshHost } from "./sshStore"

const mockConnect = vi.mocked(sshConnect)
const mockDisconnect = vi.mocked(sshDisconnect)

// The Bun-hosted test runtime injects an empty `localStorage` global with no
// Storage methods; install a minimal in-memory Storage so persistence runs for
// real (mirrors recentWorkspaces.test.ts).
function installLocalStorage(): void {
    const store = new Map<string, string>()
    const mock = {
        getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
        setItem: (k: string, v: string) => void store.set(k, String(v)),
        removeItem: (k: string) => void store.delete(k),
        clear: () => store.clear(),
        key: (i: number) => [...store.keys()][i] ?? null,
        get length() {
            return store.size
        }
    }
    Object.defineProperty(globalThis, "localStorage", {
        value: mock,
        configurable: true,
        writable: true
    })
}

const passwordHost: NewSshHost = {
    name: "web",
    host: "example.com",
    port: 22,
    user: "root",
    authKind: "password"
}

const keyHost: NewSshHost = {
    name: "box",
    host: "10.0.0.5",
    port: 2222,
    user: "deploy",
    authKind: "key",
    keyPath: "/home/u/.ssh/id_ed25519"
}

beforeEach(() => {
    installLocalStorage()
    localStorage.clear()
    vi.clearAllMocks()
    let seq = 0
    mockConnect.mockImplementation(async () => ({
        sessionId: `sess-${++seq}`,
        fingerprint: "SHA256:abc"
    }))
    mockDisconnect.mockResolvedValue(undefined)
    useSshStore.setState({ hosts: [], sessions: {}, activeHostId: null, pendingAuthHostId: null })
})

describe("useSshStore host book", () => {
    it("addHost appends and persists the descriptor", () => {
        const host = useSshStore.getState().addHost(passwordHost)
        expect(host.id).toMatch(/^ssh-/)
        expect(useSshStore.getState().hosts).toHaveLength(1)
        expect(loadSshHosts()).toEqual([{ ...passwordHost, id: host.id }])
    })

    it("addHost drops keyPath for password auth and keeps it for key auth", () => {
        const pw = useSshStore.getState().addHost({ ...passwordHost, keyPath: "/leak" } as NewSshHost)
        expect(pw.keyPath).toBeUndefined()
        const key = useSshStore.getState().addHost(keyHost)
        expect(key.keyPath).toBe("/home/u/.ssh/id_ed25519")
    })

    it("removeHost drops the host, persists, and clears its session/active state", async () => {
        const host = useSshStore.getState().addHost(passwordHost)
        await useSshStore.getState().connect(host.id, "hunter2")
        expect(useSshStore.getState().activeHostId).toBe(host.id)

        useSshStore.getState().removeHost(host.id)
        expect(useSshStore.getState().hosts).toEqual([])
        expect(loadSshHosts()).toEqual([])
        expect(useSshStore.getState().sessions[host.id]).toBeUndefined()
        expect(useSshStore.getState().activeHostId).toBeNull()
        expect(mockDisconnect).toHaveBeenCalledWith("sess-1")
    })

    it("hosts survive a reload via loadSshHosts", () => {
        const host = useSshStore.getState().addHost(keyHost)
        const reloaded = loadSshHosts()
        expect(reloaded).toEqual([{ ...keyHost, id: host.id }])
    })
})

describe("useSshStore updateHost", () => {
    it("edits the descriptor in place and persists it", () => {
        const host = useSshStore.getState().addHost(passwordHost)
        useSshStore.getState().updateHost(host.id, {
            ...passwordHost,
            name: "renamed",
            host: "new.example.com",
            port: 2200
        })
        const updated = useSshStore.getState().hosts.find((h) => h.id === host.id)!
        expect(updated).toEqual({
            ...passwordHost,
            id: host.id,
            name: "renamed",
            host: "new.example.com",
            port: 2200
        })
        expect(loadSshHosts()).toEqual([updated])
    })

    it("re-sanitizes auth-dependent fields when the auth kind changes", () => {
        const host = useSshStore.getState().addHost(keyHost)
        // key → password: the now-irrelevant keyPath must be dropped.
        useSshStore.getState().updateHost(host.id, { ...passwordHost, name: keyHost.name })
        const asPassword = useSshStore.getState().hosts.find((h) => h.id === host.id)!
        expect(asPassword.authKind).toBe("password")
        expect(asPassword.keyPath).toBeUndefined()
        // password → key: keyPath is restored.
        useSshStore.getState().updateHost(host.id, keyHost)
        const asKey = useSshStore.getState().hosts.find((h) => h.id === host.id)!
        expect(asKey.keyPath).toBe("/home/u/.ssh/id_ed25519")
    })

    it("leaves a live session untouched (editing a connected host does not disconnect)", async () => {
        const host = useSshStore.getState().addHost(passwordHost)
        await useSshStore.getState().connect(host.id, "pw")
        const before = useSshStore.getState().sessions[host.id]
        expect(before.status).toBe("connected")

        useSshStore.getState().updateHost(host.id, { ...passwordHost, name: "renamed" })

        const after = useSshStore.getState().sessions[host.id]
        // Same object reference — the sessions slice was not rewritten.
        expect(after).toBe(before)
        expect(after.status).toBe("connected")
        expect(after.sessionId).toBe("sess-1")
        expect(mockDisconnect).not.toHaveBeenCalled()
    })
})

describe("useSshStore never persists secrets", () => {
    it("keeps no password/passphrase in the stored payload after connecting", async () => {
        const host = useSshStore.getState().addHost(passwordHost)
        await useSshStore.getState().connect(host.id, "sup3r-secret-pw")

        const raw = localStorage.getItem(SSH_HOSTS_STORAGE_KEY) ?? ""
        // The secret value must never land in storage. "password" itself can
        // appear as the authKind value, so assert on the secret, not the word.
        expect(raw).not.toContain("sup3r-secret-pw")
        expect(raw).not.toContain("passphrase")
        // The connect call carried the secret, but storage never saw it.
        expect(mockConnect).toHaveBeenCalledWith("example.com", 22, "root", {
            kind: "password",
            password: "sup3r-secret-pw"
        })
    })

    it("passes the key passphrase to ipc without persisting it", async () => {
        const host = useSshStore.getState().addHost(keyHost)
        await useSshStore.getState().connect(host.id, "keypass")

        const raw = localStorage.getItem(SSH_HOSTS_STORAGE_KEY) ?? ""
        expect(raw).not.toContain("keypass")
        expect(mockConnect).toHaveBeenCalledWith("10.0.0.5", 2222, "deploy", {
            kind: "key",
            keyPath: "/home/u/.ssh/id_ed25519",
            passphrase: "keypass"
        })
    })
})

describe("useSshStore connection lifecycle", () => {
    it("connect marks connecting then connected and stores the fingerprint", async () => {
        const host = useSshStore.getState().addHost(passwordHost)
        await useSshStore.getState().connect(host.id, "pw")
        const session = useSshStore.getState().sessions[host.id]
        expect(session.status).toBe("connected")
        expect(session.sessionId).toBe("sess-1")
        expect(session.fingerprint).toBe("SHA256:abc")
    })

    it("connect records the error message on failure", async () => {
        mockConnect.mockRejectedValueOnce("SSH 認證失敗：帳號、密碼或金鑰不正確")
        const host = useSshStore.getState().addHost(passwordHost)
        await useSshStore.getState().connect(host.id, "wrong")
        const session = useSshStore.getState().sessions[host.id]
        expect(session.status).toBe("error")
        expect(session.error).toContain("SSH 認證失敗")
        // The attempt focuses the host so the panel can show the error banner.
        expect(useSshStore.getState().activeHostId).toBe(host.id)
    })

    it("beginConnect prompts for a password host and connects a key host directly", () => {
        const pw = useSshStore.getState().addHost(passwordHost)
        useSshStore.getState().beginConnect(pw.id)
        expect(useSshStore.getState().pendingAuthHostId).toBe(pw.id)
        expect(mockConnect).not.toHaveBeenCalled()

        const key = useSshStore.getState().addHost(keyHost)
        useSshStore.getState().beginConnect(key.id)
        expect(mockConnect).toHaveBeenCalledWith("10.0.0.5", 2222, "deploy", {
            kind: "key",
            keyPath: "/home/u/.ssh/id_ed25519",
            passphrase: undefined
        })
    })

    it("beginConnect on an already-connected host just activates it", async () => {
        const a = useSshStore.getState().addHost(passwordHost)
        const b = useSshStore.getState().addHost({ ...passwordHost, name: "other" })
        await useSshStore.getState().connect(a.id, "pw")
        await useSshStore.getState().connect(b.id, "pw")
        expect(useSshStore.getState().activeHostId).toBe(b.id)

        mockConnect.mockClear()
        useSshStore.getState().beginConnect(a.id)
        expect(mockConnect).not.toHaveBeenCalled()
        expect(useSshStore.getState().activeHostId).toBe(a.id)
    })

    it("disconnect flips status to disconnected and clears the session id", async () => {
        const host = useSshStore.getState().addHost(passwordHost)
        await useSshStore.getState().connect(host.id, "pw")
        await useSshStore.getState().disconnect(host.id)
        const session = useSshStore.getState().sessions[host.id]
        expect(session.status).toBe("disconnected")
        expect(session.sessionId).toBeNull()
        expect(mockDisconnect).toHaveBeenCalledWith("sess-1")
    })

    it("disconnect propagates backend failure and keeps the live session intact", async () => {
        const host = useSshStore.getState().addHost(passwordHost)
        await useSshStore.getState().connect(host.id, "pw")
        mockDisconnect.mockRejectedValueOnce(new Error("disconnect failed"))

        await expect(useSshStore.getState().disconnect(host.id)).rejects.toThrow("disconnect failed")
        expect(useSshStore.getState().sessions[host.id]).toMatchObject({
            status: "connected",
            sessionId: "sess-1"
        })
    })

    it("markExit disconnects the session matching a sessionId", async () => {
        const host = useSshStore.getState().addHost(passwordHost)
        await useSshStore.getState().connect(host.id, "pw")
        useSshStore.getState().markExit("sess-1")
        const session = useSshStore.getState().sessions[host.id]
        expect(session.status).toBe("disconnected")
        expect(session.sessionId).toBeNull()
    })

    it("cancelPendingAuth clears the pending prompt", () => {
        const host = useSshStore.getState().addHost(passwordHost)
        useSshStore.getState().beginConnect(host.id)
        expect(useSshStore.getState().pendingAuthHostId).toBe(host.id)
        useSshStore.getState().cancelPendingAuth()
        expect(useSshStore.getState().pendingAuthHostId).toBeNull()
    })
})
