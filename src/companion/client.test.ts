import { describe, expect, it } from "vitest"
import { endSession } from "./client"
import { useCompanion } from "./store"

describe("endSession", () => {
  it("forgets everything read from the repositories of the signed-out login", () => {
    useCompanion.setState({
      git: { "work\0w1": { commits: {} } },
      spaceView: { "work\0w1": { tab: "git", view: "history" } },
      snapshots: { work: { session: "work", workspaces: [], tabs: [], panes: [], agents: [] } },
    })
    endSession("logged-out")
    const state = useCompanion.getState()
    expect(state.git).toEqual({})
    expect(state.spaceView).toEqual({})
    expect(state.snapshots).toEqual({})
    expect(state.screen).toEqual({ name: "login", reason: "logged-out" })
  })
})
