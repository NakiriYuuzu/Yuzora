import { describe, expect, it } from "vitest"
import config from "./src-tauri/tauri.conf.json"
import macosConfig from "./src-tauri/tauri.macos.conf.json"
import windowsConfig from "./src-tauri/tauri.windows.conf.json"
import linuxConfig from "./src-tauri/tauri.linux.conf.json"
import capabilities from "./src-tauri/capabilities/default.json"

// Tauri merges platform files with JSON Merge Patch, so a platform `windows`
// array replaces the shared one wholesale rather than adding `transparent`.
describe("glass window config", () => {
  it.each([["macOS", macosConfig], ["Windows", windowsConfig]] as const)("keeps the %s window identical to the shared one apart from transparency", (_, platform) => {
    expect(platform.app.windows).toEqual(config.app.windows.map(window => ({ ...window, transparent: true })))
  })

  it("keeps Linux and the shared window opaque and enables the private API macOS transparency needs", () => {
    expect(config.app.windows.every(window => !("transparent" in window))).toBe(true)
    expect("app" in linuxConfig).toBe(false)
    expect(config.app.macOSPrivateApi).toBe(true)
  })

  it("lets the main window toggle vibrancy", () => {
    expect(capabilities.permissions).toContain("core:window:allow-set-effects")
  })
})
