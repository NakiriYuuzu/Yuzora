import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { cleanup, renderHook, waitFor } from "@testing-library/react"
vi.mock("@/lib/ipc", () => ({ previewResourceOpen: vi.fn(), previewResourceClose: vi.fn(async () => {}), previewClose: vi.fn(async () => {}) }))
vi.mock("@/lib/remoteFiles", () => ({ retainRemoteWorkspace: vi.fn(() => vi.fn(async () => {})), remotePreviewSource: vi.fn() }))
import { previewResourceOpen, previewResourceClose } from "@/lib/ipc"
import { remotePreviewSource } from "@/lib/remoteFiles"
import { remoteFilePath } from "@/lib/runtimeIdentity"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { usePreviewStore } from "@/state/previewStore"
import { browserTarget, canonicalFilePreviewUrl, closeFilePreviews, openHtmlPreview, resolveFilePreviewUrl } from "./filePreview"
import { reloadPreview } from "./previewCommands"
import { useRemotePreviewUrl } from "./useRemotePreviewUrl"

beforeEach(() => {
    vi.mocked(previewResourceOpen).mockReset()
    vi.mocked(previewResourceOpen).mockResolvedValue({ id: "0123456789abcdef0123456789abcdef", url: "yuzora-preview://0123456789abcdef0123456789abcdef/index.html" })
    useWorkspaceStore.setState({ workspacePath: "/project", workspaceCapabilityId: "cap", groups: [{ tabs: [], activePath: null }], activeGroupIndex: 0 })
    usePreviewStore.getState().reset()
})
afterEach(async () => { cleanup(); await closeFilePreviews() })

it.each(["close", "workspace switch"])("resolves a reopened HTML to its new native lease after %s", async (transition) => {
    const firstId = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    const secondId = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    vi.mocked(previewResourceOpen).mockResolvedValue({ id: firstId, url: `yuzora-preview://${firstId}/index.html` })
    await openHtmlPreview("/project", "/project/index.html", 0)
    const sourceUrl = usePreviewStore.getState().navForWorkspace("/project").url!
    const first = renderHook(() => useRemotePreviewUrl("/project", sourceUrl, 0))
    await waitFor(() => expect(first.result.current.url).toBe(`yuzora-preview://${firstId}/index.html`))
    first.unmount()
    if (transition === "close") useWorkspaceStore.setState({ groups: [{ tabs: [], activePath: null }] })
    else useWorkspaceStore.setState({ workspacePath: "/other" })
    await closeFilePreviews()
    useWorkspaceStore.setState({ workspacePath: "/project" })
    vi.mocked(previewResourceOpen).mockResolvedValue({ id: secondId, url: `yuzora-preview://${secondId}/index.html` })
    await openHtmlPreview("/project", "/project/index.html", 0)
    const reopenedSource = usePreviewStore.getState().navForWorkspace("/project").url!
    const reopened = renderHook(() => useRemotePreviewUrl("/project", reopenedSource, 0))
    expect(reopened.result.current.url).toBeNull()
    await waitFor(() => expect(reopened.result.current.url).toBe(`yuzora-preview://${secondId}/index.html`))
    expect(canonicalFilePreviewUrl(reopened.result.current.url!)).toBe(sourceUrl)
    expect(browserTarget(reopenedSource)).toEqual({ kind: "file", workspacePath: "/project", path: "/project/index.html" })
})

it("opens a local HTML through its capability and round-trips reserved filename characters", async () => {
    await openHtmlPreview("/project", "/project/中文 #100%.html", 0)
    expect(previewResourceOpen).toHaveBeenCalledWith({ kind: "local", workspace: "cap" }, "中文 #100%.html")
    const url = usePreviewStore.getState().navForWorkspace("/project").url!
    expect(browserTarget(url)).toEqual({ kind: "file", workspacePath: "/project", path: "/project/中文 #100%.html" })
    expect(browserTarget(url.replace("yuzora-preview://", "http://yuzora-preview."))).toEqual(browserTarget(url))
    expect(useWorkspaceStore.getState().groups[0].tabs[0].kind).toBe("preview")
})

