import { act } from "react"
import { beforeEach, describe, expect, it } from "vitest"
import { fireEvent, render, screen } from "@testing-library/react"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"

import { ContextMenu, placeMenu } from "@/app/workbench/ContextMenu"
import type { ContextMenuRequest } from "@/app/workbench/contextMenuModel"
import { useContextMenuStore } from "@/state/contextMenuStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

const openMenu = (request: ContextMenuRequest, x = 40, y = 40) =>
  act(() => useContextMenuStore.getState().open(request, x, y))

beforeEach(() => {
  clearMocks()
  mockIPC((cmd) => (cmd === "log_event" ? null : undefined))
  useContextMenuStore.setState({ request: null, x: 0, y: 0, availabilityRevision: 0 })
  useWorkspaceStore.setState({
    workspacePath: "/w",
    groups: [{ tabs: [], activePath: null }],
    activeGroupIndex: 0,
  })
})

describe("placeMenu", () => {
  it("座標除以 zoom 換算 layout px", () => {
    expect(placeMenu(200, 100, 5, 2, 1024, 768)).toEqual({ left: 100, top: 50 })
  })

  it("右緣 clamp：選單不超出視窗右邊 8px", () => {
    expect(placeMenu(1020, 10, 5, 1, 1024, 768).left).toBe(804)
  })

  it("下緣 clamp：選單不超出視窗下邊 8px", () => {
    expect(placeMenu(10, 760, 5, 1, 1024, 768).top).toBe(585)
  })
})

describe("ContextMenu", () => {
  it("未開啟時不渲染任何東西", () => {
    render(<ContextMenu />)
    expect(screen.queryByTestId("context-menu")).toBeNull()
  })

  it("只渲染 normalized visible entries", () => {
    render(<ContextMenu />)
    openMenu({ kind: "general" })
    const menu = screen.getByTestId("context-menu")
    expect(menu.dataset.kind).toBe("general")
    expect(screen.getByRole("menuitem", { name: "Command palette…" })).toBeInTheDocument()
    expect(screen.queryByRole("menuitem", { name: "Refresh" })).toBeNull()
    expect(menu.querySelectorAll('[role="separator"]')).toHaveLength(1)
  })

  it("danger 項目使用設計的紅字", () => {
    render(<ContextMenu />)
    openMenu({
      kind: "file",
      workspacePath: "/w",
      path: "/w/a.ts",
      isDirectory: false,
      sourceGroupIndex: 0,
    })
    expect(screen.getByRole("menuitem", { name: "Delete" }).className).toContain("text-[#c2293f]")
  })

  it("disabled item 使用原生 semantics 並提供可存取原因", () => {
    useWorkspaceStore.setState({
      groups: [
        { tabs: [], activePath: null },
        {
          tabs: [{ path: "/w/a.ts", name: "a.ts", dirty: false, externallyModified: false }],
          activePath: "/w/a.ts",
        },
      ],
      activeGroupIndex: 1,
    })
    render(<ContextMenu />)
    openMenu({ kind: "tab", workspacePath: "/w", path: "/w/a.ts", groupIndex: 1 })
    const split = screen.getByRole("menuitem", { name: "Split and Move Right" })
    expect(split).toBeDisabled()
    expect(split).toHaveAccessibleDescription("A maximum of two editor groups is currently supported")
  })

  it("Escape 關閉選單", () => {
    render(<ContextMenu />)
    openMenu({ kind: "general" })
    fireEvent.keyDown(window, { key: "Escape" })
    expect(screen.queryByTestId("context-menu")).toBeNull()
  })

  it("點選單外（pointerdown）關閉；點選單內不關閉", () => {
    render(<ContextMenu />)
    openMenu({ kind: "general" })
    fireEvent.pointerDown(screen.getByTestId("context-menu"))
    expect(screen.getByTestId("context-menu")).toBeInTheDocument()
    fireEvent.pointerDown(document.body)
    expect(screen.queryByTestId("context-menu")).toBeNull()
  })

  it("enabled item click 通過 preflight 後關閉", () => {
    render(<ContextMenu />)
    openMenu({ kind: "general" })
    fireEvent.click(screen.getByRole("menuitem", { name: "Settings…" }))
    expect(screen.queryByTestId("context-menu")).toBeNull()
  })
})
