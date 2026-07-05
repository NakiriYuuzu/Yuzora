import { afterEach, describe, expect, it } from "vitest"
import { fireEvent, render, screen } from "@testing-library/react"

import { TerminalDrawer } from "@/app/workbench/TerminalDrawer"
import { useContextMenuStore } from "@/state/contextMenuStore"

afterEach(() => {
  useContextMenuStore.setState({ kind: null, x: 0, y: 0, payload: {} })
})

describe("TerminalDrawer content resize", () => {
  it("tracks the pointer 1:1 while dragging (transition suppressed) and re-enables it on release", () => {
    render(<TerminalDrawer visible={true} />)

    const handle = screen.getByTitle("Drag to resize")
    const content = document.querySelector(".yzs.overflow-y-auto.font-mono") as HTMLElement
    expect(content.style.height).toBe("228px")
    expect(content.style.transition).toContain("height 220ms")

    fireEvent.pointerDown(handle, { clientY: 300, pointerId: 1 })
    expect(content.style.transition).toBe("none")

    fireEvent.pointerMove(handle, { clientY: 250, pointerId: 1 })
    expect(content.style.height).toBe("278px")

    fireEvent.pointerMove(handle, { clientY: 200, pointerId: 1 })
    expect(content.style.height).toBe("328px")

    fireEvent.pointerUp(handle, { clientY: 200, pointerId: 1 })
    expect(content.style.transition).toContain("height 220ms")
  })

  it("clamps the dragged height to the configured min/max", () => {
    const origH = window.innerHeight
    window.innerHeight = 1000 // tall enough that MAX_HEIGHT (480) is the binding cap
    try {
      render(<TerminalDrawer visible={true} />)

      const handle = screen.getByTitle("Drag to resize")
      const content = document.querySelector(".yzs.overflow-y-auto.font-mono") as HTMLElement

      fireEvent.pointerDown(handle, { clientY: 0, pointerId: 1 })
      fireEvent.pointerMove(handle, { clientY: 2000, pointerId: 1 })
      expect(content.style.height).toBe("140px")

      fireEvent.pointerMove(handle, { clientY: -2000, pointerId: 1 })
      expect(content.style.height).toBe("480px")

      fireEvent.pointerUp(handle, { clientY: -2000, pointerId: 1 })
    } finally {
      window.innerHeight = origH
    }
  })

  it("caps the dragged height at 60% of a short window", () => {
    const origH = window.innerHeight
    window.innerHeight = 600 // 60% -> 360px cap, below MAX_HEIGHT
    try {
      render(<TerminalDrawer visible={true} />)

      const handle = screen.getByTitle("Drag to resize")
      const content = document.querySelector(".yzs.overflow-y-auto.font-mono") as HTMLElement

      fireEvent.pointerDown(handle, { clientY: 0, pointerId: 1 })
      fireEvent.pointerMove(handle, { clientY: -2000, pointerId: 1 })
      expect(content.style.height).toBe("360px")

      fireEvent.pointerUp(handle, { clientY: -2000, pointerId: 1 })
    } finally {
      window.innerHeight = origH
    }
  })
})

it("右鍵 terminal drawer 開啟 terminal 選單", () => {
  render(<TerminalDrawer visible />)
  fireEvent.contextMenu(screen.getByText("Terminal"))
  expect(useContextMenuStore.getState().kind).toBe("terminal")
})
