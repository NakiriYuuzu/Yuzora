import { expect, it, vi } from "vitest"
const mocks = vi.hoisted(() => ({ version: vi.fn(), check: vi.fn(), invoke: vi.fn() }))
vi.mock("@tauri-apps/api/app", () => ({ getVersion: mocks.version }))
vi.mock("@tauri-apps/plugin-updater", () => ({ check: mocks.check, Update: class { constructor(public metadata: unknown) {} } }))
vi.mock("./ipc", () => ({ invoke: mocks.invoke }))
vi.mock("./platform", () => ({ isTauri: () => true }))
import { checkChannelUpdate, resolveUpdateChannel } from "./updateChannel"

it("defaults prerelease installations to preview and stable installations to stable", () => {
    expect(resolveUpdateChannel("auto", "0.0.9-beta.3")).toBe("preview")
    expect(resolveUpdateChannel("auto", "0.0.9")).toBe("stable")
    expect(resolveUpdateChannel("stable", "0.0.10-beta.1")).toBe("stable")
    expect(resolveUpdateChannel("preview", "0.0.9")).toBe("preview")
})
it("routes stable checks through tagged signed release metadata", async () => {
    mocks.version.mockResolvedValue("0.0.9")
    mocks.invoke.mockResolvedValue(null)
    expect(await checkChannelUpdate("auto")).toBeNull()
    expect(mocks.invoke).toHaveBeenCalledWith("check_release_update", { includePreview: false })
    expect(mocks.check).not.toHaveBeenCalled()
})
it("routes preview checks through native discovery without allowing downgrades", async () => {
    mocks.version.mockResolvedValue("0.0.9-beta.3")
    mocks.invoke.mockResolvedValue({ rid: 7, version: "0.0.9", currentVersion: "0.0.9-beta.3", rawJson: {} })
    expect(await checkChannelUpdate("auto")).toMatchObject({ metadata: { rid: 7, version: "0.0.9" } })
    expect(mocks.invoke).toHaveBeenCalledWith("check_release_update", { includePreview: true })
})
it("falls back to the updater plugin when an older host lacks native discovery", async () => {
    mocks.version.mockResolvedValue("0.0.9")
    mocks.invoke.mockRejectedValue(new Error("Command check_release_update not found"))
    mocks.check.mockResolvedValue(null)
    expect(await checkChannelUpdate("stable")).toBeNull()
    expect(mocks.check).toHaveBeenCalledWith({ timeout: 20_000 })
})