it("binds remote resources to the source connection and rejects files outside the workspace", async () => {
    const root = remoteFilePath("wsl-host", "/mnt/c/project")
    const path = remoteFilePath("wsl-host", "/mnt/c/project/sub/index.htm", "/mnt/c/project")
    useWorkspaceStore.setState({ workspacePath: root })
    const assertCurrent = vi.fn()
    vi.mocked(remotePreviewSource).mockReturnValue({ source: { kind: "runtime", owner: { hostId: "wsl-host", generation: 3 }, workspace: "remote-cap" }, assertCurrent })
    await openHtmlPreview(root, path, 0)
    expect(previewResourceOpen).toHaveBeenCalledWith({ kind: "runtime", owner: { hostId: "wsl-host", generation: 3 }, workspace: "remote-cap" }, "sub/index.htm")
    expect(assertCurrent).toHaveBeenCalled()
    const url = usePreviewStore.getState().navForWorkspace(root).url!
    expect(browserTarget(url)).toMatchObject({ kind: "file", path })
    await expect(openHtmlPreview("/project", "/outside/index.html", 0)).rejects.toThrow("outside")
})

it("maps a Windows drive-root preview back to its canonical file identity", async () => {
    const root = remoteFilePath("win-host", "C:\\")
    const path = remoteFilePath("win-host", "C:\\sub\\index.html", "C:\\")
    useWorkspaceStore.setState({ workspacePath: root })
    vi.mocked(remotePreviewSource).mockReturnValue({ source: { kind: "runtime", owner: { hostId: "win-host", generation: 1 }, workspace: "remote-cap" }, assertCurrent: vi.fn() })
    await openHtmlPreview(root, path, 0)
    expect(previewResourceOpen).toHaveBeenCalledWith({ kind: "runtime", owner: { hostId: "win-host", generation: 1 }, workspace: "remote-cap" }, "sub/index.html")
    expect(browserTarget(usePreviewStore.getState().navForWorkspace(root).url!)).toEqual({ kind: "file", workspacePath: root, path })
})

it("rebinds saved navigation to a fresh capability after closing or switching workspaces", async () => {
    await openHtmlPreview("/project", "/project/index.html", 0)
    const sourceUrl = usePreviewStore.getState().navForWorkspace("/project").url!
    await closeFilePreviews()
    expect(browserTarget(sourceUrl)).toMatchObject({ kind: "file", path: "/project/index.html" })
    expect(await reloadPreview({ workspacePath: "/project", url: sourceUrl })).toBe("completed")
    expect(usePreviewStore.getState().navForWorkspace("/project").reloadNonce).toBe(1)
    const id = "abcdef0123456789abcdef0123456789"
    vi.mocked(previewResourceOpen).mockResolvedValue({ id, url: `yuzora-preview://${id}/index.html` })
    useWorkspaceStore.setState({ workspaceCapabilityId: "new-cap" })
    const rendered = await resolveFilePreviewUrl("/project", sourceUrl)
    expect(previewResourceOpen).toHaveBeenCalledWith({ kind: "local", workspace: "new-cap" }, "index.html")
    expect(rendered).not.toBe(sourceUrl)
    expect(canonicalFilePreviewUrl(rendered)).toBe(sourceUrl)
    expect(browserTarget(rendered)).toEqual(browserTarget(sourceUrl))
})

it("closes a late resource registration instead of navigating a different workspace", async () => {
    let finish!: (value: { id: string; url: string }) => void
    vi.mocked(previewResourceOpen).mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
    const opened = openHtmlPreview("/project", "/project/index.html", 0)
    await Promise.resolve()
    useWorkspaceStore.setState({ workspacePath: "/other" })
    finish({ id: "late", url: "yuzora-preview://late/index.html" })
    await expect(opened).rejects.toThrow("workspace changed")
    expect(previewResourceClose).toHaveBeenCalledWith("late")
    expect(usePreviewStore.getState().navForWorkspace("/other").url).toBeNull()
})
