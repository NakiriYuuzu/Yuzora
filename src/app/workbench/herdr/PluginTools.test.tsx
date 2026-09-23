import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { herdrFeature, type HerdrPlugin } from "@/lib/herdrFeatures"
import { useHerdrNativeStore } from "@/state/herdrNativeStore"
import { PluginTools } from "./PluginTools"
import type { HerdrOperation } from "./useHerdrOperation"

vi.mock("@/lib/herdrFeatures", () => ({ herdrFeature: vi.fn() }))

beforeEach(() => {
  vi.clearAllMocks()
  useHerdrNativeStore.setState({ selection: null })
})

afterEach(() => {
  cleanup()
  useHerdrNativeStore.setState({ selection: null })
})

describe("plugin pane placement targets", () => {
  it.each([
    { placement: "popup", target: {} },
    { placement: "overlay", target: {} },
    { placement: "split", target: { target_pane_id: "pane-1" } },
    { placement: "zoomed", target: { target_pane_id: "pane-1" } },
    { placement: "tab", target: { workspace_id: "workspace-1" } }
  ] as const)("opens the native client before dispatching a $placement pane with its supported target", async ({ placement, target }) => {
    const plugin: HerdrPlugin = {
      plugin_id: "fixture.demo", name: "Fixture plugin", version: "1.0.0", enabled: true,
      panes: [{ id: "inspect", title: "Inspect plugin pane", placement }]
    }
    vi.mocked(herdrFeature).mockResolvedValue({ plugins: [plugin] })
    const operation: HerdrOperation = {
      busy: false, error: null, refreshError: null, result: null, run: vi.fn().mockResolvedValue(null)
    }
    render(<PluginTools sessionName="test-session" workspaceId="workspace-1" paneId="pane-1" operation={operation} can={() => true} />)

    fireEvent.click(await screen.findByRole("button", { name: "Inspect plugin pane" }))

    // The request is queued for HerdrNativeDialog, which first connects its
    // official client and focuses pane-1. Dispatching here has no active client.
    expect(useHerdrNativeStore.getState().selection).toStrictEqual({
      sessionName: "test-session", paneId: "pane-1",
      request: {
        method: "plugin.pane.open",
        params: { plugin_id: "fixture.demo", entrypoint: "inspect", placement, ...target, focus: true }
      }
    })
    expect(herdrFeature).toHaveBeenCalledExactlyOnceWith("test-session", { method: "plugin.list", params: {} })
    expect(operation.run).not.toHaveBeenCalled()
  })
})
