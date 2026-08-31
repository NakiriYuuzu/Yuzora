import { render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

const ipc = vi.hoisted(() => ({
  get: vi.fn(),
  set: vi.fn(),
  wslGet: vi.fn(),
  wslSet: vi.fn()
}))

const platform = vi.hoisted(() => ({
  isWindows: vi.fn()
}))

vi.mock("@/lib/herdrIpc", () => ({
  herdrBinarySourceGet: ipc.get,
  herdrBinarySourceSet: ipc.set,
  herdrWslIntegrationGet: ipc.wslGet,
  herdrWslIntegrationSet: ipc.wslSet
}))

vi.mock("@/lib/platform", () => ({
  isWindowsPlatform: platform.isWindows
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
  ipc.wslGet.mockReset().mockResolvedValue({
    platformSupported: true,
    bundleAvailable: true,
    active: false,
    linked: false,
    enabled: false,
    ownsRegistration: false,
    adapterStatus: "unknown",
    pluginVersion: "0.1.0",
    bundledPath: String.raw`C:\Program Files\Yuzora\herdr-plugins\yuzora-wsl-agents`,
    linkedPath: null,
    herdrPath: String.raw`C:\Program Files\Yuzora\herdr\windows-x86_64\herdr.exe`,
    reason: null
  })
  ipc.wslSet.mockReset()
  platform.isWindows.mockReset().mockReturnValue(false)
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

  it("installs and removes the bundled WSL Plugin from an explicit Windows setting", async () => {
    platform.isWindows.mockReturnValue(true)
    ipc.wslSet
      .mockResolvedValueOnce({
        platformSupported: true,
        bundleAvailable: true,
        active: true,
        linked: true,
        enabled: true,
        ownsRegistration: true,
        adapterStatus: "current",
        pluginVersion: "0.1.0",
        reason: null
      })
      .mockResolvedValueOnce({
        platformSupported: true,
        bundleAvailable: true,
        active: false,
        linked: false,
        enabled: false,
        ownsRegistration: false,
        adapterStatus: "absent",
        pluginVersion: null,
        reason: null
      })

    render(<HerdrSettingsSection />)

    const wsl = await screen.findByRole("switch", { name: /WSL Pi|WSL.*Pi/ })
    expect(wsl).not.toBeChecked()
    expect(screen.getByText(/Experimental|實驗/)).toBeInTheDocument()

    wsl.click()
    await waitFor(() => expect(ipc.wslSet).toHaveBeenCalledWith(true))
    await waitFor(() => expect(wsl).toBeChecked())
    expect(screen.getByTestId("herdr-wsl-status")).toHaveTextContent("0.1.0")

    wsl.click()
    await waitFor(() => expect(ipc.wslSet).toHaveBeenCalledWith(false))
    await waitFor(() => expect(wsl).not.toBeChecked())
  })

  it("represents an existing inactive owned registration as removable, not repairable", async () => {
    platform.isWindows.mockReturnValue(true)
    ipc.wslGet.mockResolvedValue({
      platformSupported: true,
      bundleAvailable: true,
      active: false,
      linked: true,
      enabled: false,
      ownsRegistration: true,
      adapterStatus: "outdated",
      pluginVersion: "0.1.0",
      reason: null
    })
    ipc.wslSet.mockResolvedValue({
      platformSupported: true,
      bundleAvailable: true,
      active: false,
      linked: false,
      enabled: false,
      ownsRegistration: false,
      adapterStatus: "absent",
      pluginVersion: null,
      reason: null
    })

    render(<HerdrSettingsSection />)

    const wsl = await screen.findByRole("switch", { name: /WSL Pi|WSL.*Pi/ })
    expect(wsl).toBeChecked()
    expect(screen.getByTestId("herdr-wsl-status")).toHaveTextContent(/clean up|清理/)
    wsl.click()
    await waitFor(() => expect(ipc.wslSet).toHaveBeenCalledWith(false))
    await waitFor(() => expect(wsl).not.toBeChecked())
  })

  it("fails closed when another root owns the WSL Plugin id", async () => {
    platform.isWindows.mockReturnValue(true)
    ipc.wslGet.mockResolvedValue({
      platformSupported: true,
      bundleAvailable: true,
      active: false,
      linked: true,
      enabled: true,
      ownsRegistration: false,
      adapterStatus: "unknown",
      linkedPath: String.raw`C:\foreign\yuzora-wsl-agents`,
      reason: "Plugin id yuzora-wsl-agents is registered from another root"
    })

    render(<HerdrSettingsSection />)

    const wsl = await screen.findByRole("switch", { name: /WSL Pi|WSL.*Pi/ })
    expect(wsl).toBeDisabled()
    expect(screen.getByRole("alert")).toHaveTextContent(
      String.raw`C:\foreign\yuzora-wsl-agents`
    )
    expect(ipc.wslSet).not.toHaveBeenCalled()
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
