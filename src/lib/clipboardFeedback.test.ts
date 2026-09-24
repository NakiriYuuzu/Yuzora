import { beforeEach, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => ({ writeText: vi.fn(), success: vi.fn(), error: vi.fn() }))
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: mocks.writeText }))
vi.mock("sonner", () => ({ toast: { success: mocks.success, error: mocks.error } }))

import { copyTextInBackground, copyTextWithFeedback } from "./clipboardFeedback"

beforeEach(() => vi.clearAllMocks())

it("confirms an explicit copy with a toast", async () => {
    mocks.writeText.mockResolvedValue(undefined)
    await copyTextWithFeedback("/repo/src/a.ts")
    expect(mocks.writeText).toHaveBeenCalledWith("/repo/src/a.ts")
    expect(mocks.success).toHaveBeenCalledWith("Copied to clipboard", expect.objectContaining({ id: "yuzora-clipboard-copied" }))
})

it("keeps failures on the caller's error path and reports background failures", async () => {
    mocks.writeText.mockRejectedValue(new Error("denied"))
    await expect(copyTextWithFeedback("x")).rejects.toThrow("denied")
    expect(mocks.success).not.toHaveBeenCalled()
    copyTextInBackground("x")
    await vi.waitFor(() => expect(mocks.error).toHaveBeenCalledWith("Couldn't copy to the clipboard. Try again.", expect.anything()))
})
