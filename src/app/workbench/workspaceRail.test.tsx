import { afterEach, describe, expect, it } from "vitest"
import { fireEvent, render } from "@testing-library/react"

import { WorkspaceRail } from "@/app/workbench/WorkspaceRail"
import { useContextMenuStore } from "@/state/contextMenuStore"

const MAC_UA =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)"

const renderRail = () =>
  render(
    <WorkspaceRail
      navCollapsed={false}
      onToggleNav={() => {}}
      onOpenSettings={() => {}}
      previewOpen={false}
      onTogglePreview={() => {}}
      terminalOpen={false}
      onToggleTerminalDrawer={() => {}}
    />
  )

afterEach(() => {
  delete (globalThis as { isTauri?: boolean }).isTauri
  // 移除測試蓋上的 own property，讓 jsdom 原本的 prototype getter 復原
  delete (window.navigator as { userAgent?: string }).userAgent
  useContextMenuStore.setState({ kind: null, x: 0, y: 0, payload: {} })
})

describe("WorkspaceRail 紅綠燈區塊", () => {
  it("不再渲染裝飾圓點或 drag region — 頂部空間由 AppShell 的標題帶統一讓出", () => {
    ;(globalThis as { isTauri?: boolean }).isTauri = true
    Object.defineProperty(window.navigator, "userAgent", { value: MAC_UA, configurable: true })

    const { container } = renderRail()

    expect(container.querySelector("[data-tauri-drag-region]")).toBeNull()
    expect(container.querySelector('[class*="ff5f57"]')).toBeNull()
  })
})

it("右鍵 rail 開啟 rail 選單", () => {
  const { container } = renderRail()
  fireEvent.contextMenu(container.querySelector("nav") as HTMLElement)
  expect(useContextMenuStore.getState().kind).toBe("rail")
})
