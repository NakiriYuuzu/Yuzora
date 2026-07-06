import { it, expect, vi, beforeEach, afterEach } from "vitest"
import { render, cleanup } from "@testing-library/react"

const ptyCloseWorkspace = vi.fn()
vi.mock("../lib/ipc", () => ({
    ptyCloseWorkspace: (...a: unknown[]) => ptyCloseWorkspace(...a)
}))

import { TerminalBridge } from "./TerminalBridge"
import { useTerminalStore } from "../state/terminalStore"
import { useWorkspaceStore } from "../state/workspaceStore"

beforeEach(() => {
    ptyCloseWorkspace.mockReset()
    useTerminalStore.getState().reset()
    useWorkspaceStore.setState({ workspacePath: "/ws-a" })
})

afterEach(() => {
    cleanup()
    vi.clearAllMocks()
})

it("closes the previous workspace ptys and resets terminal state when the workspace changes", async () => {
    render(<TerminalBridge />)
    useTerminalStore.getState().addSession("/ws-a", {
        sessionId: "pty-1",
        title: "Terminal 1",
        workspace: "/ws-a",
        shell: "",
        cols: 80,
        rows: 24
    })

    useWorkspaceStore.setState({ workspacePath: "/ws-b" })

    await vi.waitFor(() => expect(ptyCloseWorkspace).toHaveBeenCalledWith("/ws-a"))
    expect(useTerminalStore.getState().sessionsForWorkspace("/ws-a")).toEqual([])
})
