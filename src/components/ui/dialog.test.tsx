import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  dialogMinSize,
} from "@/components/ui/dialog"
import {
  DIALOG_SIZE_STORAGE_KEY,
  DEFAULT_DIALOG_SIZE_RATIO,
  loadDialogSizePreference,
} from "@/lib/dialogSize"

function installLocalStorage(): void {
  const values = new Map<string, string>()
  const storage = {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => void values.set(key, String(value)),
    removeItem: (key: string) => void values.delete(key),
    clear: () => values.clear(),
    key: (index: number) => [...values.keys()][index] ?? null,
    get length() {
      return values.size
    },
  }
  Object.defineProperty(globalThis, "localStorage", {
    value: storage,
    configurable: true,
    writable: true,
  })
}

function setViewport(width: number, height: number) {
  Object.defineProperty(window, "innerWidth", {
    configurable: true,
    writable: true,
    value: width,
  })
  Object.defineProperty(window, "innerHeight", {
    configurable: true,
    writable: true,
    value: height,
  })
  window.dispatchEvent(new Event("resize"))
}

function renderDialog() {
  return render(
    <Dialog open>
      <DialogContent
        resizeId="settings"
        minSize={dialogMinSize(320, 240)}
        showCloseButton={false}
      >
        <DialogHeader>
          <DialogTitle>Settings</DialogTitle>
          <DialogDescription>Dialog body</DialogDescription>
        </DialogHeader>
        <div>content</div>
      </DialogContent>
    </Dialog>,
  )
}

