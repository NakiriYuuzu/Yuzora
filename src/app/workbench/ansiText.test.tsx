import { render, screen } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import { AnsiText } from "./AnsiText"
import { MAX_ANSI_INPUT_BYTES, MAX_ANSI_SEGMENTS } from "./ansiSegments"

describe("AnsiText", () => {
  it("renders normal ANSI spans with stable keys and no overflow chrome", () => {
    const { container } = render(<AnsiText text={"\u001b[31mred\u001b[0m plain"} />)
    const spans = container.querySelectorAll("span")
    expect(spans).toHaveLength(2)
    expect(spans[0]).toHaveTextContent("red")
    expect(spans[0]).toHaveStyle({ color: "#cc6666" })
    expect(spans[1]).toHaveTextContent("plain")
    expect(container.innerHTML).not.toContain("<script")
    expect(container.innerHTML).not.toMatch(/overflow-(auto|y-auto|x-auto)/)
    expect(screen.queryByRole("status")).toBeNull()
  })

  it("announces ANSI complexity truncation accessibly", () => {
    let input = ""
    for (let index = 0; index < MAX_ANSI_SEGMENTS + 8; index += 1) {
      input += `\u001b[${31 + (index % 6)}m${String.fromCharCode(97 + (index % 26))}`
    }
    const { container } = render(<AnsiText text={input} />)
    expect(container.querySelectorAll("span").length).toBeLessThanOrEqual(MAX_ANSI_SEGMENTS + 1)
    expect(screen.getByRole("status")).toHaveTextContent(/too complex|過於複雜/)
  })

  it("announces too-large input accessibly without creating unbounded spans", () => {
    const { container } = render(<AnsiText text={"z".repeat(MAX_ANSI_INPUT_BYTES + 16)} />)
    expect(container.querySelectorAll("span").length).toBe(2)
    expect(screen.getByRole("status")).toHaveTextContent(/too large|過大/)
  })
})
