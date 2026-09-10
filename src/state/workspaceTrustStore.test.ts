import { beforeEach, expect, it, vi } from "vitest"

import { useWorkspaceTrustStore } from "@/state/workspaceTrustStore"

const ipcMocks = vi.hoisted(() => ({
    workspaceTrustStatus: vi.fn(),
    workspaceTrustGrant: vi.fn(),
    workspaceTrustList: vi.fn(),
    workspaceTrustRevoke: vi.fn()
}))

vi.mock("@/lib/ipc", async (importOriginal) => ({
    ...(await importOriginal<typeof import("@/lib/ipc")>()),
    workspaceTrustStatus: (...args: unknown[]) => ipcMocks.workspaceTrustStatus(...args),
    workspaceTrustGrant: (...args: unknown[]) => ipcMocks.workspaceTrustGrant(...args),
    workspaceTrustList: (...args: unknown[]) => ipcMocks.workspaceTrustList(...args),
    workspaceTrustRevoke: (...args: unknown[]) => ipcMocks.workspaceTrustRevoke(...args)
}))

beforeEach(() => {
    useWorkspaceTrustStore.getState().cancelPrompt()
    useWorkspaceTrustStore.setState({
        statusByPath: {},
        trustedWorkspaces: [],
        trustRevision: 0,
        prompt: null,
        lastError: null,
        confirming: false
    })
    useWorkspaceTrustStore.getState().cancelPrompt()
    ipcMocks.workspaceTrustStatus.mockReset()
    ipcMocks.workspaceTrustGrant.mockReset()
    ipcMocks.workspaceTrustList.mockReset()
    ipcMocks.workspaceTrustRevoke.mockReset()
})

const untrusted = {
    state: "untrusted" as const,
    canonicalPath: "/w",
    challengeId: "grant-1",
    repoPresent: true
}

it("keeps a failed grant visible and requires an explicit retry with a fresh challenge", async () => {
    ipcMocks.workspaceTrustGrant.mockRejectedValueOnce(new Error("host-request-limit"))
    ipcMocks.workspaceTrustStatus.mockResolvedValue({ ...untrusted, challengeId: "grant-2" })
    const grant = useWorkspaceTrustStore.getState().requestWorkspaceGrant(untrusted)
    await useWorkspaceTrustStore.getState().confirmPrompt()
    expect(useWorkspaceTrustStore.getState()).toMatchObject({
        confirming: false,
        lastError: "host-request-limit",
        prompt: { challengeId: "grant-2" }
    })
    expect(ipcMocks.workspaceTrustGrant).toHaveBeenCalledTimes(1)
    ipcMocks.workspaceTrustGrant.mockResolvedValue({ state: "trusted", canonicalPath: "/w" })
    await useWorkspaceTrustStore.getState().confirmPrompt()
    await expect(grant).resolves.toBe(true)
    expect(ipcMocks.workspaceTrustGrant).toHaveBeenLastCalledWith("grant-2")
})

it("recovers a lost grant response by reading status without replaying the write", async () => {
    ipcMocks.workspaceTrustGrant.mockRejectedValue(new Error("connection lost"))
    ipcMocks.workspaceTrustStatus.mockResolvedValue({ state: "trusted", canonicalPath: "/w" })
    const grant = useWorkspaceTrustStore.getState().requestWorkspaceGrant(untrusted)
    await useWorkspaceTrustStore.getState().confirmPrompt()
    await expect(grant).resolves.toBe(true)
    expect(ipcMocks.workspaceTrustGrant).toHaveBeenCalledTimes(1)
    expect(useWorkspaceTrustStore.getState().prompt).toBeNull()
})

