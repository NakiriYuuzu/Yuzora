import { expect, test, afterEach } from "vitest"
import { render, screen, fireEvent, waitFor } from "@testing-library/react"
import { mockIPC, clearMocks } from "@tauri-apps/api/mocks"
import { FileTree } from "./FileTree"
import { useWorkspaceStore } from "../state/workspaceStore"
import { useContextMenuStore } from "../state/contextMenuStore"
import { useGitStore, initialGitState } from "../state/gitStore"
import { useUiStore, uiInitialState } from "../state/uiStore"
import type { GitStatus } from "../lib/types"

function makeStatus(): GitStatus {
    return {
        branch: "main",
        headOid: "0".repeat(40),
        detached: false,
        upstream: null,
        ahead: 0,
        behind: 0,
        staged: [],
        unstaged: [],
        untracked: [],
        conflicted: [],
        inProgress: null
    }
}

afterEach(() => {
    clearMocks()
    useContextMenuStore.setState({ request: null, x: 0, y: 0, availabilityRevision: 0 })
    useGitStore.setState(initialGitState)
    useUiStore.setState(uiInitialState)
})

test("載入根目錄並在點擊檔案時開 tab", async () => {
    mockIPC((cmd, args) => {
        if (cmd === "list_dir") {
            const path = (args as { path: string }).path
            if (path === "/w") {
                return [
                    { name: "src", path: "/w/src", isDir: true },
                    { name: "readme.md", path: "/w/readme.md", isDir: false }
                ]
            }
            return []
        }
        if (cmd === "log_event") return null
    })
    useWorkspaceStore.setState({
        workspacePath: "/w",
        groups: [{ tabs: [], activePath: null }],
        activeGroupIndex: 0
    })
    render(<FileTree />)
    await waitFor(() => expect(screen.getByText("readme.md")).toBeTruthy())
    fireEvent.click(screen.getByText("readme.md"))
    expect(useWorkspaceStore.getState().groups[0].tabs[0].path).toBe("/w/readme.md")
})

test("右鍵檔案列開啟 file 選單並帶 path payload", async () => {
    mockIPC((cmd, args) => {
        if (cmd === "list_dir") {
            const path = (args as { path: string }).path
            if (path === "/w") {
                return [{ name: "readme.md", path: "/w/readme.md", isDir: false }]
            }
            return []
        }
        if (cmd === "log_event") return null
    })
    useWorkspaceStore.setState({
        workspacePath: "/w",
        groups: [{ tabs: [], activePath: null }],
        activeGroupIndex: 0
    })
    render(<FileTree />)
    await waitFor(() => expect(screen.getByText("readme.md")).toBeTruthy())
    fireEvent.contextMenu(screen.getByText("readme.md"))
    expect(useContextMenuStore.getState().request).toMatchObject({
        kind: "file",
        workspacePath: "/w",
        path: "/w/readme.md",
        isDirectory: false,
        sourceGroupIndex: 0
    })
})

test("workspace 為 repo 子目錄時 rel 以 repo root 為基準（changed 標記/Open diff 生效）", async () => {
    // workspace = /repo/sub，repo root = /repo。git status 回報的 path 相對 repo root
    // （sub/readme.md），節點絕對路徑 /repo/sub/readme.md 須以 root 去前綴才對得上。
    mockIPC((cmd, args) => {
        if (cmd === "list_dir") {
            const path = (args as { path: string }).path
            if (path === "/repo/sub") {
                return [{ name: "readme.md", path: "/repo/sub/readme.md", isDir: false }]
            }
            return []
        }
        if (cmd === "log_event") return null
    })
    useWorkspaceStore.setState({
        workspacePath: "/repo/sub",
        groups: [{ tabs: [], activePath: null }],
        activeGroupIndex: 0
    })
    useGitStore.setState({
        environment: { status: "ready", root: "/repo", version: "2.50.1" },
        status: { ...makeStatus(), unstaged: [{ path: "sub/readme.md", origPath: null, status: "M" }] }
    })
    render(<FileTree />)
    await waitFor(() => expect(screen.getByText("readme.md")).toBeTruthy())
    // changed 標記生效 → Open diff 鈕存在，且以 repo-relative path 開 diff。
    fireEvent.click(screen.getByRole("button", { name: "Open diff readme.md" }))
    expect(useUiStore.getState().gitSelectedPath).toBe("sub/readme.md")
})

test("changed 檔案列的 Open diff 鈕呼叫 openDiffInGitMode", async () => {
    mockIPC((cmd, args) => {
        if (cmd === "list_dir") {
            const path = (args as { path: string }).path
            if (path === "/w") {
                return [{ name: "readme.md", path: "/w/readme.md", isDir: false }]
            }
            return []
        }
        if (cmd === "log_event") return null
    })
    useWorkspaceStore.setState({
        workspacePath: "/w",
        groups: [{ tabs: [], activePath: null }],
        activeGroupIndex: 0
    })
    useGitStore.setState({
        status: { ...makeStatus(), unstaged: [{ path: "readme.md", origPath: null, status: "M" }] }
    })
    render(<FileTree />)
    await waitFor(() => expect(screen.getByText("readme.md")).toBeTruthy())
    fireEvent.click(screen.getByRole("button", { name: "Open diff readme.md" }))
    expect(useUiStore.getState().mode).toBe("git")
    expect(useUiStore.getState().gitSelectedPath).toBe("readme.md")
})
