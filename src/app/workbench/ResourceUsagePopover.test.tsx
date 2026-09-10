import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { afterEach, beforeEach, expect, it } from "vitest"
import { ResourceUsagePopover } from "./ResourceUsagePopover"
import { usePerfStore } from "@/state/perfStore"
import type { PerfSnapshot } from "@/lib/types"

const snapshot: PerfSnapshot = {
  cpuPercent: 200, memoryBytes: 100_000_000,
  appCpuPercent: 150, appMemoryBytes: 10_000_000, descendantCount: 5,
  webviewCpuPercent: 40, webviewMemoryBytes: 30_000_000, webviewCount: 2,
  managedToolsCpuPercent: 10, managedToolsMemoryBytes: 60_000_000, managedToolsCount: 3,
}
beforeEach(() => usePerfStore.getState().reset())
afterEach(cleanup)
function open() {
  render(<ResourceUsagePopover trigger={<button>Usage</button>} />)
  fireEvent.click(screen.getByRole("button", { name: "Usage" }))
  return screen.getByRole("dialog", { name: "Resource usage" })
}
it("sorts disjoint categories by the selected metric using the real total", () => {
  usePerfStore.getState().setSnapshot(snapshot)
  const dialog = open()
  const rows = () => within(dialog).getAllByRole("listitem")
  expect(rows().map((row) => row.dataset.resourceId)).toEqual(["tools", "webview", "app"])
  expect(rows()[0]).toHaveTextContent("60.0% of total")
  fireEvent.click(within(dialog).getByRole("radio", { name: "CPU" }))
  expect(rows().map((row) => row.dataset.resourceId)).toEqual(["app", "webview", "tools"])
  expect(rows()[0]).toHaveTextContent("150.0 %")
  expect(rows()[0]).toHaveTextContent("75.0% of total")
  expect(within(dialog).getByText("200.0")).toBeInTheDocument()
})
it("keeps no sample distinct from measured zero and exposes sampling health", () => {
  usePerfStore.setState({ outcomes: ["failed", "empty", "skipped_no_focus"], lastError: "sample failed" })
  const dialog = open()
  expect(within(dialog).getByText(/No sample is available/)).toBeInTheDocument()
  expect(within(dialog).queryAllByRole("listitem")).toHaveLength(0)
  expect(within(dialog).getByText(/1 failed · 1 empty · 1 skipped/)).toBeInTheDocument()
  expect(within(dialog).getByText("sample failed")).toBeInTheDocument()
})
it("shows measured zero with unavailable shares instead of NaN", () => {
  usePerfStore.getState().setSnapshot(Object.fromEntries(Object.keys(snapshot).map((key) => [key, 0])) as unknown as PerfSnapshot)
  const dialog = open()
  expect(within(dialog).getAllByRole("listitem")).toHaveLength(3)
  expect(within(dialog).getAllByText("Share unavailable")).toHaveLength(3)
  expect(dialog).not.toHaveTextContent("NaN")
  expect(within(dialog).queryByText(/No sample is available/)).not.toBeInTheDocument()
})
it("associates its description and returns focus to the trigger on Escape", async () => {
  const dialog = open()
  expect(document.getElementById(dialog.getAttribute("aria-describedby")!)).toHaveTextContent(/owned descendants/)
  fireEvent.keyDown(dialog, { key: "Escape" })
  await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
  expect(screen.getByRole("button", { name: "Usage" })).toHaveFocus()
})