it("blocks duplicate confirmation even when status refreshes during the grant", async () => {
    let complete!: (value: unknown) => void
    ipcMocks.workspaceTrustGrant.mockReturnValue(new Promise((resolve) => { complete = resolve }))
    ipcMocks.workspaceTrustStatus.mockResolvedValue(untrusted)
    const grant = useWorkspaceTrustStore.getState().requestWorkspaceGrant(untrusted)
    const confirmation = useWorkspaceTrustStore.getState().confirmPrompt()
    await useWorkspaceTrustStore.getState().refreshStatus("/w")
    await useWorkspaceTrustStore.getState().confirmPrompt()
    expect(ipcMocks.workspaceTrustGrant).toHaveBeenCalledTimes(1)
    expect(useWorkspaceTrustStore.getState().confirming).toBe(true)
    complete({ state: "trusted", canonicalPath: "/w" })
    await confirmation
    await expect(grant).resolves.toBe(true)
})

it("does not settle a replacement prompt with a stale grant response", async () => {
    let complete!: (value: unknown) => void
    ipcMocks.workspaceTrustGrant.mockReturnValue(new Promise((resolve) => { complete = resolve }))
    const oldGrant = useWorkspaceTrustStore.getState().requestWorkspaceGrant(untrusted)
    const confirmation = useWorkspaceTrustStore.getState().confirmPrompt()
    const newGrant = useWorkspaceTrustStore.getState().requestWorkspaceGrant({
        ...untrusted, canonicalPath: "/other", challengeId: "other"
    })
    await expect(oldGrant).resolves.toBe(false)
    complete({ state: "trusted", canonicalPath: "/w" })
    await confirmation
    expect(useWorkspaceTrustStore.getState()).toMatchObject({
        confirming: false, prompt: { canonicalPath: "/other" }, statusByPath: {}
    })
    useWorkspaceTrustStore.getState().cancelPrompt()
    await expect(newGrant).resolves.toBe(false)
})

it("preserves a grant error when the recovery status read also fails", async () => {
    ipcMocks.workspaceTrustGrant.mockRejectedValue(new Error("disconnected"))
    ipcMocks.workspaceTrustStatus.mockRejectedValue(new Error("disconnected"))
    const grant = useWorkspaceTrustStore.getState().requestWorkspaceGrant(untrusted)
    await useWorkspaceTrustStore.getState().confirmPrompt()
    expect(useWorkspaceTrustStore.getState()).toMatchObject({
        confirming: false, lastError: "disconnected", prompt: { challengeId: "grant-1" }
    })
    useWorkspaceTrustStore.getState().cancelPrompt()
    await expect(grant).resolves.toBe(false)
})

it("returns the exact challenge id on confirm and grants workspace trust", async () => {
    ipcMocks.workspaceTrustGrant.mockResolvedValue({
        state: "trusted",
        canonicalPath: "/w"
    })
    const grant = useWorkspaceTrustStore.getState().requestWorkspaceGrant({
        state: "untrusted",
        challengeId: "grant-1",
        canonicalPath: "/w",
        repoPresent: true
    })
    expect(useWorkspaceTrustStore.getState().prompt).toMatchObject({
        kind: "workspace",
        challengeId: "grant-1"
    })
    await useWorkspaceTrustStore.getState().confirmPrompt()
    await expect(grant).resolves.toBe(true)
    expect(ipcMocks.workspaceTrustGrant).toHaveBeenCalledWith("grant-1")
})

it("revokes a trusted workspace and bumps the trust revision", async () => {
    ipcMocks.workspaceTrustRevoke.mockResolvedValue([])
    useWorkspaceTrustStore.setState({
        trustedWorkspaces: [
            { canonicalPath: "/w", fsIdentity: "id-1", grantedAt: "2026-01-01T00:00:00Z" }
        ],
        statusByPath: {
            "/w": { state: "trusted", canonicalPath: "/w" }
        }
    })
    await expect(useWorkspaceTrustStore.getState().revokeWorkspace("/w")).resolves.toEqual([])
    expect(ipcMocks.workspaceTrustRevoke).toHaveBeenCalledWith("/w")
    expect(useWorkspaceTrustStore.getState().trustedWorkspaces).toEqual([])
    expect(useWorkspaceTrustStore.getState().statusByPath["/w"]).toBeUndefined()
    expect(useWorkspaceTrustStore.getState().trustRevision).toBe(1)
})
