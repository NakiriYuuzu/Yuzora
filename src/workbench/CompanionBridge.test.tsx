import { render } from "@testing-library/react"
import { beforeEach, expect, it, vi } from "vitest"
import { useCompanionStore } from "@/state/companionStore"
import { companionFixtures as fx } from "@/test/companionFixtures"

const events = vi.hoisted(() => ({ listen: vi.fn(), unlisten: vi.fn() }))
vi.mock("@tauri-apps/api/event", () => ({ listen: events.listen }))
const audit = vi.hoisted(() => ({ report: vi.fn() }))
vi.mock("./companionAudit", () => ({ reportCompanionEvent: audit.report }))
import { CompanionBridge } from "./CompanionBridge"

beforeEach(() => {
  events.listen.mockReset().mockResolvedValue(events.unlisten)
  events.unlisten.mockReset()
  useCompanionStore.setState({ overview: null, devices: [], claims: [], settled: {} })
})

it("feeds companion events into the store and stops listening on unmount", async () => {
  const view = render(<CompanionBridge />)
  expect(events.listen).toHaveBeenCalledWith("companion://event", expect.any(Function))
  const deliver = events.listen.mock.calls[0][1] as (event: { payload: unknown }) => void
  deliver({ payload: { type: "device.registered", device: fx.device } })
  expect(useCompanionStore.getState().devices).toEqual([fx.device])
  expect(audit.report).toHaveBeenCalledWith({ type: "device.registered", device: fx.device })
  view.unmount()
  await vi.waitFor(() => expect(events.unlisten).toHaveBeenCalled())
})
