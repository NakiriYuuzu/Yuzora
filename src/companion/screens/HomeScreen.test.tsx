import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"
import { I18nextProvider } from "react-i18next"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { i18n } from "../i18n"
import type { Snapshot } from "../lib/protocol"
import { navigate, useCompanion } from "../store"

vi.mock("../client", () => ({ call: vi.fn(), logout: vi.fn(), refresh: vi.fn(), retryNow: vi.fn() }))
import { HomeScreen } from "./HomeScreen"

const snapshot: Snapshot = {
  session: "work",
  workspaces: [
    { id: "w1", label: "Yuzora", status: "working", git: true },
    { id: "w2", label: "Docs", status: "idle", git: true },
    { id: "w3", label: "Scratch", status: "idle", git: false },
    { id: null, label: "Loose", status: "unknown", git: false },
  ],
  tabs: [{ id: "w1:t", workspaceId: "w1", label: "main" }],
  panes: [
    { id: "w1:p", tabId: "w1:t", workspaceId: "w1", status: "working", label: null, layout: null },
    { id: "x:p", tabId: null, workspaceId: null, status: "idle", label: null, layout: null },
  ],
  agents: [
    { paneId: "w1:p", name: "pi", agent: "pi", status: "working" },
    { paneId: "x:p", name: "stray", agent: null, status: "idle" },
  ],
}

beforeEach(async () => {
  await i18n.changeLanguage("en")
  navigate({ name: "home" })
  useCompanion.setState({ socket: "ready", sessions: [{ name: "work", default: true }], snapshots: { work: snapshot }, sessionErrors: {} })
  render(<I18nextProvider i18n={i18n}><HomeScreen /></I18nextProvider>)
})
afterEach(cleanup)

it("turns a Space header into a way into the Space", () => {
  fireEvent.click(screen.getByRole("button", { name: "Yuzora" }))
  expect(useCompanion.getState().screen).toEqual({ name: "space", session: "work", workspaceId: "w1" })
})

it("opens an agent's pane as before", () => {
  fireEvent.click(screen.getByRole("button", { name: /^pi/ }))
  expect(useCompanion.getState().screen).toEqual({ name: "pane", session: "work", pane: "w1:p" })
})

it("keeps a Space that has no id out of the buttons", () => {
  expect(screen.getByText("Loose")).toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Loose" })).toBeNull()
})

it("folds Spaces without agents into a group that starts collapsed", () => {
  const group = screen.getByRole("button", { name: /Other Spaces/ })
  expect(group).toHaveAttribute("aria-expanded", "false")
  expect(group.textContent).toContain("2")
  expect(screen.queryByRole("button", { name: "Docs" })).toBeNull()
  fireEvent.click(group)
  expect(group).toHaveAttribute("aria-expanded", "true")
  expect(screen.getByRole("button", { name: "Scratch" })).toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Docs" }))
  expect(useCompanion.getState().screen).toEqual({ name: "space", session: "work", workspaceId: "w2" })
})

it("shows the notifications bell only when the desktop can send them", () => {
  expect(screen.queryByRole("button", { name: "Notifications" })).toBeNull()
  act(() => useCompanion.setState({ hello: { protocolVersion: 1, serverTime: "2026-10-20T00:00:00Z", scopes: ["view"], push: { vapidPublicKey: "k" } } }))
  expect(screen.getByRole("button", { name: "Notifications" })).toBeInTheDocument()
  act(() => useCompanion.setState({ hello: { protocolVersion: 1, serverTime: "2026-10-20T00:00:00Z", scopes: ["view"], push: null } }))
  expect(screen.queryByRole("button", { name: "Notifications" })).toBeNull()
})
