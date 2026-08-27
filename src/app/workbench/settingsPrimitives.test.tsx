import { cleanup, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { SettingsTextInput } from "./settingsPrimitives"

afterEach(() => {
  cleanup()
})

describe("SettingsTextInput", () => {
  it("composes the shared Input accessibility and interaction contract", () => {
    render(
      <SettingsTextInput
        label="Port"
        type="number"
        value="invalid"
        onChange={vi.fn()}
        disabled
        error="Enter a valid port"
        errorId="port-error"
      />,
    )

    const input = screen.getByRole("spinbutton", { name: "Port" })
    expect(input).toHaveAttribute("data-slot", "input")
    expect(input.className).toContain("focus-visible:ring-3")
    expect(input).toBeDisabled()
    expect(input).toHaveAttribute("aria-invalid", "true")
    expect(input).toHaveAttribute("aria-describedby", "port-error")
  })
})
