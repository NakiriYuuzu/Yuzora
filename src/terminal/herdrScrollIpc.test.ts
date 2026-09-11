import { expect, it, vi } from "vitest"
import { invokeHerdr } from "@/lib/herdrProvider"
import { readPaneScroll, setPaneScroll } from "./herdrScrollIpc"
vi.mock("@/lib/herdrProvider", () => ({ invokeHerdr: vi.fn().mockResolvedValue(null) }))
it("keeps runtime/pane identity and absolute bottom offset at the typed boundary", async () => {
  const scope = '["wsl:host","work"]'
  await readPaneScroll(scope, "w1:p1")
  expect(invokeHerdr).toHaveBeenLastCalledWith("herdr_pane_scroll_state", { sessionName: scope, paneId: "w1:p1" })
  await setPaneScroll(scope, "w1:p1", 0)
  expect(invokeHerdr).toHaveBeenLastCalledWith("herdr_pane_scroll_to", { sessionName: scope, paneId: "w1:p1", offsetFromBottom: 0 })
  await expect(setPaneScroll(scope, "w1:p1", Number.NaN)).rejects.toThrow("invalid")
  await expect(setPaneScroll(scope, "w1:p1", -1)).rejects.toThrow("invalid")
})
