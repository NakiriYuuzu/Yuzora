import { act, render } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

type PanelSize = { asPercentage: number; inPixels: number }
type ResizeCallback = (
  size: PanelSize,
  id: string | number | undefined,
  previous: PanelSize | undefined
) => void

const panelHarness = vi.hoisted(() => ({
  resize: null as ResizeCallback | null,
}))

vi.mock("react-resizable-panels", async () => {
  const React = await import("react")

  function Panel({ onResize }: { onResize?: ResizeCallback }) {
    React.useEffect(() => {
      panelHarness.resize = onResize ?? null
      return () => {
        panelHarness.resize = null
      }
    }, [onResize])
    return React.createElement("div", { "data-testid": "primitive-panel" })
  }

  return {
    Panel,
    Group: ({ children }: React.PropsWithChildren) => React.createElement("div", null, children),
    Separator: () => React.createElement("div"),
  }
})

const { ResizablePanel } = await import("@/components/ui/resizable")

function emitResize(size: PanelSize, previous?: PanelSize) {
  expect(panelHarness.resize).toBeTypeOf("function")
  panelHarness.resize?.(size, "files", previous)
}

describe("ResizablePanel collapse lifecycle", () => {
  it("ignores zero and NaN mount measurements until a real expanded baseline", () => {
    const onCollapse = vi.fn()
    const onExpand = vi.fn()
    render(
      <ResizablePanel onCollapse={onCollapse} onExpand={onExpand}>
        files
      </ResizablePanel>
    )

    act(() => emitResize({ asPercentage: 0, inPixels: 0 }))
    act(() =>
      emitResize(
        { asPercentage: Number.NaN, inPixels: 0 },
        { asPercentage: 0, inPixels: 0 }
      )
    )
    act(() =>
      emitResize(
        { asPercentage: 0, inPixels: 0 },
        { asPercentage: Number.NaN, inPixels: 0 }
      )
    )
    expect(onCollapse).not.toHaveBeenCalled()
    expect(onExpand).not.toHaveBeenCalled()

    act(() =>
      emitResize(
        { asPercentage: 24, inPixels: 240 },
        { asPercentage: 0, inPixels: 0 }
      )
    )
    expect(onCollapse).not.toHaveBeenCalled()
    expect(onExpand).not.toHaveBeenCalled()

    act(() =>
      emitResize(
        { asPercentage: 0, inPixels: 0 },
        { asPercentage: 24, inPixels: 240 }
      )
    )
    expect(onCollapse).toHaveBeenCalledTimes(1)

    act(() =>
      emitResize(
        { asPercentage: 24, inPixels: 240 },
        { asPercentage: 0, inPixels: 0 }
      )
    )
    expect(onExpand).toHaveBeenCalledTimes(1)
  })
})
