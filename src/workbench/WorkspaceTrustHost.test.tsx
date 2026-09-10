import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

import i18n from "@/lib/i18n"
import { useGitStore } from "@/state/gitStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { useWorkspaceTrustStore } from "@/state/workspaceTrustStore"
import { WorkspaceTrustHost } from "@/workbench/WorkspaceTrustHost"

const ipcMocks = vi.hoisted(() => ({
    workspaceTrustStatus: vi.fn(),
    workspaceTrustGrant: vi.fn(),
    workspaceTrustExecutionChallenge: vi.fn()
}))


vi.mock("@/lib/ipc", async (importOriginal) => ({
    ...(await importOriginal<typeof import("@/lib/ipc")>()),
    workspaceTrustStatus: (...args: unknown[]) => ipcMocks.workspaceTrustStatus(...args),
    workspaceTrustGrant: (...args: unknown[]) => ipcMocks.workspaceTrustGrant(...args),
    workspaceTrustExecutionChallenge: (...args: unknown[]) =>
        ipcMocks.workspaceTrustExecutionChallenge(...args)
}))

beforeEach(() => {
    useWorkspaceTrustStore.getState().cancelPrompt()
    useWorkspaceStore.setState({ workspacePath: null })
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
    ipcMocks.workspaceTrustExecutionChallenge.mockReset()
    vi.spyOn(useGitStore.getState(), "detect").mockResolvedValue()
})

afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
})

it("displays a verbatim Windows workspace path without changing grant identity", async () => {
    ipcMocks.workspaceTrustStatus.mockResolvedValue({
        state: "untrusted",
        canonicalPath: String.raw`\\?\C:\Apps\Tauri\Yuzora`,
        challengeId: "grant-win",
        repoPresent: true
    })
    ipcMocks.workspaceTrustGrant.mockResolvedValue({
        state: "trusted",
        canonicalPath: String.raw`\\?\C:\Apps\Tauri\Yuzora`,
        repoPresent: true
    })
    useWorkspaceStore.setState({ workspacePath: String.raw`\\?\C:\Apps\Tauri\Yuzora` })
    render(<WorkspaceTrustHost />)

    expect(await screen.findByRole("alertdialog")).toBeInTheDocument()
    expect(screen.getByText(String.raw`C:\Apps\Tauri\Yuzora`)).toBeInTheDocument()
    expect(screen.queryByText(/\\\?\\/)).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: i18n.t("workspaceTrust.grant", { ns: "workbench" }) }))
    await waitFor(() => expect(ipcMocks.workspaceTrustGrant).toHaveBeenCalledWith("grant-win"))
    await waitFor(() =>
        expect(useGitStore.getState().detect).toHaveBeenCalledWith(String.raw`\\?\C:\Apps\Tauri\Yuzora`)
    )
})

it("grants workspace trust for a detected repo and retries git detect", async () => {
    ipcMocks.workspaceTrustStatus.mockResolvedValue({
        state: "untrusted",
        canonicalPath: "/canonical/workspace",
        challengeId: "grant-1",
        repoPresent: true
    })
    ipcMocks.workspaceTrustGrant.mockResolvedValue({
        state: "trusted",
        canonicalPath: "/canonical/workspace",
        repoPresent: true
    })
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    render(<WorkspaceTrustHost />)

    expect(await screen.findByRole("alertdialog")).toBeInTheDocument()
    expect(screen.getByText("/canonical/workspace")).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: i18n.t("workspaceTrust.grant", { ns: "workbench" }) }))
    await waitFor(() => expect(ipcMocks.workspaceTrustGrant).toHaveBeenCalledWith("grant-1"))
    await waitFor(() => expect(useGitStore.getState().detect).toHaveBeenCalledWith("/workspace"))
})

it("keeps the dialog open and displays asynchronous grant failures", async () => {
    ipcMocks.workspaceTrustStatus.mockResolvedValue({
        state: "untrusted", canonicalPath: "/workspace", challengeId: "grant-retry", repoPresent: true
    })
    let fail!: (reason: Error) => void
    ipcMocks.workspaceTrustGrant.mockReturnValue(new Promise((_resolve, reject) => { fail = reject }))
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    render(<WorkspaceTrustHost />)
    await screen.findByRole("alertdialog")
    const confirm = screen.getByRole("button", { name: i18n.t("workspaceTrust.grant", { ns: "workbench" }) })
    fireEvent.click(confirm)
    expect(screen.getByRole("alertdialog")).toBeInTheDocument()
    expect(confirm).toBeDisabled()
    fail(new Error("host-request-limit"))
    expect(await screen.findByText("host-request-limit")).toBeInTheDocument()
    expect(confirm).toBeEnabled()
    expect(useGitStore.getState().detect).not.toHaveBeenCalled()
})

it("dismisses a stale prompt when switching to a different already-trusted workspace", async () => {
    ipcMocks.workspaceTrustStatus.mockImplementation(async (path: string) => {
        if (path === "/workspace-a") {
            return {
                state: "untrusted",
                canonicalPath: "/canonical/a",
                challengeId: "grant-a",
                repoPresent: true
            }
        }
        return {
            state: "trusted",
            canonicalPath: "/canonical/b",
            repoPresent: true
        }
    })
    useWorkspaceStore.setState({ workspacePath: "/workspace-a" })
    render(<WorkspaceTrustHost />)

    expect(await screen.findByRole("alertdialog")).toBeInTheDocument()
    expect(screen.getByText("/canonical/a")).toBeInTheDocument()

    useWorkspaceStore.setState({ workspacePath: "/workspace-b" })
    await waitFor(() => {
        expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument()
    })
    expect(screen.queryByText("/canonical/a")).not.toBeInTheDocument()
})

it("cancels the current grant prompt on Escape", async () => {
    ipcMocks.workspaceTrustStatus.mockResolvedValue({
        state: "untrusted",
        canonicalPath: "/canonical/workspace",
        challengeId: "grant-esc",
        repoPresent: true
    })
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    render(<WorkspaceTrustHost />)

    const dialog = await screen.findByRole("alertdialog")
    fireEvent.keyDown(dialog, { key: "Escape" })
    await waitFor(() => {
        expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument()
    })
    expect(ipcMocks.workspaceTrustGrant).not.toHaveBeenCalled()
})

it("moves focus into the trust dialog", async () => {
    ipcMocks.workspaceTrustStatus.mockResolvedValue({
        state: "untrusted",
        canonicalPath: "/canonical/workspace",
        challengeId: "grant-focus",
        repoPresent: true
    })
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    render(<WorkspaceTrustHost />)

    const dialog = await screen.findByRole("alertdialog")
    await waitFor(() => {
        expect(dialog.contains(document.activeElement)).toBe(true)
    })
})
