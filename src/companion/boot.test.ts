import { afterEach, beforeEach, expect, it, vi } from "vitest"

vi.mock("./client", () => ({ connect: vi.fn() }))
import { boot } from "./boot"
import { connect } from "./client"
import { navigate, useCompanion } from "./store"

const at = (hash: string) => history.replaceState(null, "", `/${hash}`)
const link = `#pane=${encodeURIComponent(JSON.stringify(["work", "w1:p"]))}`

beforeEach(() => {
  navigate({ name: "pair", link: null })
  useCompanion.setState({ pendingPane: null })
  vi.mocked(connect).mockClear()
})
afterEach(() => {
  sessionStorage.clear()
  localStorage.clear()
  at("")
})

it("keeps a notification's pane for the first snapshots, and strips the link", () => {
  sessionStorage.setItem("yzc.binding", "binding")
  at(link)
  boot()
  expect(location.hash).toBe("")
  expect(useCompanion.getState().pendingPane).toEqual({ session: "work", pane: "w1:p" })
  expect(useCompanion.getState().screen).toEqual({ name: "home" })
  expect(connect).toHaveBeenCalled()
})

it("holds it across the sign-in a cold start needs", () => {
  localStorage.setItem("yzc.paired", "1")
  at(link)
  boot()
  expect(useCompanion.getState().screen).toEqual({ name: "login" })
  expect(useCompanion.getState().pendingPane).toEqual({ session: "work", pane: "w1:p" })
})

it("ignores a malformed link and still starts normally", () => {
  sessionStorage.setItem("yzc.binding", "binding")
  at("#pane=%5B%22work%22")
  boot()
  expect(location.hash).toBe("")
  expect(useCompanion.getState().pendingPane).toBeNull()
  expect(useCompanion.getState().screen).toEqual({ name: "home" })
})
