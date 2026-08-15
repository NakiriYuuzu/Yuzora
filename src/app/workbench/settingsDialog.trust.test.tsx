import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

import i18n from "@/lib/i18n"
import { useWorkspaceTrustStore } from "@/state/workspaceTrustStore"

const ipcMocks = vi.hoisted(() => ({
    workspaceTrustList: vi.fn(),
    workspaceTrustRevoke: vi.fn(),
}))

vi.mock("@tauri-apps/api/app", () => ({
    getVersion: vi.fn(async () => "0.0.7"),
}))
vi.mock("@tauri-apps/api/event", () => ({
    listen: vi.fn(async () => () => {}),
}))
vi.mock("@/lib/ipc", async (importOriginal) => ({
    ...(await importOriginal<typeof import("@/lib/ipc")>()),
    workspaceTrustList: (...args: unknown[]) => ipcMocks.workspaceTrustList(...args),
    workspaceTrustRevoke: (...args: unknown[]) => ipcMocks.workspaceTrustRevoke(...args),
}))

import { SettingsDialog } from "@/app/workbench/SettingsDialog"

function installLocalStorage(): void {
    const store = new Map<string, string>()
    const mock = {
        getItem: (key: string) => (store.has(key) ? store.get(key)! : null),
        setItem: (key: string, value: string) => void store.set(key, String(value)),
        removeItem: (key: string) => void store.delete(key),
        clear: () => store.clear(),
        key: (index: number) => [...store.keys()][index] ?? null,
        get length() {
            return store.size
        },
    }
    Object.defineProperty(globalThis, "localStorage", {
        value: mock,
        configurable: true,
        writable: true,
    })
}

beforeEach(() => {
    installLocalStorage()
    useWorkspaceTrustStore.setState({
        statusByPath: {},
        trustedWorkspaces: [],
        trustRevision: 0,
        prompt: null,
        lastError: null,
    })
    useWorkspaceTrustStore.getState().cancelPrompt()
    ipcMocks.workspaceTrustList.mockReset()
    ipcMocks.workspaceTrustRevoke.mockReset()
    ipcMocks.workspaceTrustList.mockResolvedValue([
        {
            canonicalPath: "/trusted/one",
            fsIdentity: "id-1",
            grantedAt: "2026-01-01T00:00:00Z",
        },
    ])
    ipcMocks.workspaceTrustRevoke.mockResolvedValue([])
})

afterEach(() => {
    cleanup()
})

it("lists trusted workspaces and revokes from the Safety settings pane", async () => {
    render(
        <SettingsDialog
            open
            onOpenChange={() => {}}
            theme="light"
            onThemeChange={() => {}}
            initialSection="safety"
        />,
    )

    expect(await screen.findByText("/trusted/one")).toBeInTheDocument()
    expect(ipcMocks.workspaceTrustList).toHaveBeenCalled()
    fireEvent.click(
        screen.getByRole("button", {
            name: i18n.t("settings.revokeWorkspaceNamed", {
                ns: "workbench",
                path: "/trusted/one",
            }),
        }),
    )
    await waitFor(() => expect(ipcMocks.workspaceTrustRevoke).toHaveBeenCalledWith("/trusted/one"))
    await waitFor(() => {
        expect(screen.queryByText("/trusted/one")).not.toBeInTheDocument()
    })
    expect(screen.getByText(i18n.t("settings.noTrustedWorkspaces", { ns: "workbench" }))).toBeInTheDocument()
    expect(useWorkspaceTrustStore.getState().trustRevision).toBe(1)
})
