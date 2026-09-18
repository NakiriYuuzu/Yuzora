import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { act, cleanup, renderHook, waitFor } from "@testing-library/react"
import { writeText } from "@tauri-apps/plugin-clipboard-manager"
import { previewInteractions, previewNavigationState, previewSelectElement } from "@/lib/ipc"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { usePreviewStore } from "@/state/previewStore"
import { usePreviewInteractions } from "./usePreviewInteractions"
import { enqueueNativePreviewOperation } from "./nativePreviewQueue"
import type { PreviewInteractionSnapshot } from "@/lib/previewTypes"

vi.mock("@/lib/platform", async original => ({ ...await original<typeof import("@/lib/platform")>(), isTauri: () => true }))
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn(async () => {}) }))
vi.mock("@/lib/ipc", async original => ({ ...await original<typeof import("@/lib/ipc")>(), previewInteractions: vi.fn(), previewNavigationState: vi.fn(), previewSelectElement: vi.fn() }))

const url = "https://example.test/"
const target = { workspace: "/workspace", url, nativeSessionId: "owner", external: true, previewVisible: true }
const selection = { url, selector: "#card", html: '<section id="card">Example</section>', text: "Example", width: 200, height: 100, styles: { display: "flex" }, truncated: false }

beforeEach(() => {
    vi.clearAllMocks()
    useWorkspaceStore.setState({ workspacePath: "/workspace" })
    usePreviewStore.getState().reset()
    usePreviewStore.getState().navigate("/workspace", url)
    usePreviewStore.getState().recordNativeOpen("/workspace", url, "owner")
    vi.mocked(previewNavigationState).mockResolvedValue({ sessionId: "owner", url, canGoBack: false, canGoForward: false })
    vi.mocked(previewInteractions).mockResolvedValue(null)
    vi.mocked(previewSelectElement).mockResolvedValue(undefined)
})
afterEach(async () => { cleanup(); await enqueueNativePreviewOperation(async () => {}) })

it("copies a validated selection once and reports completion", async () => {
    const { result } = renderHook(() => usePreviewInteractions(target))
    await act(async () => { await result.current.toggleElementSelection() })
    expect(previewSelectElement).toHaveBeenCalledWith("owner", true)
    vi.mocked(previewInteractions).mockResolvedValueOnce({ commands: [], selection, selecting: false })
    await waitFor(() => expect(writeText).toHaveBeenCalledOnce())
    expect(writeText).toHaveBeenCalledWith(expect.stringContaining("Selector: #card"))
    expect(writeText).toHaveBeenCalledWith(expect.stringContaining(`Source: ${url}`))
    expect(result.current.selecting).toBe(false)
    await waitFor(() => expect(result.current.selectionFeedback).toBeTruthy())
})

it("cancels native selection before hiding for a dialog", async () => {
    const { result, rerender } = renderHook(({ visible }) => usePreviewInteractions({ ...target, previewVisible: visible }), { initialProps: { visible: true } })
    await act(async () => { await result.current.toggleElementSelection() })
    rerender({ visible: false })
    await waitFor(() => expect(previewSelectElement).toHaveBeenLastCalledWith("owner", false))
    expect(result.current.selecting).toBe(false)
})

it("discards an in-flight selection after the workspace and native owner change", async () => {
    const { result, unmount } = renderHook(() => usePreviewInteractions(target))
    await act(async () => { await result.current.toggleElementSelection() })
    let finish!: (value: PreviewInteractionSnapshot) => void
    vi.mocked(previewInteractions).mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
    await waitFor(() => expect(finish).toBeTypeOf("function"))
    unmount()
    useWorkspaceStore.setState({ workspacePath: "/other" })
    usePreviewStore.getState().recordNativeOpen("/other", url, "new-owner")
    await act(async () => { finish({ commands: [], selection, selecting: false }); await enqueueNativePreviewOperation(async () => {}) })
    expect(writeText).not.toHaveBeenCalled()
})
