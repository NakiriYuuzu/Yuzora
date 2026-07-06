import { afterEach, describe, expect, it, vi } from "vitest"
import { act, fireEvent, render, screen } from "@testing-library/react"
import { openUrl } from "@tauri-apps/plugin-opener"

import { PreviewFrame } from "./PreviewFrame"

vi.mock("@tauri-apps/plugin-opener", () => ({
    openUrl: vi.fn(async () => undefined)
}))

afterEach(() => {
    vi.clearAllMocks()
    vi.useRealTimers()
})

describe("PreviewFrame", () => {
    it("sets iframe src only for localhost preview URLs", () => {
        const { rerender } = render(<PreviewFrame url="http://localhost:5173" reloadNonce={0} />)

        expect(screen.getByTitle("Live preview")).toHaveAttribute("src", "http://localhost:5173")

        rerender(<PreviewFrame url="https://example.com" reloadNonce={0} />)

        expect(screen.queryByTitle("Live preview")).not.toBeInTheDocument()
        expect(screen.getByText("Preview 只允許 localhost 或 127.0.0.1。")).toBeInTheDocument()
    })

    it("remounts the iframe when reloadNonce changes", () => {
        const { rerender } = render(<PreviewFrame url="http://localhost:5173" reloadNonce={0} />)
        const first = screen.getByTitle("Live preview")

        rerender(<PreviewFrame url="http://localhost:5173" reloadNonce={1} />)

        expect(screen.getByTitle("Live preview")).not.toBe(first)
    })

    it("shows blocked-frame guidance when no load event arrives before the timeout", () => {
        vi.useFakeTimers()
        render(<PreviewFrame url="http://localhost:5173" reloadNonce={0} timeoutMs={1000} />)

        act(() => {
            vi.advanceTimersByTime(1000)
        })

        expect(screen.getByText(/X-Frame-Options/)).toBeInTheDocument()
        expect(screen.getByRole("button", { name: "Open externally" })).toBeInTheDocument()
    })

    it("does not treat load as definitive failure and keeps external open available", () => {
        vi.useFakeTimers()
        render(<PreviewFrame url="http://localhost:5173" reloadNonce={0} timeoutMs={1000} />)
        const frame = screen.getByTitle("Live preview")
        Object.defineProperty(frame, "contentDocument", {
            configurable: true,
            get() {
                throw new Error("cross origin")
            }
        })

        fireEvent.load(frame)
        act(() => {
            vi.advanceTimersByTime(1000)
        })

        expect(screen.queryByText(/無法確認 iframe 是否完成載入/)).toBeInTheDocument()
        expect(screen.queryByText(/載入逾時/)).not.toBeInTheDocument()
    })

    it("opens the validated localhost URL externally", () => {
        render(<PreviewFrame url="http://localhost:5173" reloadNonce={0} />)

        fireEvent.click(screen.getByRole("button", { name: "Open externally" }))

        expect(openUrl).toHaveBeenCalledWith("http://localhost:5173")
    })
})
