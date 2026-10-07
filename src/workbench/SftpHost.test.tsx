import { act, cleanup, render, screen } from "@testing-library/react"
import { afterEach, expect, it, vi } from "vitest"
import { SftpHost } from "./SftpHost"
import { useSftpStore } from "@/state/sftpStore"
import { useSshStore } from "@/state/sshStore"

const panel = vi.hoisted(() => ({ imported: vi.fn() }))
vi.mock("@/app/panels/SftpPanel", () => {
    panel.imported()
    return { SftpPanel: () => <div>SFTP files</div> }
})
vi.mock("@/app/workbench/HostList", () => ({ PasswordPromptDialog: () => null }))
vi.mock("@/lib/workspaceActions", () => ({ pickRemoteWorkspace: vi.fn() }))

afterEach(() => {
    cleanup()
    useSftpStore.getState().setPanelOpen(false)
})

it("imports the transfer panel only when opened and keeps the dialog size while loading", async () => {
    useSshStore.setState({ hosts: [], activeHostId: null })
    useSftpStore.getState().setPanelOpen(false)
    render(<SftpHost />)
    expect(panel.imported).not.toHaveBeenCalled()
    act(() => useSftpStore.getState().setPanelOpen(true))
    expect(screen.getByRole("dialog").className).toContain("h-[min(85vh,720px)]")
    expect(await screen.findByText("SFTP files")).toBeInTheDocument()
    expect(panel.imported).toHaveBeenCalledTimes(1)
    act(() => useSftpStore.getState().setPanelOpen(false))
    expect(screen.queryByText("SFTP files")).not.toBeInTheDocument()
    act(() => useSftpStore.getState().setPanelOpen(true))
    expect(await screen.findByText("SFTP files")).toBeInTheDocument()
    expect(panel.imported).toHaveBeenCalledTimes(1)
})
