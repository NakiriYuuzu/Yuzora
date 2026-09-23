import { act, renderHook } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"
import { useHerdrOperation } from "./useHerdrOperation"

const mocks = vi.hoisted(() => ({ feature: vi.fn(), refresh: vi.fn(), bootstrap: vi.fn(), navigate: vi.fn(), close: vi.fn(), bump: vi.fn() }))
vi.mock("@/lib/herdrFeatures", () => ({ herdrFeature: mocks.feature }))
vi.mock("@/lib/herdrProvider", () => ({ sessionScope: (s: { name: string }) => s.name }))
vi.mock("@/lib/herdrFeatureNavigation", () => ({ closeRemovedHerdrPages: mocks.close, activateHerdrFeatureResult: mocks.navigate }))
vi.mock("@/state/herdrStore", () => ({ useHerdrStore: { getState: () => ({ refreshSessions: mocks.refresh, bootstrap: mocks.bootstrap, bumpTopologyRevision: mocks.bump, sessions: [{ name: "a", running: true }], runtimesBySession: { a: { connectionState: "ready" } } }) } }))
const request = { method: "integration.install", params: { target: "codex" } } as const

describe("HERDR operation ownership", () => {
  beforeEach(() => { vi.resetAllMocks(); mocks.feature.mockResolvedValue({ messages: ["installed"] }) })
  it("keeps mutation success when reconciliation fails and never retries", async () => {
    mocks.refresh.mockRejectedValue(new Error("offline"))
    const { result } = renderHook(() => useHerdrOperation("a"))
    let outcome: unknown
    await act(async () => { outcome = await result.current.run(request) })
    expect(outcome).toEqual({ messages: ["installed"] })
    expect(result.current.error).toBeNull()
    expect(result.current.refreshError).toContain("offline")
    expect(mocks.feature).toHaveBeenCalledTimes(1)
  })
  it("does not execute double clicks or publish a result into another Session", async () => {
    let complete!: (value: unknown) => void
    mocks.feature.mockReturnValue(new Promise(resolve => { complete = resolve }))
    const { result, rerender } = renderHook(({ scope }) => useHerdrOperation(scope), { initialProps: { scope: "a" } })
    let pending!: Promise<unknown>
    act(() => { pending = result.current.run(request) })
    await act(async () => { expect(await result.current.run(request)).toBeNull() })
    rerender({ scope: "b" })
    await act(async () => { complete({ messages: ["a completed"] }); await pending })
    expect(result.current.result).toBeNull()
    expect(mocks.navigate).not.toHaveBeenCalled()
    expect(mocks.feature).toHaveBeenCalledTimes(1)
  })
  it("reports operation failure separately and does not refresh or navigate", async () => {
    mocks.feature.mockRejectedValue(new Error("unsupported"))
    const { result } = renderHook(() => useHerdrOperation("a"))
    await act(async () => { await result.current.run(request) })
    expect(result.current.error).toBe("unsupported")
    expect(mocks.refresh).not.toHaveBeenCalled()
    expect(mocks.close).not.toHaveBeenCalled()
  })
})
