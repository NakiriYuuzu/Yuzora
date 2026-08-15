import { render, screen } from "@testing-library/react"
import { describe, expect, it } from "vitest"

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
