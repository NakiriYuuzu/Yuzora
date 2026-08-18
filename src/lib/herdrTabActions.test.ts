import { beforeEach, describe, expect, it, vi } from "vitest"

const ipc = vi.hoisted(() => ({
  close: vi.fn(),
  rename: vi.fn()
}))

vi.mock("@/lib/herdrIpc", () => ({
  herdrTabClose: ipc.close,
  herdrTabRename: ipc.rename
}))

import { closeHerdrTabIdempotently } from "./herdrTabActions"

describe("Herdr tab runtime actions", () => {
  beforeEach(() => {
    ipc.close.mockReset().mockResolvedValue(undefined)
    ipc.rename.mockReset().mockResolvedValue(undefined)
  })

  it("preserves the WSL RuntimeTarget when closing a same-name tab", async () => {
    const ubuntu = { kind: "wsl" as const, distro: "Ubuntu" }
    await closeHerdrTabIdempotently("default", "tab-1", ubuntu)
    expect(ipc.close).toHaveBeenCalledWith({
      runtimeTarget: ubuntu,
      sessionName: "default",
      tabId: "tab-1"
    })
  })
})
