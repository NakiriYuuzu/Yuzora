import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import { ScrollArea } from "@/components/ui/scroll-area"

describe("ScrollArea composition", () => {
  it("keeps content layout classes on an inner content wrapper owned by the viewport", () => {
    render(
      <ScrollArea
        orientation="horizontal"
        type="always"
        contentClassName="flex h-[44px] w-max items-center gap-[3px]"
        viewportClassName="px-[4px]"
        data-testid="scroll-root"
      >
        <span>tab-a</span>
        <span>tab-b</span>
      </ScrollArea>
    )

    const root = screen.getByTestId("scroll-root")
    const viewport = root.querySelector(
      '[data-slot="scroll-area-viewport"]'
    ) as HTMLElement
    const content = root.querySelector(
      '[data-slot="scroll-area-content"]'
    ) as HTMLElement

    expect(viewport).toBeTruthy()
    expect(content).toBeTruthy()
    // Ownership: content is a descendant of this viewport and this root.
    expect(viewport.contains(content)).toBe(true)
    expect(root.contains(viewport)).toBe(true)
    expect(root.contains(content)).toBe(true)
    // Layout classes live on the content wrapper, not the scrollport.
    expect(viewport.className).toContain("px-[4px]")
    expect(viewport.className).not.toContain("flex")
    expect(content.className).toContain("flex")
    expect(content.className).toContain("w-max")
    expect(content.textContent).toContain("tab-a")
    expect(content.textContent).toContain("tab-b")
    // Horizontal orientation actually mounts a horizontal scrollbar.
    expect(
      root.querySelector(
        '[data-slot="scroll-area-scrollbar"][data-orientation="horizontal"]'
      )
    ).toBeTruthy()
  })

  it("mounts both vertical and horizontal scrollbars when orientation is both", () => {
    render(
      <ScrollArea
        orientation="both"
        type="always"
        data-testid="both-axes-root"
      >
        <div>wide and tall</div>
      </ScrollArea>
    )

    const root = screen.getByTestId("both-axes-root")
    const viewport = root.querySelector(
      '[data-slot="scroll-area-viewport"]'
    ) as HTMLElement
    expect(viewport).toBeTruthy()
    expect(viewport.contains(screen.getByText("wide and tall"))).toBe(true)
    expect(
      root.querySelector(
        '[data-slot="scroll-area-scrollbar"][data-orientation="vertical"]'
      )
    ).toBeTruthy()
    expect(
      root.querySelector(
        '[data-slot="scroll-area-scrollbar"][data-orientation="horizontal"]'
      )
    ).toBeTruthy()
  })

  it("inherits max-height from Root so max-h surfaces stay bounded", () => {
    render(
      <ScrollArea className="max-h-[398px]" data-testid="max-h-root">
        <div>long list</div>
      </ScrollArea>
    )

    const root = screen.getByTestId("max-h-root")
    const viewport = root.querySelector(
      '[data-slot="scroll-area-viewport"]'
    ) as HTMLElement

    expect(root).toHaveClass("max-h-[398px]", "overflow-hidden")
    expect(viewport.className).toMatch(/max-h-\[inherit\]/)
    expect(viewport.contains(screen.getByText("long list"))).toBe(true)
  })

  it("makes pure-reading viewports keyboard focusable when requested", () => {
    render(
      <ScrollArea
        focusable
        data-testid="focusable-root"
        viewportProps={{ "aria-label": "Rollback targets", role: "region" }}
      >
        <div>target</div>
      </ScrollArea>
    )

    const root = screen.getByTestId("focusable-root")
    const viewport = root.querySelector(
      '[data-slot="scroll-area-viewport"]'
    ) as HTMLElement
    expect(viewport.tabIndex).toBe(0)
    expect(viewport).toHaveAttribute("role", "region")
    expect(viewport).toHaveAttribute("aria-label", "Rollback targets")
    expect(viewport.contains(screen.getByText("target"))).toBe(true)
  })
})

it("keeps scrollbar gestures from activating an ancestor editor group or moving keyboard focus", () => {
  const activate = vi.fn()
  const view = render(<div onPointerDown={activate} onMouseDown={activate}>
    <input aria-label="current editor" />
    <ScrollArea type="always"><div>content</div></ScrollArea>
  </div>)
  const input = screen.getByRole("textbox")
  input.focus()
  const bar = view.container.querySelector('[data-slot="scroll-area-scrollbar"]')!
  fireEvent.pointerDown(bar, { pointerId: 1, button: 0, clientY: 20 })
  const mouse = new MouseEvent("mousedown", { bubbles: true, cancelable: true })
  bar.dispatchEvent(mouse)
  expect(activate).not.toHaveBeenCalled()
  expect(mouse.defaultPrevented).toBe(true)
  expect(document.activeElement).toBe(input)
})

it("keeps Radix track clicks and thumb dragging connected to the real viewport", async () => {
  const resizeCallbacks: Array<() => void> = []
  vi.stubGlobal("ResizeObserver", class {
    constructor(callback: () => void) { resizeCallbacks.push(callback) }
    observe() {} unobserve() {} disconnect() {}
  })
  const height = vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(100)
  const offset = vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(100)
  const contentHeight = vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockReturnValue(1000)
  try {
    const view = render(<ScrollArea type="always"><div>long content</div></ScrollArea>)
    const viewport = view.container.querySelector('[data-slot="scroll-area-viewport"]') as HTMLElement
    const bar = view.container.querySelector('[data-slot="scroll-area-scrollbar"]') as HTMLElement
    vi.spyOn(bar, "getBoundingClientRect").mockReturnValue({ top: 0, left: 0, width: 12, height: 100, bottom: 100, right: 12, x: 0, y: 0, toJSON: () => ({}) })
    act(() => resizeCallbacks.forEach((callback) => callback()))
    await waitFor(() => expect(bar.querySelector('[data-slot="scroll-area-thumb"]')).toBeTruthy())
    fireEvent.pointerDown(bar, { button: 0, pointerId: 1, clientY: 60 })
    expect(viewport.scrollTop).toBeGreaterThan(0)
    fireEvent.pointerUp(bar, { pointerId: 1 })
    const thumb = bar.querySelector('[data-slot="scroll-area-thumb"]')!
    fireEvent.pointerDown(thumb, { button: 0, pointerId: 2, clientY: 60 })
    const previous = viewport.scrollTop
    fireEvent.pointerMove(bar, { pointerId: 2, clientY: 80 })
    expect(viewport.scrollTop).toBeGreaterThan(previous)
    fireEvent.pointerUp(bar, { pointerId: 2 })
    viewport.scrollTop = 100
    const wheel = new WheelEvent("wheel", { deltaY: 20, bubbles: true, cancelable: true })
    bar.dispatchEvent(wheel)
    expect(viewport.scrollTop).toBe(120)
    expect(wheel.defaultPrevented).toBe(true)
    viewport.scrollTop = 0
    const boundaryWheel = new WheelEvent("wheel", { deltaY: -20, bubbles: true, cancelable: true })
    bar.dispatchEvent(boundaryWheel)
    expect(boundaryWheel.defaultPrevented).toBe(false)
    view.unmount()
  } finally { height.mockRestore(); offset.mockRestore(); contentHeight.mockRestore(); vi.unstubAllGlobals() }
})
