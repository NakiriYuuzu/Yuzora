import { expect, test, afterEach } from "vitest"
import { act, render, screen, fireEvent, waitFor } from "@testing-library/react"
import { mockIPC, clearMocks } from "@tauri-apps/api/mocks"
import { FileTree } from "./FileTree"
import { useWorkspaceStore } from "../state/workspaceStore"
import { useContextMenuStore } from "../state/contextMenuStore"
import { useFileTreeStore } from "../state/fileTreeStore"
import { useGitStore, initialGitState } from "../state/gitStore"
import { useUiStore, uiInitialState } from "../state/uiStore"
import type { FileNode, GitStatus } from "../lib/types"

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
    useFileTreeStore.setState({ trees: {}, preciseRevision: null })
    useWorkspaceStore.setState({ treeRevision: 0 })
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

test("Windows drive workspace 以 Git-relative path 命中 changed 標記", async () => {
    mockIPC((cmd, args) => {
        if (cmd === "list_dir") {
            const path = (args as { path: string }).path
            if (path === String.raw`C:\Work\Repo`) {
                return [{
                    name: "readme.md",
                    path: String.raw`C:\Work\Repo\readme.md`,
                    isDir: false
                }]
            }
            return []
        }
        if (cmd === "log_event") return null
    })
    useWorkspaceStore.setState({
        workspacePath: String.raw`C:\Work\Repo`,
        groups: [{ tabs: [], activePath: null }],
        activeGroupIndex: 0
    })
    useGitStore.setState({
        environment: {
            status: "ready",
            root: String.raw`C:\Work\Repo`,
            version: "2.50.1"
        },
        status: {
            ...makeStatus(),
            unstaged: [{ path: "readme.md", origPath: null, status: "M" }]
        }
    })
    render(<FileTree />)
    await waitFor(() => expect(screen.getByText("readme.md")).toBeTruthy())
    fireEvent.click(screen.getByRole("button", { name: "Open diff readme.md" }))
    expect(useUiStore.getState().gitSelectedPath).toBe("readme.md")
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

// --- #59 T4b：per-workspace 樹狀態保留＋精準失效 ---

function setWorkspaceState(path: string) {
    useWorkspaceStore.setState({
        workspacePath: path,
        groups: [{ tabs: [], activePath: null }],
        activeGroupIndex: 0
    })
}

test("A→B→A：展開狀態與子層從快取立即復原（切回時不等待重新 list）", async () => {
    let blocked = false
    const fs: Record<string, FileNode[]> = {
        "/a": [{ name: "src", path: "/a/src", isDir: true }],
        "/a/src": [{ name: "main.ts", path: "/a/src/main.ts", isDir: false }],
        "/b": [{ name: "b.md", path: "/b/b.md", isDir: false }]
    }
    mockIPC((cmd, args) => {
        if (cmd === "list_dir") {
            if (blocked) return new Promise(() => {})
            return fs[(args as { path: string }).path] ?? []
        }
        if (cmd === "log_event") return null
    })
    setWorkspaceState("/a")
    render(<FileTree />)
    await waitFor(() => expect(screen.getByText("src")).toBeTruthy())
    fireEvent.click(screen.getByText("src"))
    await waitFor(() => expect(screen.getByText("main.ts")).toBeTruthy())

    act(() => setWorkspaceState("/b"))
    await waitFor(() => expect(screen.getByText("b.md")).toBeTruthy())
    expect(screen.queryByText("main.ts")).toBeNull()

    // 切回 A：把 list_dir 全部卡住 → 畫面上的內容只可能來自快取 hydrate。
    blocked = true
    act(() => setWorkspaceState("/a"))
    expect(screen.getByText("src")).toBeTruthy()
    expect(screen.getByText("main.ts")).toBeTruthy()
})

test("外部變更精準失效：只 re-list 受影響目錄、展開保留、樹不 remount", async () => {
    const fs: Record<string, FileNode[]> = {
        "/a": [{ name: "src", path: "/a/src", isDir: true }],
        "/a/src": [{ name: "main.ts", path: "/a/src/main.ts", isDir: false }]
    }
    const calls: string[] = []
    mockIPC((cmd, args) => {
        if (cmd === "list_dir") {
            const path = (args as { path: string }).path
            calls.push(path)
            return fs[path] ?? []
        }
        if (cmd === "log_event") return null
    })
    setWorkspaceState("/a")
    const { container } = render(<FileTree />)
    await waitFor(() => expect(screen.getByText("src")).toBeTruthy())
    fireEvent.click(screen.getByText("src"))
    await waitFor(() => expect(screen.getByText("main.ts")).toBeTruthy())
    const listElement = container.querySelector("ul")

    fs["/a/src"] = [
        { name: "main.ts", path: "/a/src/main.ts", isDir: false },
        { name: "new.ts", path: "/a/src/new.ts", isDir: false }
    ]
    calls.length = 0
    await act(async () => {
        await useFileTreeStore.getState().invalidatePaths("/a", ["/a/src/new.ts"])
    })
    // 只有受影響目錄被 re-list（marker 讓 FileTree 跳過整樹 revalidate）。
    expect(calls).toEqual(["/a/src"])
    expect(screen.getByText("new.ts")).toBeTruthy()
    // 展開沒丟、<ul> 是同一個 DOM 節點——證明不再整樹 remount。
    expect(screen.getByText("main.ts")).toBeTruthy()
    expect(container.querySelector("ul")).toBe(listElement)
})

test("未標記的 treeRevision bump（explorer Refresh 相容路徑）→ 全樹 revalidate、展開保留", async () => {
    const fs: Record<string, FileNode[]> = {
        "/a": [{ name: "src", path: "/a/src", isDir: true }],
        "/a/src": [{ name: "main.ts", path: "/a/src/main.ts", isDir: false }]
    }
    mockIPC((cmd, args) => {
        if (cmd === "list_dir") return fs[(args as { path: string }).path] ?? []
        if (cmd === "log_event") return null
    })
    setWorkspaceState("/a")
    render(<FileTree />)
    await waitFor(() => expect(screen.getByText("src")).toBeTruthy())
    fireEvent.click(screen.getByText("src"))
    await waitFor(() => expect(screen.getByText("main.ts")).toBeTruthy())

    fs["/a"] = [
        { name: "src", path: "/a/src", isDir: true },
        { name: "root-new.md", path: "/a/root-new.md", isDir: false }
    ]
    fs["/a/src"] = [
        { name: "main.ts", path: "/a/src/main.ts", isDir: false },
        { name: "extra.ts", path: "/a/src/extra.ts", isDir: false }
    ]
    act(() => useWorkspaceStore.getState().refreshTree())
    await waitFor(() => expect(screen.getByText("root-new.md")).toBeTruthy())
    await waitFor(() => expect(screen.getByText("extra.ts")).toBeTruthy())
    expect(screen.getByText("main.ts")).toBeTruthy()
})

test("scrollTop per-workspace 保存並在切回時復原", async () => {
    const fs: Record<string, FileNode[]> = {
        "/a": [{ name: "a.md", path: "/a/a.md", isDir: false }],
        "/b": [{ name: "b.md", path: "/b/b.md", isDir: false }]
    }
    mockIPC((cmd, args) => {
        if (cmd === "list_dir") return fs[(args as { path: string }).path] ?? []
        if (cmd === "log_event") return null
    })
    setWorkspaceState("/a")
    const { container } = render(<FileTree />)
    await waitFor(() => expect(screen.getByText("a.md")).toBeTruthy())
    const scroller = container.querySelector("ul")?.parentElement as HTMLElement
    scroller.scrollTop = 80
    fireEvent.scroll(scroller)
    expect(useFileTreeStore.getState().trees["/a"]?.scrollTop).toBe(80)

    act(() => setWorkspaceState("/b"))
    await waitFor(() => expect(screen.getByText("b.md")).toBeTruthy())
    expect(scroller.scrollTop).toBe(0)

    act(() => setWorkspaceState("/a"))
    await waitFor(() => expect(scroller.scrollTop).toBe(80))
})

test("FilesNavContent ScrollArea viewport owns FileTree scrollTop restore", async () => {
    const { FilesNavContent } = await import("@/app/workbench/FilesNavContent")
    const fs: Record<string, FileNode[]> = {
        "/a": [{ name: "a.md", path: "/a/a.md", isDir: false }],
        "/b": [{ name: "b.md", path: "/b/b.md", isDir: false }]
    }
    mockIPC((cmd, args) => {
        if (cmd === "list_dir") return fs[(args as { path: string }).path] ?? []
        if (cmd === "log_event") return null
    })
    setWorkspaceState("/a")
    render(<FilesNavContent />)
    await waitFor(() => expect(screen.getByText("a.md")).toBeTruthy())
    const list = screen.getByText("a.md").closest("ul")
    const viewport = list?.closest('[data-slot="scroll-area-viewport"]') as HTMLElement | null
    expect(viewport).toBeTruthy()
    viewport!.scrollTop = 80
    fireEvent.scroll(viewport!)
    expect(useFileTreeStore.getState().trees["/a"]?.scrollTop).toBe(80)

    act(() => setWorkspaceState("/b"))
    await waitFor(() => expect(screen.getByText("b.md")).toBeTruthy())
    const restoredList = screen.getByText("b.md").closest("ul")
    const restoredViewport = restoredList?.closest(
        '[data-slot="scroll-area-viewport"]'
    ) as HTMLElement | null
    expect(restoredViewport).toBeTruthy()
    expect(restoredViewport!.scrollTop).toBe(0)

    act(() => setWorkspaceState("/a"))
    await waitFor(() => expect(screen.getByText("a.md")).toBeTruthy())
    const backList = screen.getByText("a.md").closest("ul")
    const backViewport = backList?.closest(
        '[data-slot="scroll-area-viewport"]'
    ) as HTMLElement | null
    await waitFor(() => expect(backViewport?.scrollTop).toBe(80))
})
