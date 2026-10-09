import { render, screen } from "@testing-library/react"
import { expect, it, vi } from "vitest"

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn() }))
vi.mock("react-i18next", () => ({ useTranslation: () => ({ t: (key: string) => key }) }))
import { DescribedError } from "./DescribedError"

it("renders long raw diagnostics inside the app ScrollArea instead of a bare overflow-auto block", () => {
  render(<DescribedError error={"x".repeat(400)} />)
  const area = screen.getByTestId("described-error-diagnostics")
  expect(area).toHaveAttribute("data-slot", "scroll-area")
  const pre = area.querySelector("pre")!
  expect(pre.className).not.toContain("overflow-auto")
  expect(pre.textContent).toBe("x".repeat(400))
})
