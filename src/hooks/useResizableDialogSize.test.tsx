import { act, render } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"
import { useEffect, useState } from "react"

import { dialogMinSize } from "@/lib/dialogSize"
import { useResizableDialogSize } from "./useResizableDialogSize"

function Probe({
    minW,
    minH,
    onSize,
}: {
    minW: number
    minH: number
    onSize: (size: { width: number; height: number }) => void
}) {
    const sizing = useResizableDialogSize({
        resizeId: "git-diff",
        minSize: dialogMinSize(minW, minH),
    })
    useEffect(() => {
        onSize(sizing.size)
    }, [onSize, sizing.size])
    return null
}

describe("useResizableDialogSize", () => {
    it("recomputes size when minSize changes without writing refs during render", () => {
        let latest = { width: 0, height: 0 }
        function Host() {
            const [minW, setMinW] = useState(320)
            return (
                <>
                    <Probe
                        minW={minW}
                        minH={240}
                        onSize={(size) => {
                            latest = size
                        }}
                    />
                    <button type="button" onClick={() => setMinW(480)}>
                        grow
                    </button>
                </>
            )
        }
        const { getByRole } = render(<Host />)
        expect(latest.width).toBeGreaterThanOrEqual(320)
        act(() => {
            getByRole("button", { name: "grow" }).click()
        })
        expect(latest.width).toBeGreaterThanOrEqual(480)
    })
})

it("uses the large Git size as the first pointer and keyboard origin and restores saved size", () => {
    const storage = new Map<string, string>()
    vi.stubGlobal("localStorage", { getItem: (key: string) => storage.get(key) ?? null, setItem: (key: string, value: string) => storage.set(key, value), removeItem: (key: string) => storage.delete(key) })
    let latest!: ReturnType<typeof useResizableDialogSize>
    function GitProbe() {
        const sizing = useResizableDialogSize({ resizeId: "git-diff" })
        useEffect(() => { latest = sizing }, [sizing])
        return null
    }
    const mounted = render(<GitProbe />)
    const initialWidth = window.innerWidth - 16
    expect(latest.size).toEqual({ width: initialWidth, height: window.innerHeight - 16 })
    const target = document.createElement("div")
    const pointer = (x: number) => ({ button: 0, pointerId: 1, clientX: x, clientY: 100, preventDefault() {}, stopPropagation() {}, currentTarget: target }) as unknown as React.PointerEvent<HTMLElement>
    act(() => latest.beginPointerResize("x", pointer(100)))
    expect(latest.size.width).toBe(initialWidth)
    act(() => latest.onPointerMove(pointer(98)))
    expect(latest.size.width).toBe(initialWidth - 4)
    act(() => latest.onPointerUp(pointer(98)))
    act(() => latest.onKeyboardResize("x", { key: "ArrowLeft", shiftKey: false, preventDefault() {}, stopPropagation() {} } as React.KeyboardEvent<HTMLElement>))
    expect(latest.size.width).toBe(initialWidth - 12)
    mounted.unmount()
    render(<GitProbe />)
    expect(latest.size.width).toBe(initialWidth - 12)
    vi.unstubAllGlobals()
})
