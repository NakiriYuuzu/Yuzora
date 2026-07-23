import { describe, expect, it, vi } from "vitest"

import { installTerminalImePositioning } from "./terminalImePositioning"

type Subscription<T> = {
    subscribe: (listener: (event: T) => void) => { dispose: () => void }
    emit: (event: T) => void
    dispose: ReturnType<typeof vi.fn>
}

function subscription<T>(): Subscription<T> {
    let listener: ((event: T) => void) | undefined
    const dispose = vi.fn(() => {
        listener = undefined
    })

    return {
        subscribe: vi.fn((next) => {
            listener = next
            return { dispose }
        }),
        emit: (event) => listener?.(event),
        dispose
    }
}

function createTerminal() {
    const element = document.createElement("div")
    const screen = document.createElement("div")
    const textarea = document.createElement("textarea")
    screen.className = "xterm-screen"
    screen.append(textarea)
    element.append(screen)

    vi.spyOn(screen, "getBoundingClientRect").mockReturnValue({
        x: 0,
        y: 0,
        top: 0,
        right: 800,
        bottom: 480,
        left: 0,
        width: 800,
        height: 480,
        toJSON: () => ({})
    })

    const resize = subscription<{ cols: number; rows: number }>()
    const scroll = subscription<number>()
    const activeBuffer = {
        type: "alternate" as "normal" | "alternate",
        cursorX: 12,
        cursorY: 7,
        viewportY: 0,
        baseY: 0,
        length: 24,
        getLine: vi.fn(() => ({
            getCell: vi.fn(() => ({ getWidth: () => 1 }))
        }))
    }

    const terminal = {
        element,
        textarea,
        cols: 80,
        rows: 24,
        buffer: { active: activeBuffer },
        onResize: resize.subscribe,
        onScroll: scroll.subscribe
    } as unknown as Parameters<typeof installTerminalImePositioning>[0]

    return { terminal, textarea, activeBuffer, resize, scroll }
}

describe("installTerminalImePositioning", () => {
    it("anchors composition to the visible active-buffer cursor before xterm handles it", () => {
        const { terminal, textarea } = createTerminal()
        textarea.value = "existing composition state"
        textarea.style.left = "790px"
        textarea.style.top = "460px"

        let leftSeenByXterm = ""
        textarea.addEventListener("compositionstart", () => {
            leftSeenByXterm = textarea.style.left
        })

        installTerminalImePositioning(terminal)

        const event = new CompositionEvent("compositionstart", {
            bubbles: true,
            cancelable: true,
            data: "ㄋ"
        })
        textarea.dispatchEvent(event)

        expect(leftSeenByXterm).toBe("120px")
        expect(textarea.style.left).toBe("120px")
        expect(textarea.style.top).toBe("140px")
        expect(textarea.style.width).toBe("10px")
        expect(textarea.style.height).toBe("20px")
        expect(textarea.style.lineHeight).toBe("20px")
        expect(textarea.style.zIndex).toBe("-5")
        expect(textarea.value).toBe("existing composition state")
        expect(event.defaultPrevented).toBe(false)
    })

    it("refreshes the anchor after resize and scroll, then disposes every listener", () => {
        const { terminal, textarea, activeBuffer, resize, scroll } = createTerminal()
        const installed = installTerminalImePositioning(terminal)

        activeBuffer.cursorX = 3
        activeBuffer.cursorY = 4
        resize.emit({ cols: 80, rows: 24 })
        expect(textarea.style.left).toBe("30px")
        expect(textarea.style.top).toBe("80px")

        activeBuffer.cursorX = 5
        activeBuffer.cursorY = 6
        scroll.emit(0)
        expect(textarea.style.left).toBe("50px")
        expect(textarea.style.top).toBe("120px")

        installed.dispose()
        expect(resize.dispose).toHaveBeenCalledTimes(1)
        expect(scroll.dispose).toHaveBeenCalledTimes(1)

        textarea.style.left = "777px"
        textarea.dispatchEvent(new CompositionEvent("compositionstart"))
        expect(textarea.style.left).toBe("777px")
    })

    it("preserves xterm coordinates when the cursor or rendered grid cannot be measured", () => {
        const { terminal, textarea, activeBuffer } = createTerminal()
        textarea.style.left = "41px"
        textarea.style.top = "42px"
        activeBuffer.type = "normal"
        activeBuffer.baseY = 100
        activeBuffer.viewportY = 0

        installTerminalImePositioning(terminal)
        textarea.dispatchEvent(new CompositionEvent("compositionstart"))

        expect(textarea.style.left).toBe("41px")
        expect(textarea.style.top).toBe("42px")
    })

    it("anchors TUI composition to a visible overlay instead of the parked buffer cursor", () => {
        const { terminal, textarea, activeBuffer, resize, scroll } = createTerminal()
        const installed = installTerminalImePositioning(terminal, { anchorMode: "tui" })

        textarea.dispatchEvent(new CompositionEvent("compositionstart", { data: "" }))
        textarea.dispatchEvent(new CompositionEvent("compositionupdate", { data: "ㄋㄧ" }))

        const overlay = terminal.element?.querySelector<HTMLElement>("[data-yuzora-ime-overlay]")
        expect(overlay).not.toBeNull()
        expect(overlay).toHaveTextContent("ㄋㄧ")
        expect(overlay).toHaveStyle({ display: "block" })
        expect(textarea.style.left).toBe("8px")
        expect(textarea.style.top).toBe("452px")
        expect(textarea.style.zIndex).toBe("5")

        textarea.style.left = "777px"
        textarea.style.top = "778px"
        activeBuffer.cursorX = 70
        activeBuffer.cursorY = 1
        resize.emit({ cols: 80, rows: 24 })
        expect(textarea.style.left).toBe("8px")
        expect(textarea.style.top).toBe("452px")

        textarea.style.left = "779px"
        textarea.style.top = "780px"
        scroll.emit(1)
        expect(textarea.style.left).toBe("8px")
        expect(textarea.style.top).toBe("452px")
        expect(overlay).toHaveStyle({ display: "block" })

        textarea.dispatchEvent(new CompositionEvent("compositionend", { data: "你" }))
        expect(overlay?.style.display).toBe("none")

        installed.dispose()
        expect(terminal.element?.querySelector("[data-yuzora-ime-overlay]")).toBeNull()
    })
})
