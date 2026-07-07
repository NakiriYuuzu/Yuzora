import { describe, expect, it } from "vitest"
import { render, screen } from "@testing-library/react"

import { PreviewFrame } from "./PreviewFrame"

describe("PreviewFrame", () => {
    it("sets iframe src only for localhost preview URLs", () => {
        const { rerender } = render(<PreviewFrame url="http://localhost:5173" reloadNonce={0} />)

        expect(screen.getByTitle("Live preview")).toHaveAttribute("src", "http://localhost:5173")

        // Non-local URLs never reach PreviewFrame in production (PreviewPanel routes
        // external https to the child webview); the guard here is defence in depth.
        rerender(<PreviewFrame url="https://example.com" reloadNonce={0} />)

        expect(screen.queryByTitle("Live preview")).not.toBeInTheDocument()
        expect(screen.getByText("Preview 只允許 localhost 或 127.0.0.1。")).toBeInTheDocument()
    })

    it("serves 127.0.0.1 static-server URLs (right-clicked HTML preview)", () => {
        render(<PreviewFrame url="http://127.0.0.1:4599/index.html" reloadNonce={0} />)
        expect(screen.getByTitle("Live preview")).toHaveAttribute(
            "src",
            "http://127.0.0.1:4599/index.html"
        )
    })

    it("remounts the iframe when reloadNonce changes", () => {
        const { rerender } = render(<PreviewFrame url="http://localhost:5173" reloadNonce={0} />)
        const first = screen.getByTitle("Live preview")

        rerender(<PreviewFrame url="http://localhost:5173" reloadNonce={1} />)

        expect(screen.getByTitle("Live preview")).not.toBe(first)
    })
})
