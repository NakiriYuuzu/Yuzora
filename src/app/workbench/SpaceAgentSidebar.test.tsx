import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, expect, it } from "vitest"
import { herdrInitialState, useHerdrStore } from "@/state/herdrStore"
import { SpaceAgentSidebar } from "./SpaceAgentSidebar"

const initial = useHerdrStore.getState()
beforeEach(() => {
  useHerdrStore.setState({ ...herdrInitialState, sessions: [
    { name: "running", running: true, default: true, sessionDir: "/run", socketPath: "/run.sock" },
    { name: "stopped", running: false, default: false, sessionDir: "/stop", socketPath: "/stop.sock" }
  ] })
})
afterEach(() => { cleanup(); useHerdrStore.setState(initial, true) })

it("shows only running sessions in the scope menu, without Agent creation", async () => {
  render(<SpaceAgentSidebar />)
  fireEvent.keyDown(screen.getByRole("button", { name: "Herdr Session: All" }), { key: "Enter" })
  expect(await screen.findByRole("menuitem", { name: "Refresh sessions" })).toBeInTheDocument()
  expect(screen.queryByRole("menuitem", { name: "New Herdr Agent" })).not.toBeInTheDocument()
  expect(screen.getByRole("menuitemradio", { name: "Local · running" })).toBeInTheDocument()
  expect(screen.queryByRole("menuitemradio", { name: /stopped/ })).not.toBeInTheDocument()
})

it("offers Space and Session creation from the add menu", async () => {
  render(<SpaceAgentSidebar />)
  fireEvent.keyDown(screen.getByRole("button", { name: "Add Space or Herdr Session" }), { key: "Enter" })
  expect(await screen.findByRole("menuitem", { name: "Open folder and create Space" })).toBeInTheDocument()
  expect(screen.getByRole("menuitem", { name: "Add Herdr Session" })).toBeInTheDocument()
  expect(screen.queryByRole("menuitem", { name: "New Herdr Agent" })).not.toBeInTheDocument()
})
