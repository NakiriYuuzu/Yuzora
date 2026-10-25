import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { goBack, navigate, open, restoreScreen, savedScroll, useCompanion, type Screen } from "./store"

const space: Screen = { name: "space", session: "work", workspaceId: "w1" }
const diff: Screen = { name: "diff", session: "work", workspaceId: "w1", source: { kind: "worktree", path: "src/a.ts", untracked: false } }
const screenName = () => useCompanion.getState().screen.name
const scrollTo = (y: number) => Object.defineProperty(window, "scrollY", { value: y, configurable: true })

beforeEach(() => {
  window.addEventListener("popstate", restoreScreen)
  navigate({ name: "home" })
})
afterEach(() => {
  window.removeEventListener("popstate", restoreScreen)
  scrollTo(0)
})

describe("screen history", () => {
  it("gives each opened screen a history entry that the back gesture undoes", async () => {
    open(space)
    open(diff)
    expect(useCompanion.getState().screen).toEqual(diff)
    history.back()
    await vi.waitFor(() => expect(useCompanion.getState().screen).toEqual(space))
    history.back()
    await vi.waitFor(() => expect(screenName()).toBe("home"))
  })

  it("steps forward again to the screens it left", async () => {
    open(space)
    open(diff)
    history.go(-2)
    await vi.waitFor(() => expect(screenName()).toBe("home"))
    history.forward()
    await vi.waitFor(() => expect(useCompanion.getState().screen).toEqual(space))
  })

  it("makes the in-app back button take the same step", async () => {
    open(space)
    goBack()
    await vi.waitFor(() => expect(screenName()).toBe("home"))
  })

  it("falls back to home when there is no entry to go back to", () => {
    navigate({ name: "login" })
    goBack()
    expect(screenName()).toBe("home")
  })

  it("keeps screens, links and secrets out of history entries", () => {
    navigate({ name: "pair", link: { pairingId: "pairing-id", secret: "very-secret-value" } })
    expect(Object.keys(history.state as object).sort()).toEqual(["depth", "load"])
    open(space)
    open(diff)
    expect(Object.keys(history.state as object).sort()).toEqual(["depth", "load"])
    expect((history.state as { depth: number }).depth).toBe(2)
    expect(JSON.stringify(history.state)).not.toMatch(/secret|pairing|src\/a\.ts/)
    expect(location.href).not.toMatch(/secret|pairing/)
  })

  it("ignores entries that belong to another page load or to screens that are gone", () => {
    open(space)
    const stale = history.state
    navigate({ name: "login" })
    restoreScreen(new PopStateEvent("popstate", { state: stale }))
    expect(screenName()).toBe("login")
    restoreScreen(new PopStateEvent("popstate", { state: null }))
    expect(screenName()).toBe("login")
  })

  it("does not follow an entry left behind by an earlier page load", () => {
    open(space)
    restoreScreen(new PopStateEvent("popstate", { state: { depth: 0, load: -1 } }))
    expect(useCompanion.getState().screen).toEqual(space)
  })

  it("discards the screens ahead when a new one is opened after going back", async () => {
    open(space)
    open(diff)
    history.back()
    await vi.waitFor(() => expect(useCompanion.getState().screen).toEqual(space))
    const commit: Screen = { name: "commit", session: "work", workspaceId: "w1", hash: "abc" }
    open(commit)
    const load = (history.state as { load: number }).load
    restoreScreen(new PopStateEvent("popstate", { state: { depth: 1, load } }))
    expect(useCompanion.getState().screen).toEqual(space)
    restoreScreen(new PopStateEvent("popstate", { state: { depth: 2, load } }))
    expect(useCompanion.getState().screen).toEqual(commit)
  })

  it("returns a screen to where it was scrolled and starts new ones at the top", () => {
    scrollTo(480)
    open(space)
    expect(savedScroll()).toBe(0)
    scrollTo(120)
    open(diff)
    restoreScreen(new PopStateEvent("popstate", { state: { depth: 1, load: (history.state as { load: number }).load } }))
    expect(savedScroll()).toBe(120)
    restoreScreen(new PopStateEvent("popstate", { state: { depth: 0, load: (history.state as { load: number }).load } }))
    expect(savedScroll()).toBe(480)
  })
})
