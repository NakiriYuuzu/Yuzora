import { create } from "zustand"

import {
    workspaceTrustExecutionChallenge,
    workspaceTrustGrant,
    workspaceTrustList,
    workspaceTrustRevoke,
    workspaceTrustStatus
} from "@/lib/ipc"
import type {
    TrustedWorkspace,
    WorkspaceExecutionChallenge,
    WorkspaceTrustStatus
} from "@/lib/types"

export type WorkspaceTrustPrompt =
    | {
          kind: "workspace"
          challengeId: string
          canonicalPath: string
          repoPresent: boolean
      }
    | {
          kind: "execute"
          challengeId: string
          canonicalPath: string
          command: string
          grantsTrust: boolean
      }

type WorkspaceResolver = (granted: boolean) => void
type ExecutionResolver = (challengeId: string | null) => void

interface WorkspaceTrustStore {
    statusByPath: Record<string, WorkspaceTrustStatus>
    trustedWorkspaces: TrustedWorkspace[]
    trustRevision: number
    prompt: WorkspaceTrustPrompt | null
    lastError: string | null
    refreshStatus: (path: string) => Promise<WorkspaceTrustStatus>
    refreshList: () => Promise<TrustedWorkspace[]>
    revokeWorkspace: (canonicalPath: string) => Promise<TrustedWorkspace[]>
    requestWorkspaceGrant: (status: WorkspaceTrustStatus) => Promise<boolean>
    requestExecution: (path: string, command: string) => Promise<string | null>
    confirmPrompt: () => Promise<void>
    cancelPrompt: () => void
}

let workspaceResolver: WorkspaceResolver | null = null
let executionResolver: ExecutionResolver | null = null

function settleWorkspace(granted: boolean): void {
    const resolve = workspaceResolver
    workspaceResolver = null
    resolve?.(granted)
}

function settleExecution(challengeId: string | null): void {
    const resolve = executionResolver
    executionResolver = null
    resolve?.(challengeId)
}

function supersedePending(): void {
    settleWorkspace(false)
    settleExecution(null)
}

export const useWorkspaceTrustStore = create<WorkspaceTrustStore>((set, get) => ({
    statusByPath: {},
    trustedWorkspaces: [],
    trustRevision: 0,
    prompt: null,
    lastError: null,

    refreshStatus: async (path) => {
        const status = await workspaceTrustStatus(path)
        set((current) => ({
            statusByPath: { ...current.statusByPath, [path]: status },
            lastError: null
        }))
        return status
    },

    refreshList: async () => {
        const trustedWorkspaces = await workspaceTrustList()
        set({ trustedWorkspaces, lastError: null })
        return trustedWorkspaces
    },

    revokeWorkspace: async (canonicalPath) => {
        const trustedWorkspaces = await workspaceTrustRevoke(canonicalPath)
        set((current) => {
            const statusByPath = { ...current.statusByPath }
            delete statusByPath[canonicalPath]
            return {
                trustedWorkspaces,
                statusByPath,
                lastError: null,
                trustRevision: current.trustRevision + 1
            }
        })
        return trustedWorkspaces
    },

    requestWorkspaceGrant: (status) => {
        if (!status.challengeId || !status.canonicalPath) return Promise.resolve(false)
        supersedePending()
        set({
            prompt: {
                kind: "workspace",
                challengeId: status.challengeId,
                canonicalPath: status.canonicalPath,
                repoPresent: status.repoPresent === true
            },
            lastError: null
        })
        return new Promise((resolve) => {
            workspaceResolver = resolve
        })
    },

    requestExecution: async (path, command) => {
        const challenge: WorkspaceExecutionChallenge = await workspaceTrustExecutionChallenge(
            path,
            command
        )
        supersedePending()
        set({
            prompt: {
                kind: "execute",
                challengeId: challenge.challengeId,
                canonicalPath: challenge.canonicalPath,
                command: challenge.command,
                grantsTrust: challenge.grantsTrust
            },
            lastError: null
        })
        return new Promise((resolve) => {
            executionResolver = resolve
        })
    },

    confirmPrompt: async () => {
        const prompt = get().prompt
        if (!prompt) return
        set({ prompt: null, lastError: null })
        if (prompt.kind === "execute") {
            settleExecution(prompt.challengeId)
            return
        }
        try {
            const status = await workspaceTrustGrant(prompt.challengeId)
            set((current) => ({
                lastError: null,
                statusByPath: {
                    ...current.statusByPath,
                    [prompt.canonicalPath]: status
                }
            }))
            settleWorkspace(status.state === "trusted")
        } catch (error) {
            set({
                lastError: error instanceof Error ? error.message : String(error)
            })
            settleWorkspace(false)
        }
    },

    cancelPrompt: () => {
        const prompt = get().prompt
        if (!prompt) return
        set({ prompt: null, lastError: null })
        if (prompt.kind === "workspace") settleWorkspace(false)
        else settleExecution(null)
    }
}))

export function requestDevServerAuthorization(
    path: string,
    command: string
): Promise<string | null> {
    return useWorkspaceTrustStore.getState().requestExecution(path, command)
}
