import { render, screen } from "@testing-library/react"
import { describe, expect, it } from "vitest"
import { GitBadge } from "./fileRows"

describe("GitBadge", () => {
    it("keeps the shared dimensions and default worktree palette", () => {
        render(<GitBadge badge="D" />)
        const badge = screen.getByText("D")
        expect(badge).toHaveAttribute("aria-hidden", "true")
        expect(badge).toHaveClass("size-[18px]", "rounded-[6px]", "font-mono", "text-[10px]", "font-bold")
        expect(badge).toHaveStyle({ background: "var(--paper-3)", color: "var(--git-file-deleted)" })
    })

    it("preserves the commit palette without duplicating badge markup", () => {
        render(<GitBadge badge="D" colors={{ fg: "#c2293f", bg: "var(--danger-soft)" }} />)
        expect(screen.getByText("D")).toHaveStyle({ background: "var(--danger-soft)", color: "#c2293f" })
    })

    it("supports the diff copy palette while preserving unknown-status fallback", () => {
        render(<><GitBadge badge="C" colors={{ fg: "var(--git-file-untracked)", bg: "var(--amber-soft)" }} /><GitBadge badge="X" /></>)
        expect(screen.getByText("C")).toHaveStyle({ background: "var(--amber-soft)", color: "var(--git-file-untracked)" })
        expect(screen.getByText("X")).toHaveStyle({ background: "var(--paper-3)", color: "var(--git-file-deleted)" })
    })
})
