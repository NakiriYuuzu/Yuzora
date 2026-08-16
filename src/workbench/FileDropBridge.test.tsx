import { act, cleanup, render, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

const dragMock = vi.hoisted(() => ({
  handler: null as ((event: {
    payload:
      | { type: "drop"; paths: string[]; position: { x: number; y: number } }
      | { type: "leave" }
  }) => void) | null,
  forwardedHandler: null as ((event: { payload: { paths: string[] } }) => void) | null,
}))

const documentMock = vi.hoisted(() => ({
  getDocument: vi.fn(),
}))

const feedbackMock = vi.hoisted(() => ({
  showActionError: vi.fn(async (_action: string, _error: unknown) => undefined),
}))

vi.mock("@tauri-apps/api/event", () => ({
  listen: (_event: string, handler: typeof dragMock.forwardedHandler) => {
    dragMock.forwardedHandler = handler
    return Promise.resolve(() => {
      dragMock.forwardedHandler = null
    })
  },
}))

vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: (handler: typeof dragMock.handler) => {
      dragMock.handler = handler
      return Promise.resolve(() => {
        dragMock.handler = null
      })
    },
  }),
}))

vi.mock("@/lib/platform", () => ({ isTauri: () => true }))
vi.mock("@/editor/documentRegistry", () => ({
  getDocument: (path: string) => documentMock.getDocument(path),
}))
vi.mock("@/lib/actionFeedback", () => ({
  showActionError: (action: string, error: unknown) => feedbackMock.showActionError(action, error),
}))
vi.mock("@/features/logs/userAction", () => ({
  logUserAction: vi.fn(async () => undefined),
}))

import { FileDropBridge } from "./FileDropBridge"
import { uiInitialState, useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

const initialWorkspaceState = useWorkspaceStore.getState()

beforeEach(() => {
  dragMock.handler = null
  dragMock.forwardedHandler = null
  documentMock.getDocument.mockReset()
  documentMock.getDocument.mockResolvedValue({
    result: { kind: "full", content: "hello", size: 5, lineEnding: "lf" },
  })
  feedbackMock.showActionError.mockClear()
  useWorkspaceStore.setState(initialWorkspaceState, true)
  useUiStore.setState(uiInitialState)
})

afterEach(() => {
  cleanup()
  document.querySelectorAll("[data-yuzora-os-file-drop-target]").forEach((node) => node.remove())
})

it("opens dropped files in the active editor group and switches to Files", async () => {
  useWorkspaceStore.getState().splitRight()
  useWorkspaceStore.getState().setActiveGroup(1)
  useUiStore.getState().setMode("database")
  render(<FileDropBridge />)
  await waitFor(() => expect(dragMock.handler).not.toBeNull())

  act(() => {
    dragMock.handler!({
      payload: {
        type: "drop",
        paths: ["/outside/one.ts", "/outside/two.ts"],
        position: { x: 100, y: 80 },
      },
    })
  })

  await waitFor(() => {
    expect(useWorkspaceStore.getState().groups[1].tabs.map((tab) => tab.path)).toEqual([
      "/outside/one.ts",
      "/outside/two.ts",
    ])
  })
  expect(useWorkspaceStore.getState().groups[1].activePath).toBe("/outside/two.ts")
  expect(useWorkspaceStore.getState().groups[0].tabs).toEqual([])
  expect(useUiStore.getState().mode).toBe("files")
})

it("opens a file drop forwarded from the native Preview child webview", async () => {
  render(<FileDropBridge />)
  await waitFor(() => expect(dragMock.forwardedHandler).not.toBeNull())

  act(() => {
    dragMock.forwardedHandler!({ payload: { paths: ["/outside/from-preview.ts"] } })
  })

  await waitFor(() => {
    expect(useWorkspaceStore.getState().groups[0].activePath).toBe("/outside/from-preview.ts")
  })
})

it("opens valid files without creating a tab for a rejected directory path", async () => {
  documentMock.getDocument.mockImplementation(async (path: string) => {
    if (path === "/outside/folder") throw new Error("Is a directory")
    return { result: { kind: "full", content: "hello", size: 5, lineEnding: "lf" } }
  })
  render(<FileDropBridge />)
  await waitFor(() => expect(dragMock.handler).not.toBeNull())

  act(() => {
    dragMock.handler!({
      payload: {
        type: "drop",
        paths: ["/outside/folder", "/outside/file.ts"],
        position: { x: 100, y: 80 },
      },
    })
  })

  await waitFor(() => {
    expect(useWorkspaceStore.getState().groups[0].tabs.map((tab) => tab.path)).toEqual([
      "/outside/file.ts",
    ])
  })
  expect(feedbackMock.showActionError).toHaveBeenCalledTimes(1)
})

it("leaves drops over an SFTP-owned target to the SFTP upload handler", async () => {
  const target = document.createElement("div")
  target.dataset.yuzoraOsFileDropTarget = "sftp-upload"
  target.getBoundingClientRect = () => ({
    x: 10,
    y: 10,
    left: 10,
    top: 10,
    right: 110,
    bottom: 110,
    width: 100,
    height: 100,
    toJSON: () => ({}),
  })
  document.body.append(target)
  render(<FileDropBridge />)
  await waitFor(() => expect(dragMock.handler).not.toBeNull())

  act(() => {
    dragMock.handler!({
      payload: {
        type: "drop",
        paths: ["/outside/upload.ts"],
        position: { x: 50, y: 50 },
      },
    })
  })

  await Promise.resolve()
  expect(documentMock.getDocument).not.toHaveBeenCalled()
  expect(useWorkspaceStore.getState().groups[0].tabs).toEqual([])
})
