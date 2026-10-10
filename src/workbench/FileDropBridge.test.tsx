import { act, cleanup, render, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

const dragMock = vi.hoisted(() => ({
  handler: null as ((event: {
    payload:
      | { type: "drop"; paths: string[]; position: { x: number; y: number } }
      | { type: "enter" | "over"; paths?: string[]; position: { x: number; y: number } }
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

vi.mock("@/lib/platform", () => ({ isTauri: () => true, isWindowsPlatform: () => false }))
vi.mock("@/editor/documentRegistry", () => ({
  getDocument: (path: string) => documentMock.getDocument(path),
}))
vi.mock("@/lib/actionFeedback", () => ({
  showActionError: (action: string, error: unknown) => feedbackMock.showActionError(action, error),
}))
vi.mock("@/features/logs/userAction", () => ({
  logUserAction: vi.fn(async () => undefined),
}))
const importMock = vi.hoisted(() => ({
  importDroppedFiles: vi.fn(async (_workspace: string, _dir: string, _paths: string[]) => [] as string[]),
}))
vi.mock("./fileClipboard", () => ({
  importDroppedFiles: (workspace: string, dir: string, paths: string[]) =>
    importMock.importDroppedFiles(workspace, dir, paths),
}))

import { FileDropBridge } from "./FileDropBridge"
import { stubElementFromPoint } from "@/test/pointerDrag"
import { DROP_TARGET_ATTRIBUTE } from "@/lib/pointerDrag"
import { remoteFilePath } from "@/lib/runtimeIdentity"
import { registerTerminalDropTarget } from "@/terminal/terminalDropTargets"
import { useHostStore } from "@/state/hostStore"
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
  importMock.importDroppedFiles.mockClear()
  useWorkspaceStore.setState(initialWorkspaceState, true)
  useHostStore.setState({ configs: {} })
  useUiStore.setState(uiInitialState)
})

let restoreElementFromPoint: (() => void) | null = null

afterEach(() => {
  restoreElementFromPoint?.()
  restoreElementFromPoint = null
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

function mountTerminalLeaf() {
  const leaf = document.createElement("div")
  leaf.dataset.attachmentKey = "leaf-1"
  const inner = document.createElement("div")
  leaf.append(inner)
  document.body.append(leaf)
  const pasted: string[] = []
  const unregister = registerTerminalDropTarget("leaf-1", {
    scope: "main",
    canWrite: () => true,
    paste: async (text) => { pasted.push(text) },
    focus: () => undefined,
  })
  restoreElementFromPoint = stubElementFromPoint((point) => (point.x === 50 && point.y === 50 ? inner : null))
  return {
    leaf,
    pasted,
    cleanup: () => {
      unregister()
      leaf.remove()
    },
  }
}

it("pastes dropped paths into a terminal leaf instead of opening them", async () => {
  const terminal = mountTerminalLeaf()
  render(<FileDropBridge />)
  await waitFor(() => expect(dragMock.handler).not.toBeNull())

  act(() => {
    dragMock.handler!({
      payload: { type: "drop", paths: ["/outside/my file.ts"], position: { x: 50, y: 50 } },
    })
  })

  await waitFor(() => expect(terminal.pasted).toEqual(["/outside/my\\ file.ts "]))
  expect(documentMock.getDocument).not.toHaveBeenCalled()
  expect(useWorkspaceStore.getState().groups[0].tabs).toEqual([])
  terminal.cleanup()
})

it("converts the physical drop position by devicePixelRatio before hit-testing", async () => {
  const terminal = mountTerminalLeaf()
  const dpr = vi.spyOn(window, "devicePixelRatio", "get").mockReturnValue(2)
  render(<FileDropBridge />)
  await waitFor(() => expect(dragMock.handler).not.toBeNull())

  act(() => {
    dragMock.handler!({
      payload: { type: "drop", paths: ["/outside/a.ts"], position: { x: 100, y: 100 } },
    })
  })

  await waitFor(() => expect(terminal.pasted).toEqual(["/outside/a.ts "]))
  dpr.mockRestore()
  terminal.cleanup()
})

it("still opens drops that land outside any terminal", async () => {
  const terminal = mountTerminalLeaf()
  render(<FileDropBridge />)
  await waitFor(() => expect(dragMock.handler).not.toBeNull())

  act(() => {
    dragMock.handler!({
      payload: { type: "drop", paths: ["/outside/a.ts"], position: { x: 300, y: 300 } },
    })
  })

  await waitFor(() => expect(useWorkspaceStore.getState().groups[0].activePath).toBe("/outside/a.ts"))
  expect(terminal.pasted).toEqual([])
  terminal.cleanup()
})

it("marks the hovered terminal leaf on enter/over and clears it on leave and unmount", async () => {
  const terminal = mountTerminalLeaf()
  const { unmount } = render(<FileDropBridge />)
  await waitFor(() => expect(dragMock.handler).not.toBeNull())
  const handler = dragMock.handler!

  act(() => handler({ payload: { type: "enter", paths: ["/a"], position: { x: 50, y: 50 } } }))
  expect(terminal.leaf.getAttribute("data-pointer-drop-target")).toBe("inside")
  act(() => handler({ payload: { type: "over", position: { x: 300, y: 300 } } }))
  expect(terminal.leaf.hasAttribute("data-pointer-drop-target")).toBe(false)
  act(() => handler({ payload: { type: "over", position: { x: 50, y: 50 } } }))
  expect(terminal.leaf.getAttribute("data-pointer-drop-target")).toBe("inside")
  act(() => handler({ payload: { type: "leave" } }))
  expect(terminal.leaf.hasAttribute("data-pointer-drop-target")).toBe(false)

  act(() => handler({ payload: { type: "over", position: { x: 50, y: 50 } } }))
  unmount()
  expect(terminal.leaf.hasAttribute("data-pointer-drop-target")).toBe(false)
  terminal.cleanup()
})

function mountTree() {
  const zone = document.createElement("div")
  zone.dataset.fileTreeRoot = ""
  zone.innerHTML =
    '<ul><li><div><button data-tree-path="/w/src" data-tree-dir="true"></button></div>' +
    '<ul><li><div><button data-tree-path="/w/src/a.ts"></button></div></li></ul></li>' +
    '<li><div><button data-tree-path="/w/top.ts"></button></div></li></ul><p id="blank"></p>'
  document.body.append(zone)
  const q = (selector: string) => zone.querySelector(selector)!
  const folder = q('[data-tree-path="/w/src"]')
  const nested = q('[data-tree-path="/w/src/a.ts"]')
  const top = q('[data-tree-path="/w/top.ts"]')
  const blank = q("#blank")
  const hits: Record<string, Element> = { "10,10": folder, "20,20": nested, "30,30": top, "40,40": blank }
  restoreElementFromPoint = stubElementFromPoint((point) => hits[`${point.x},${point.y}`] ?? null)
  return { zone, folder, top, cleanup: () => zone.remove() }
}

const dropAt = (x: number, y: number, paths = ["/outside/a.txt"]) =>
  dragMock.handler!({ payload: { type: "drop", paths, position: { x, y } } })

it("imports a drop on a folder row into that folder and does not open tabs", async () => {
  useWorkspaceStore.setState({ workspacePath: "/w" })
  const tree = mountTree()
  render(<FileDropBridge />)
  await waitFor(() => expect(dragMock.handler).not.toBeNull())

  act(() => dropAt(10, 10))

  expect(importMock.importDroppedFiles).toHaveBeenCalledWith("/w", "/w/src", ["/outside/a.txt"])
  expect(documentMock.getDocument).not.toHaveBeenCalled()
  expect(useWorkspaceStore.getState().groups[0].tabs).toEqual([])
  tree.cleanup()
})

it("imports a drop on a file row into the folder that holds it", async () => {
  useWorkspaceStore.setState({ workspacePath: "/w" })
  const tree = mountTree()
  render(<FileDropBridge />)
  await waitFor(() => expect(dragMock.handler).not.toBeNull())

  act(() => dropAt(20, 20))
  expect(importMock.importDroppedFiles).toHaveBeenLastCalledWith("/w", "/w/src", ["/outside/a.txt"])
  act(() => dropAt(30, 30))
  expect(importMock.importDroppedFiles).toHaveBeenLastCalledWith("/w", "/w", ["/outside/a.txt"])
  tree.cleanup()
})

it("imports a drop on a filtered file result into the folder that holds it", async () => {
  useWorkspaceStore.setState({ workspacePath: "/w" })
  const zone = document.createElement("div")
  zone.dataset.fileTreeRoot = ""
  zone.innerHTML = '<ul><li><button data-file-result-path="/w/src/deep/a.ts"><span id="label"></span></button></li></ul>'
  document.body.append(zone)
  const result = zone.querySelector("[data-file-result-path]")!
  restoreElementFromPoint = stubElementFromPoint(() => zone.querySelector("#label"))
  render(<FileDropBridge />)
  await waitFor(() => expect(dragMock.handler).not.toBeNull())

  act(() => { dragMock.handler!({ payload: { type: "over", position: { x: 5, y: 5 } } }) })
  expect(result.hasAttribute(DROP_TARGET_ATTRIBUTE)).toBe(true)
  act(() => dropAt(5, 5))
  expect(importMock.importDroppedFiles).toHaveBeenCalledWith("/w", "/w/src/deep", ["/outside/a.txt"])
  zone.remove()
})

it("imports a drop on blank tree space into the workspace root", async () => {
  useWorkspaceStore.setState({ workspacePath: "/w" })
  const tree = mountTree()
  render(<FileDropBridge />)
  await waitFor(() => expect(dragMock.handler).not.toBeNull())

  act(() => dropAt(40, 40, ["/outside/a.txt", "/outside/b"]))

  expect(importMock.importDroppedFiles).toHaveBeenCalledWith("/w", "/w", ["/outside/a.txt", "/outside/b"])
  tree.cleanup()
})

it("highlights the target folder row, then the tree root, and clears on leave and drop", async () => {
  useWorkspaceStore.setState({ workspacePath: "/w" })
  const tree = mountTree()
  render(<FileDropBridge />)
  await waitFor(() => expect(dragMock.handler).not.toBeNull())
  const handler = dragMock.handler!

  act(() => handler({ payload: { type: "enter", paths: ["/a"], position: { x: 20, y: 20 } } }))
  expect(tree.folder.getAttribute("data-pointer-drop-target")).toBe("inside")
  act(() => handler({ payload: { type: "over", position: { x: 40, y: 40 } } }))
  expect(tree.folder.hasAttribute("data-pointer-drop-target")).toBe(false)
  expect(tree.zone.getAttribute("data-pointer-drop-target")).toBe("inside")
  act(() => handler({ payload: { type: "leave" } }))
  expect(tree.zone.hasAttribute("data-pointer-drop-target")).toBe(false)

  act(() => handler({ payload: { type: "over", position: { x: 10, y: 10 } } }))
  expect(tree.folder.getAttribute("data-pointer-drop-target")).toBe("inside")
  act(() => dropAt(10, 10))
  expect(tree.folder.hasAttribute("data-pointer-drop-target")).toBe(false)
  tree.cleanup()
})

it("lets a drop over the tree of an SSH workspace fall through to opening tabs", async () => {
  const remote = remoteFilePath("host-1", "/srv/app")
  useHostStore.setState({ configs: { "host-1": { hostId: "host-1", label: "box", kind: "ssh", helper: "h", binary: "b" } } })
  useWorkspaceStore.setState({ workspacePath: remote })
  const tree = mountTree()
  render(<FileDropBridge />)
  await waitFor(() => expect(dragMock.handler).not.toBeNull())

  act(() => dragMock.handler!({ payload: { type: "over", position: { x: 40, y: 40 } } }))
  expect(tree.zone.hasAttribute("data-pointer-drop-target")).toBe(false)
  act(() => dropAt(40, 40))

  expect(importMock.importDroppedFiles).not.toHaveBeenCalled()
  await waitFor(() => expect(documentMock.getDocument).toHaveBeenCalledWith("/outside/a.txt"))
  tree.cleanup()
})

it("highlights and imports a drop over the tree of a WSL host workspace", async () => {
  const remote = remoteFilePath("wsl-1", "/home/me/app")
  useHostStore.setState({ configs: { "wsl-1": { hostId: "wsl-1", label: "Ubuntu", kind: "wsl", distro: "Ubuntu", helper: "h", binary: "b" } } })
  useWorkspaceStore.setState({ workspacePath: remote })
  const tree = mountTree()
  render(<FileDropBridge />)
  await waitFor(() => expect(dragMock.handler).not.toBeNull())

  act(() => dragMock.handler!({ payload: { type: "over", position: { x: 40, y: 40 } } }))
  expect(tree.zone.getAttribute("data-pointer-drop-target")).toBe("inside")
  act(() => dropAt(40, 40))

  expect(importMock.importDroppedFiles).toHaveBeenCalledWith(remote, remote, ["/outside/a.txt"])
  expect(documentMock.getDocument).not.toHaveBeenCalled()
  tree.cleanup()
})

it("keeps SFTP-owned drops out of the tree import", async () => {
  useWorkspaceStore.setState({ workspacePath: "/w" })
  const tree = mountTree()
  const sftp = document.createElement("div")
  sftp.dataset.yuzoraOsFileDropTarget = "sftp-upload"
  sftp.getBoundingClientRect = () => ({
    x: 5, y: 5, left: 5, top: 5, right: 15, bottom: 15, width: 10, height: 10, toJSON: () => ({}),
  })
  document.body.append(sftp)
  render(<FileDropBridge />)
  await waitFor(() => expect(dragMock.handler).not.toBeNull())

  // (10, 10) hits a folder row but lies inside the SFTP surface rectangle.
  act(() => dropAt(10, 10))
  expect(importMock.importDroppedFiles).not.toHaveBeenCalled()
  expect(documentMock.getDocument).not.toHaveBeenCalled()
  sftp.remove()
  tree.cleanup()
})

it("keeps terminal drops out of the tree import", async () => {
  useWorkspaceStore.setState({ workspacePath: "/w" })
  const terminal = mountTerminalLeaf()
  render(<FileDropBridge />)
  await waitFor(() => expect(dragMock.handler).not.toBeNull())

  act(() => dropAt(50, 50, ["/outside/t.ts"]))

  await waitFor(() => expect(terminal.pasted).toEqual(["/outside/t.ts "]))
  expect(importMock.importDroppedFiles).not.toHaveBeenCalled()
  terminal.cleanup()
})

it("still opens drops that land outside the tree when a tree is mounted", async () => {
  useWorkspaceStore.setState({ workspacePath: "/w" })
  const tree = mountTree()
  render(<FileDropBridge />)
  await waitFor(() => expect(dragMock.handler).not.toBeNull())

  act(() => dropAt(300, 300, ["/outside/x.ts"]))

  await waitFor(() => expect(useWorkspaceStore.getState().groups[0].activePath).toBe("/outside/x.ts"))
  expect(importMock.importDroppedFiles).not.toHaveBeenCalled()
  tree.cleanup()
})