describe("DialogContent resizable sizing", () => {
  beforeEach(() => {
    installLocalStorage()
    setViewport(1000, 800)
  })

  afterEach(() => {
    cleanup()
    localStorage.clear()
    vi.restoreAllMocks()
  })

  it("opens at 80% of the current viewport", () => {
    renderDialog()
    const content = document.querySelector('[data-slot="dialog-content"]') as HTMLElement
    expect(content).toBeTruthy()
    expect(content.getAttribute("data-dialog-size-id")).toBe("settings")
    expect(content.style.width).toBe(`${1000 * DEFAULT_DIALOG_SIZE_RATIO}px`)
    expect(content.style.height).toBe(`${800 * DEFAULT_DIALOG_SIZE_RATIO}px`)
  })

  it("resizes from the right handle with pointer capture and persists on release", () => {
    const setPointerCapture = vi.spyOn(Element.prototype, "setPointerCapture")
    renderDialog()
    const handle = document.querySelector(
      '[data-slot="dialog-resize-handle"][data-axis="x"]',
    ) as HTMLElement
    const content = document.querySelector('[data-slot="dialog-content"]') as HTMLElement

    fireEvent.pointerDown(handle, { button: 0, pointerId: 7, clientX: 900, clientY: 400 })
    expect(setPointerCapture).toHaveBeenCalled()
    fireEvent.pointerMove(handle, { pointerId: 7, clientX: 950, clientY: 400 })
    // Centered dialog: edge tracks cursor, so width grows by 2 * deltaX.
    expect(content.style.width).toBe("900px")
    fireEvent.pointerUp(handle, { pointerId: 7, clientX: 950, clientY: 400 })

    expect(loadDialogSizePreference("settings").widthRatio).toBeCloseTo(0.9, 3)
    expect(localStorage.getItem(DIALOG_SIZE_STORAGE_KEY)).toContain("settings")
  })

  it("supports exact keyboard deltas, Shift deltas, and Home reset", () => {
    renderDialog()
    const xHandle = document.querySelector(
      '[data-slot="dialog-resize-handle"][data-axis="x"]',
    ) as HTMLElement
    const yHandle = document.querySelector(
      '[data-slot="dialog-resize-handle"][data-axis="y"]',
    ) as HTMLElement
    const bothHandle = document.querySelector(
      '[data-slot="dialog-resize-handle"][data-axis="both"]',
    ) as HTMLElement
    const content = document.querySelector('[data-slot="dialog-content"]') as HTMLElement

    fireEvent.keyDown(xHandle, { key: "ArrowRight" })
    expect(content.style.width).toBe("808px")
    fireEvent.keyDown(xHandle, { key: "ArrowRight", shiftKey: true })
    expect(content.style.width).toBe("840px")

    fireEvent.keyDown(yHandle, { key: "ArrowDown" })
    expect(content.style.height).toBe("648px")
    fireEvent.keyDown(yHandle, { key: "ArrowDown", shiftKey: true })
    expect(content.style.height).toBe("680px")

    fireEvent.keyDown(bothHandle, { key: "ArrowRight" })
    fireEvent.keyDown(bothHandle, { key: "ArrowDown" })
    expect(content.style.width).toBe("848px")
    expect(content.style.height).toBe("688px")

    fireEvent.keyDown(xHandle, { key: "Home" })
    expect(content.style.width).toBe("800px")
    expect(content.style.height).toBe("640px")
    expect(loadDialogSizePreference("settings")).toEqual({
      widthRatio: DEFAULT_DIALOG_SIZE_RATIO,
      heightRatio: DEFAULT_DIALOG_SIZE_RATIO,
    })
  })

  it("keeps exact 8px / 32px keyboard steps at adversarial viewport widths", () => {
    // 10,128 and 10,094 previously quantized four-decimal ratios into 9px / 33px jumps.
    for (const width of [10_128, 10_094] as const) {
      cleanup()
      localStorage.clear()
      setViewport(width, 800)
      renderDialog()
      const xHandle = document.querySelector(
        '[data-slot="dialog-resize-handle"][data-axis="x"]',
      ) as HTMLElement
      const content = document.querySelector('[data-slot="dialog-content"]') as HTMLElement

      const baseline = Number.parseFloat(content.style.width)
      expect(baseline).toBe(Math.round(width * DEFAULT_DIALOG_SIZE_RATIO))

      fireEvent.keyDown(xHandle, { key: "ArrowRight" })
      expect(Number.parseFloat(content.style.width)).toBe(baseline + 8)

      fireEvent.keyDown(xHandle, { key: "ArrowRight", shiftKey: true })
      expect(Number.parseFloat(content.style.width)).toBe(baseline + 8 + 32)
    }
  })

  it("clamps on a smaller viewport without overwriting the stored preference", () => {
    localStorage.setItem(
      DIALOG_SIZE_STORAGE_KEY,
      JSON.stringify({
        version: 1,
        sizes: { settings: { widthRatio: 0.99, heightRatio: 0.99 } },
      }),
    )
    renderDialog()
    const content = document.querySelector('[data-slot="dialog-content"]') as HTMLElement
    expect(content.style.width).toBe("968px")

    act(() => {
      setViewport(500, 400)
    })
    // max = viewport - 32 edge margin; preference itself stays 0.99
    expect(content.style.width).toBe("468px")
    expect(loadDialogSizePreference("settings")).toEqual({
      widthRatio: 0.99,
      heightRatio: 0.99,
    })
  })

  it("clamps an active drag when the viewport shrinks before release", () => {
    renderDialog()
    const handle = document.querySelector(
      '[data-slot="dialog-resize-handle"][data-axis="x"]',
    ) as HTMLElement
    const content = document.querySelector('[data-slot="dialog-content"]') as HTMLElement

    // 1000x800: drag width 800 -> 900.
    fireEvent.pointerDown(handle, { button: 0, pointerId: 11, clientX: 900, clientY: 400 })
    fireEvent.pointerMove(handle, { pointerId: 11, clientX: 950, clientY: 400 })
    expect(content.style.width).toBe("900px")
    expect(handle.getAttribute("aria-valuenow")).toBe("900")

    // Shrink before release. max width becomes 500 - 32 = 468.
    act(() => {
      setViewport(500, 400)
    })
    expect(Number.parseFloat(content.style.width)).toBeLessThanOrEqual(468)
    expect(Number.parseFloat(handle.getAttribute("aria-valuenow") ?? "0")).toBeLessThanOrEqual(468)
    expect(Number.parseFloat(handle.getAttribute("aria-valuemax") ?? "0")).toBe(468)

    fireEvent.pointerUp(handle, { pointerId: 11, clientX: 950, clientY: 400 })
    const preference = loadDialogSizePreference("settings")
    expect(preference.widthRatio).toBeCloseTo(468 / 500, 5)
    expect(preference.widthRatio).toBeLessThan(0.95)

    // Enlarge: preference reflects the clamped width, not the stale 900/1000=0.9
    // and not an oversized 968 from ratio 1.
    act(() => {
      setViewport(1000, 800)
    })
    expect(content.style.width).toBe("936px")
    expect(content.style.width).not.toBe("968px")
    expect(content.style.width).not.toBe("900px")
  })

  it("exposes accessible, in-bounds, visible resize handles", () => {
    renderDialog()
    expect(screen.getAllByRole("separator")).toHaveLength(3)
    const handles = [
      ...document.querySelectorAll('[data-slot="dialog-resize-handle"]'),
    ] as HTMLElement[]
    expect(handles).toHaveLength(3)
    for (const handle of handles) {
      expect(handle.className).not.toMatch(/translate-/)
      expect(handle.querySelector('[data-slot="dialog-resize-grip"]')).toBeTruthy()
      expect(handle.className).toMatch(/absolute/)
      expect(handle.className).toMatch(/cursor-/)
    }
    expect(
      document.querySelector('[data-slot="dialog-resize-handle"][data-axis="x"]')?.className,
    ).toMatch(/right-0/)
    expect(
      document.querySelector('[data-slot="dialog-resize-handle"][data-axis="y"]')?.className,
    ).toMatch(/bottom-0/)
    expect(
      document.querySelector('[data-slot="dialog-resize-handle"][data-axis="both"]')?.className,
    ).toMatch(/right-0/)
    expect(
      document.querySelector('[data-slot="dialog-resize-handle"][data-axis="both"]')?.className,
    ).toMatch(/bottom-0/)
  })

  it("does not jump a min-clamped size after release on an extreme viewport", () => {
    setViewport(10_000, 10_000)
    renderDialog()
    const handle = document.querySelector(
      '[data-slot="dialog-resize-handle"][data-axis="both"]',
    ) as HTMLElement
    const content = document.querySelector('[data-slot="dialog-content"]') as HTMLElement

    // Drag far left/up so width/height hit the 320x240 minSize used by renderDialog.
    fireEvent.pointerDown(handle, { button: 0, pointerId: 3, clientX: 5000, clientY: 5000 })
    fireEvent.pointerMove(handle, { pointerId: 3, clientX: -50_000, clientY: -50_000 })
    expect(content.style.width).toBe("320px")
    expect(content.style.height).toBe("240px")
    fireEvent.pointerUp(handle, { pointerId: 3, clientX: -50_000, clientY: -50_000 })

    const preference = loadDialogSizePreference("settings")
    expect(preference.widthRatio).toBe(320 / 10_000)
    expect(preference.heightRatio).toBe(240 / 10_000)
    expect(preference.widthRatio).toBeLessThan(0.05)

    cleanup()
    renderDialog()
    const reopened = document.querySelector('[data-slot="dialog-content"]') as HTMLElement
    expect(reopened.style.width).toBe("320px")
    expect(reopened.style.height).toBe("240px")
  })

  it("roundtrips the absolute minimum through a 10,000,000 viewport", () => {
    setViewport(10_000_000, 10_000_000)
    render(
      <Dialog open>
        <DialogContent resizeId="settings" showCloseButton={false}>
          <DialogHeader>
            <DialogTitle>Settings</DialogTitle>
            <DialogDescription>body</DialogDescription>
          </DialogHeader>
        </DialogContent>
      </Dialog>,
    )
    const handle = document.querySelector(
      '[data-slot="dialog-resize-handle"][data-axis="both"]',
    ) as HTMLElement
    const content = document.querySelector('[data-slot="dialog-content"]') as HTMLElement

    fireEvent.pointerDown(handle, { button: 0, pointerId: 9, clientX: 5_000_000, clientY: 5_000_000 })
    fireEvent.pointerMove(handle, { pointerId: 9, clientX: -1, clientY: -1 })
    expect(content.style.width).toBe("280px")
    expect(content.style.height).toBe("180px")
    fireEvent.pointerUp(handle, { pointerId: 9, clientX: -1, clientY: -1 })

    const preference = loadDialogSizePreference("settings")
    expect(preference.widthRatio).toBe(280 / 10_000_000)
    expect(preference.heightRatio).toBe(180 / 10_000_000)

    cleanup()
    setViewport(10_000_000, 10_000_000)
    render(
      <Dialog open>
        <DialogContent resizeId="settings" showCloseButton={false}>
          <DialogHeader>
            <DialogTitle>Settings</DialogTitle>
            <DialogDescription>body</DialogDescription>
          </DialogHeader>
        </DialogContent>
      </Dialog>,
    )
    const reopened = document.querySelector('[data-slot="dialog-content"]') as HTMLElement
    expect(reopened.style.width).toBe("280px")
    expect(reopened.style.height).toBe("180px")
  })
})
