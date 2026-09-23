import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"
import type { HerdrSnapshot } from "@/lib/herdrTypes"
import { PaneTools } from "./PaneTools"
import type { HerdrOperation } from "./useHerdrOperation"

afterEach(cleanup)

const snapshot = {
  herdrSessionId: "default", protocol: 1, version: "test", agents: [], raw: null, focusedPaneId: "p1",
  spaces: [{ id: "w1", label: "yuzora" }, { id: "w2", label: "docs" }],
  tabs: [{ id: "t2", label: "server", order: 1, workspaceId: "w2", paneCount: 2, status: "idle", active: true, focused: false }],
  terminals: [{ terminalId: "term-1", paneId: "p1", workspaceId: "w1", tabId: "t1", title: "shell" }],
} as unknown as HerdrSnapshot

function setup() {
  const run = vi.fn().mockResolvedValue({})
  const operation = { busy: false, run } as unknown as HerdrOperation
  render(<PaneTools snapshot={snapshot} paneId="p1" workspaceId="w1" operation={operation} can={() => true} />)
  return run
}

describe("PaneTools move flow", () => {
  it("moves the pane into an existing tab with the chosen split direction", () => {
    const run = setup()
    fireEvent.click(screen.getByRole("radio", { name: /Existing tab/ }))
    fireEvent.click(screen.getByRole("radio", { name: /server/ }))
    fireEvent.click(screen.getByRole("radio", { name: /Down/ }))
    fireEvent.click(screen.getByRole("button", { name: "Move pane" }))
    expect(run).toHaveBeenCalledWith({ method: "pane.move", params: { pane_id: "p1", destination: { type: "tab", tab_id: "t2", split: "down" }, focus: false } })
  })

  it("opens the pane as a new tab in the chosen Space", () => {
    const run = setup()
    fireEvent.click(screen.getByRole("radio", { name: "docs" }))
    fireEvent.click(screen.getByRole("button", { name: "Move pane" }))
    expect(run).toHaveBeenCalledWith({ method: "pane.move", params: { pane_id: "p1", destination: { type: "new_tab", workspace_id: "w2", label: undefined }, focus: false } })
  })

  it("keeps the move button disabled until an existing tab is picked", () => {
    setup()
    fireEvent.click(screen.getByRole("radio", { name: /Existing tab/ }))
    expect((screen.getByRole("button", { name: "Move pane" }) as HTMLButtonElement).disabled).toBe(true)
  })
})
