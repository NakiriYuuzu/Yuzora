import { beforeEach, expect, it, vi } from "vitest"

import { useWorkspaceTrustStore } from "@/state/workspaceTrustStore"

const ipcMocks = vi.hoisted(() => ({
    workspaceTrustStatus: vi.fn(),
    workspaceTrustGrant: vi.fn(),
    workspaceTrustExecutionChallenge: vi.fn(),
    workspaceTrustList: vi.fn(),
    workspaceTrustRevoke: vi.fn()
}))

vi.mock("@/lib/ipc", async (importOriginal) => ({
    ...(await importOriginal<typeof import("@/lib/ipc")>()),
    workspaceTrustStatus: (...args: unknown[]) => ipcMocks.workspaceTrustStatus(...args),
    workspaceTrustGrant: (...args: unknown[]) => ipcMocks.workspaceTrustGrant(...args),
    workspaceTrustExecutionChallenge: (...args: unknown[]) =>
        ipcMocks.workspaceTrustExecutionChallenge(...args),
    workspaceTrustList: (...args: unknown[]) => ipcMocks.workspaceTrustList(...args),
    workspaceTrustRevoke: (...args: unknown[]) => ipcMocks.workspaceTrustRevoke(...args)
}))

beforeEach(() => {
    useWorkspaceTrustStore.setState({
        statusByPath: {},
        trustedWorkspaces: [],
        trustRevision: 0,
        prompt: null,
        lastError: null
    })
    useWorkspaceTrustStore.getState().cancelPrompt()
    ipcMocks.workspaceTrustStatus.mockReset()
    ipcMocks.workspaceTrustGrant.mockReset()
    ipcMocks.workspaceTrustExecutionChallenge.mockReset()
    ipcMocks.workspaceTrustList.mockReset()
    ipcMocks.workspaceTrustRevoke.mockReset()
})

it("cancels a superseded execution challenge without spawning authority", async () => {
    ipcMocks.workspaceTrustExecutionChallenge.mockResolvedValue({
        challengeId: "exec-1",
        canonicalPath: "/w",
        command: "bun run dev",
        commandDigest: "abc",
        grantsTrust: true,
        trusted: false,
        expiresAt: 1
    })
    const first = useWorkspaceTrustStore.getState().requestExecution("/w", "bun run dev")
    await vi.waitFor(() => expect(useWorkspaceTrustStore.getState().prompt?.kind).toBe("execute"))
    const second = useWorkspaceTrustStore.getState().requestExecution("/w", "bun run dev")
    await expect(first).resolves.toBeNull()
    useWorkspaceTrustStore.getState().cancelPrompt()
    await expect(second).resolves.toBeNull()
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

it("returns the execution challenge without granting from the dialog", async () => {
    ipcMocks.workspaceTrustExecutionChallenge.mockResolvedValue({
        challengeId: "exec-2",
        canonicalPath: "/canonical/w",
        command: "bun run dev:web",
        commandDigest: "def",
        grantsTrust: true,
        trusted: false,
        expiresAt: 1
    })
    const pending = useWorkspaceTrustStore.getState().requestExecution("/w", "bun run dev:web")
    await vi.waitFor(() =>
        expect(useWorkspaceTrustStore.getState().prompt).toMatchObject({
            kind: "execute",
            command: "bun run dev:web"
        })
    )
    await useWorkspaceTrustStore.getState().confirmPrompt()
    await expect(pending).resolves.toBe("exec-2")
    expect(ipcMocks.workspaceTrustGrant).not.toHaveBeenCalled()
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
