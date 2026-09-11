import { beforeEach, expect, it, vi } from "vitest"
const mocks = vi.hoisted(() => ({ check: vi.fn(), relaunch: vi.fn() }))
vi.mock("@/lib/updateChannel", () => ({ checkChannelUpdate: mocks.check, loadUpdateChannel: () => "auto", UPDATE_CHANNEL_KEY: "test.channel" }))
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: mocks.relaunch }))
import { useUpdateStore } from "./updateStore"

beforeEach(() => {
    useUpdateStore.getState().reset()
    vi.clearAllMocks()
})
it("discards and closes an old in-flight result after a channel change", async () => {
    let finish!: (value: unknown) => void
    mocks.check.mockReturnValueOnce(new Promise(resolve => { finish = resolve })).mockResolvedValueOnce(null)
    const pending = useUpdateStore.getState().checkForUpdates()
    useUpdateStore.getState().setChannel("preview")
    await useUpdateStore.getState().checkForUpdates()
    const stale = { version: "0.0.9", close: vi.fn().mockResolvedValue(undefined) }
    finish(stale)
    await pending
    expect(stale.close).toHaveBeenCalledOnce()
    expect(useUpdateStore.getState()).toMatchObject({ channel: "preview", status: "up-to-date", update: null })
})
it("allows retry after preview discovery fails", async () => {
    mocks.check.mockRejectedValueOnce(new Error("HTTP 403")).mockResolvedValueOnce(null)
    useUpdateStore.getState().setChannel("preview")
    await useUpdateStore.getState().checkForUpdates()
    expect(useUpdateStore.getState().status).toBe("error")
    await useUpdateStore.getState().checkForUpdates()
    expect(useUpdateStore.getState().status).toBe("up-to-date")
})
it("does not switch channels or start a check during a download", async () => {
    let finish!: () => void
    const update = { version: "0.0.10-beta.1", download: vi.fn(() => new Promise<void>(resolve => { finish = resolve })), close: vi.fn().mockResolvedValue(undefined) }
    mocks.check.mockResolvedValue(update)
    await useUpdateStore.getState().checkForUpdates()
    const pending = useUpdateStore.getState().downloadUpdate()
    useUpdateStore.getState().setChannel("preview")
    await useUpdateStore.getState().checkForUpdates()
    expect(useUpdateStore.getState()).toMatchObject({ channel: "auto", status: "downloading" })
    expect(mocks.check).toHaveBeenCalledOnce()
    finish()
    await pending
    expect(useUpdateStore.getState().status).toBe("downloaded")
})
