import { act, renderHook } from "@testing-library/react"
import { beforeEach, expect, it, vi } from "vitest"

const relaunch = vi.hoisted(() => vi.fn())
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch }))
import { useWorkspaceStore } from "./workspaceStore"
import { useRestartYuzora } from "./useRestartYuzora"

function setDirty(dirty: boolean) {
  useWorkspaceStore.setState({ groups: [{ id: "g", tabs: [{ id: "t", dirty }] }] } as never)
}
beforeEach(() => { relaunch.mockReset(); relaunch.mockResolvedValue(undefined); setDirty(false) })

it("relaunches when no tab is dirty", async () => {
  const { result } = renderHook(() => useRestartYuzora())
  expect(result.current.blocked).toBe(false)
  await act(async () => result.current.restart())
  expect(relaunch).toHaveBeenCalledOnce()
})

it("is blocked and does not relaunch when a tab is dirty", async () => {
  setDirty(true)
  const { result } = renderHook(() => useRestartYuzora())
  expect(result.current.blocked).toBe(true)
  await act(async () => result.current.restart())
  expect(relaunch).not.toHaveBeenCalled()
})

it("re-checks dirtiness at click time", async () => {
  const { result } = renderHook(() => useRestartYuzora())
  setDirty(true)
  await act(async () => result.current.restart())
  expect(relaunch).not.toHaveBeenCalled()
})
