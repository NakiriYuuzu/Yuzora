import { act } from "react"
import { beforeEach, describe, expect, it } from "vitest"
import { fireEvent, render, screen } from "@testing-library/react"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"

import { ContextMenu, placeMenu } from "@/app/workbench/ContextMenu"
import { useContextMenuStore, type ContextMenuKind } from "@/state/contextMenuStore"

const openMenu = (kind: ContextMenuKind, x = 40, y = 40) =>
  act(() => useContextMenuStore.getState().open(kind, x, y, {}))

beforeEach(() => {
  clearMocks()
  mockIPC((cmd) => (cmd === "log_event" ? null : undefined))
  useContextMenuStore.setState({ kind: null, x: 0, y: 0, payload: {} })
})

describe("placeMenu", () => {
  it("座標除以 zoom 換算 layout px", () => {
    expect(placeMenu(200, 100, 5, 2, 1024, 768)).toEqual({ left: 100, top: 50 })
  })

  it("右緣 clamp：選單不超出視窗右邊 8px", () => {
    // vw=1024, menu 寬 212 → left 上限 1024-212-8 = 804
    expect(placeMenu(1020, 10, 5, 1, 1024, 768).left).toBe(804)
  })

  it("下緣 clamp：選單不超出視窗下邊 8px", () => {
    // 5 entries → 高度估 5*33+10=175 → top 上限 768-175-8 = 585
    expect(placeMenu(10, 760, 5, 1, 1024, 768).top).toBe(585)
  })
})

describe("ContextMenu", () => {
  it("未開啟時不渲染任何東西", () => {
    render(<ContextMenu />)
    expect(screen.queryByTestId("context-menu")).toBeNull()
  })

  it("開啟時渲染該區域的項目與分隔線", () => {
    render(<ContextMenu />)
    openMenu("general")
    const menu = screen.getByTestId("context-menu")
    expect(menu.dataset.kind).toBe("general")
    expect(screen.getByRole("menuitem", { name: "Command palette…" })).toBeInTheDocument()
    expect(screen.getByRole("menuitem", { name: "Toggle sidebar" })).toBeInTheDocument()
    expect(menu.querySelectorAll('[role="separator"]')).toHaveLength(1)
  })

  it("danger 項目使用設計的紅字", () => {
    render(<ContextMenu />)
    openMenu("file")
    expect(screen.getByRole("menuitem", { name: "Delete" }).className).toContain("text-[#c2293f]")
  })

  it("Escape 關閉選單", () => {
    render(<ContextMenu />)
    openMenu("general")
    fireEvent.keyDown(window, { key: "Escape" })
    expect(screen.queryByTestId("context-menu")).toBeNull()
  })

  it("點選單外（pointerdown）關閉；點選單內不關閉", () => {
    render(<ContextMenu />)
    openMenu("general")
    fireEvent.pointerDown(screen.getByTestId("context-menu"))
    expect(screen.getByTestId("context-menu")).toBeInTheDocument()
    fireEvent.pointerDown(document.body)
    expect(screen.queryByTestId("context-menu")).toBeNull()
  })

  it("點擊項目後關閉選單（stub dispatch）", () => {
    render(<ContextMenu />)
    openMenu("general")
    fireEvent.click(screen.getByRole("menuitem", { name: "Refresh" }))
    expect(screen.queryByTestId("context-menu")).toBeNull()
  })
})
