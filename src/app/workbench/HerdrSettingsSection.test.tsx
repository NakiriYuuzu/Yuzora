import { render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

const ipc = vi.hoisted(() => ({
  get: vi.fn(),
  set: vi.fn()
}))

vi.mock("@/lib/herdrIpc", () => ({
  herdrBinarySourceGet: ipc.get,
  herdrBinarySourceSet: ipc.set
}))

import { HerdrSettingsSection } from "./HerdrSettingsSection"

beforeEach(() => {
  ipc.get.mockReset().mockResolvedValue({
    configured: "default",
    active: "global",
    resolved: "global",
    available: true,
    path: "/Users/me/.local/bin/herdr",
    version: "0.8.0",
    protocol: 19,
    configuredAvailable: false,
    configuredPath: null,
    configuredReason: "This build does not include a managed Herdr binary",
    configuredVersion: null,
    configuredProtocol: null,
    configurationError: null,
    restartRequired: true
  })
  ipc.set.mockReset()
})

describe("HerdrSettingsSection", () => {
  it("separates active and configured-target diagnostics", async () => {
    render(<HerdrSettingsSection />)

    const global = await screen.findByRole("button", { name: /Global|全域/ })
    const managed = screen.getByRole("button", { name: /Yuzora-managed/i })
    expect(global).toHaveAttribute("aria-pressed", "false")
    expect(managed).toHaveAttribute("aria-pressed", "true")
    expect(screen.getByText("/Users/me/.local/bin/herdr")).toBeInTheDocument()
    expect(screen.getByText("0.8.0")).toBeInTheDocument()
    expect(screen.getByText("19")).toBeInTheDocument()
    expect(
      screen.getAllByText(/does not include a managed Herdr binary|尚未內建 managed Herdr binary/)
    ).not.toHaveLength(0)
    await waitFor(() => expect(ipc.get).toHaveBeenCalledTimes(1))
  })

  it("shows the effective managed source when global falls back", async () => {
    ipc.get.mockResolvedValue({
      configured: "global",
      active: "global",
      resolved: "default",
      available: true,
      path: String.raw`C:\Program Files\Yuzora\herdr\windows-x86_64\herdr.exe`,
      reason: "Herdr was not found on PATH; using Yuzora-managed Herdr",
      version: "0.8.0-preview.2026-08-04-d78e3d3b5126",
      protocol: 19,
      configuredAvailable: true,
      configuredPath: String.raw`C:\Program Files\Yuzora\herdr\windows-x86_64\herdr.exe`,
      configuredReason: "Herdr was not found on PATH; using Yuzora-managed Herdr",
      configuredVersion: "0.8.0-preview.2026-08-04-d78e3d3b5126",
      configuredProtocol: 19,
      configurationError: null,
      restartRequired: false
    })

    render(<HerdrSettingsSection />)

    expect(await screen.findByText("default", { selector: "dd" })).toBeInTheDocument()
    expect(screen.getByRole("button", { name: /Global|全域/ })).toHaveAttribute(
      "aria-pressed",
      "true"
    )
    expect(screen.getAllByText(/using Yuzora-managed Herdr/)).not.toHaveLength(0)
  })

  it("normalizes verbatim Windows diagnostic paths without mutating the DTO", async () => {
    const fixture = {
      configured: "default",
      active: "global",
      resolved: "global",
      available: true,
      path: String.raw`\\?\C:\Users\me\.local\bin\herdr.exe`,
      version: "0.8.0",
      protocol: 19,
      configuredAvailable: false,
      configuredPath: null,
      configuredReason: String.raw`Yuzora-managed Herdr binary is unavailable at \\?\C:\Program Files\Yuzora\herdr\windows-x86_64\herdr.exe`,
      configuredVersion: null,
      configuredProtocol: null,
      configurationError: null,
      restartRequired: true
    }
    ipc.get.mockResolvedValue(fixture)

    render(<HerdrSettingsSection />)

    expect(
      await screen.findByText(String.raw`C:\Users\me\.local\bin\herdr.exe`)
    ).toBeInTheDocument()
    expect(
      screen.getByText(
        String.raw`Yuzora-managed Herdr binary is unavailable at C:\Program Files\Yuzora\herdr\windows-x86_64\herdr.exe`
      )
    ).toBeInTheDocument()
    expect(screen.queryByText(/\\\?\\/)).not.toBeInTheDocument()
    expect(fixture.path).toBe(String.raw`\\?\C:\Users\me\.local\bin\herdr.exe`)
    expect(fixture.configuredReason).toBe(
      String.raw`Yuzora-managed Herdr binary is unavailable at \\?\C:\Program Files\Yuzora\herdr\windows-x86_64\herdr.exe`
    )
  })
})
