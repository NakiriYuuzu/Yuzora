import { afterEach, describe, expect, it, vi } from "vitest"
import { rm } from "node:fs/promises"
import { removeRuntimeFixture } from "./runtime-fixture-cleanup"

const fsMock = vi.hoisted(() => ({ rm: vi.fn() }))
vi.mock("node:fs/promises", () => ({ ...fsMock, default: fsMock }))
const remove = vi.mocked(rm)

afterEach(() => { vi.useRealTimers(); vi.resetAllMocks() })

describe("runtime fixture cleanup", () => {
  it("waits for a Windows handle to release before removing the fixture", async () => {
    vi.useFakeTimers()
    const locked = Object.assign(new Error("directory handle is still open"), { code: "EBUSY" })
    remove.mockRejectedValueOnce(locked).mockRejectedValueOnce(locked).mockResolvedValueOnce(undefined)
    const result = removeRuntimeFixture("fixture").catch(error => error)
    await vi.runAllTimersAsync()
    expect(await result).toBeUndefined()
    expect(remove).toHaveBeenCalledTimes(3)
  })

  it("fails with the original lock error after bounded retries", async () => {
    vi.useFakeTimers()
    const locked = Object.assign(new Error("locked forever"), { code: "EBUSY" })
    remove.mockRejectedValue(locked)
    const started = Date.now()
    const result = removeRuntimeFixture("fixture").catch(error => error)
    await vi.runAllTimersAsync()
    expect(await result).toBe(locked)
    expect(remove).toHaveBeenCalledTimes(11)
    expect(Date.now() - started).toBe(5500)
  })

  it("preserves unrelated filesystem failures without retrying", async () => {
    const failure = Object.assign(new Error("read-only filesystem"), { code: "EROFS" })
    remove.mockRejectedValue(failure)
    await expect(removeRuntimeFixture("fixture")).rejects.toBe(failure)
    expect(remove).toHaveBeenCalledTimes(1)
  })
})
