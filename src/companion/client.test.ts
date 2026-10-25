import { beforeEach, describe, expect, it, vi } from "vitest"
import { api } from "./lib/api"
import { abortLogout, endSession, logout } from "./client"
import { storage, useCompanion } from "./store"

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

describe("logout", () => {
  beforeEach(() => vi.restoreAllMocks())

  it("clears the login at once, without waiting for the gateway", async () => {
    vi.spyOn(storage, "binding").mockReturnValue("binding")
    const setBinding = vi.spyOn(storage, "setBinding").mockImplementation(() => undefined)
    const sent = vi.spyOn(api, "logout").mockReturnValue(new Promise(() => undefined))
    const outcome = await Promise.race([logout().then(() => "done"), new Promise((resolve) => setTimeout(() => resolve("hung"), 100))])
    expect(outcome).toBe("done")
    expect(sent).toHaveBeenCalledWith("binding", expect.any(AbortSignal))
    expect(setBinding).toHaveBeenCalledWith(null)
    expect(useCompanion.getState().screen).toEqual({ name: "login", reason: "logged-out" })
  })

  it("aborts the request when a new login starts, so its answer cannot clear the new cookie", async () => {
    vi.spyOn(storage, "binding").mockReturnValue("binding")
    vi.spyOn(storage, "setBinding").mockImplementation(() => undefined)
    const sent = vi.spyOn(api, "logout").mockReturnValue(new Promise(() => undefined))
    await logout()
    const signal = sent.mock.calls[0][1] as AbortSignal
    expect(signal.aborted).toBe(false)
    abortLogout()
    expect(signal.aborted).toBe(true)
  })

  it("gives up on a stuck request after a short deadline", async () => {
    vi.useFakeTimers()
    try {
      vi.spyOn(storage, "binding").mockReturnValue("binding")
      vi.spyOn(storage, "setBinding").mockImplementation(() => undefined)
      const sent = vi.spyOn(api, "logout").mockReturnValue(new Promise(() => undefined))
      await logout()
      const signal = sent.mock.calls[0][1] as AbortSignal
      vi.advanceTimersByTime(5000)
      expect(signal.aborted).toBe(true)
    } finally {
      vi.useRealTimers()
    }
  })
})
