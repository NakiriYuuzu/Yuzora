import { afterEach, expect, it, vi } from "vitest"
import { act, cleanup, render, renderHook } from "@testing-library/react"
import { HerdrToolsHost } from "./HerdrToolsHost"
import { useHerdrToolsStore } from "@/state/herdrToolsStore"
import { useHerdrNativeStore } from "@/state/herdrNativeStore"
import { useAnyOverlayOpen } from "@/state/overlayStore"

vi.mock("@/app/workbench/herdr/HerdrToolsDialog", () => ({ default: () => null }))
vi.mock("@/app/workbench/herdr/HerdrNativeDialog", () => ({ default: () => null }))
afterEach(() => {
  cleanup()
  act(() => { useHerdrToolsStore.getState().close(); useHerdrNativeStore.setState({ selection: null }) })
})

it("registers HERDR dialogs with the overlay gate so native previews hide beneath them", () => {
  render(<HerdrToolsHost />)
  const overlay = renderHook(() => useAnyOverlayOpen())
  expect(overlay.result.current).toBe(false)
  act(() => useHerdrToolsStore.getState().open({ tool: "sessions", sessionName: "default" }))
  expect(overlay.result.current).toBe(true)
  act(() => useHerdrToolsStore.getState().close())
  expect(overlay.result.current).toBe(false)
  act(() => useHerdrNativeStore.setState({ selection: { sessionName: "default" } as never }))
  expect(overlay.result.current).toBe(true)
})
