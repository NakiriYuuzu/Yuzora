import { act, render } from "@testing-library/react"
import { describe, expect, it } from "vitest"
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
