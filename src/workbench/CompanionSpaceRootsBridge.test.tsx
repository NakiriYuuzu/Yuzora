import { act, render } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { useCompanionStore } from "@/state/companionStore"
import { useHerdrStore } from "@/state/herdrStore"
import { companionFixtures as fx, companionInvoke, type CompanionReplies } from "@/test/companionFixtures"

const ipc = vi.hoisted(() => ({ invoke: vi.fn() }))
vi.mock("@/lib/ipc", async importOriginal => ({ ...(await importOriginal<typeof import("@/lib/ipc")>()), invoke: ipc.invoke }))
import { CompanionSpaceRootsBridge } from "./CompanionSpaceRootsBridge"

let replies: CompanionReplies
const roots = () => ipc.invoke.mock.calls.filter(([name]) => name === "companion_space_roots_set").map(([, args]) => (args as { roots: unknown }).roots)
const spaces = (...items: Array<{ id: string; path?: string | null }>) => ({ snapshot: { spaces: items } }) as never
const hold = (runtimesBySession: Record<string, never>) => act(() => useHerdrStore.setState({ runtimesBySession }))
const wait = (ms: number) => act(() => vi.advanceTimersByTimeAsync(ms))
const experimental = (value: boolean) =>
  act(() => useCompanionStore.setState({ overview: { ...fx.overviewRunning, settings: { ...fx.overviewRunning.settings, experimental: value } } }))

beforeEach(() => {
  vi.useFakeTimers()
  replies = { companion_status: fx.overviewRunning, companion_devices: [], companion_space_roots_set: undefined }
  ipc.invoke.mockReset().mockImplementation(companionInvoke(() => replies))
  useCompanionStore.setState({ overview: fx.overviewRunning, devices: [] })
  useHerdrStore.setState({ runtimesBySession: {} })
})
afterEach(() => vi.useRealTimers())

it("sends the folders of local Spaces only, as absolute plain paths", async () => {
  hold({
    default: spaces(
      { id: "w1", path: "/Users/me/app" },
      { id: "w2", path: null },
      { id: "w3", path: "relative/dir" },
      { id: "w4", path: "yuzora-fs://ssh-1@%2Fsrv/%2Fsrv/app" },
      { id: "w5", path: "C:\\work\\app" },
    ),
    // A Session of a remote host: its paths mean nothing on this computer.
    [JSON.stringify(["ssh-1", "remote"])]: spaces({ id: "w1", path: "/srv/app" }),
    // The local host's scoped identity.
    [JSON.stringify(["local", "work"])]: spaces({ id: "w7", path: "/Users/me/work" }),
  })
  render(<CompanionSpaceRootsBridge />)
  await wait(500)
  expect(roots()).toEqual([[
    { session: "default", workspaceId: "w1", root: "/Users/me/app" },
    { session: "default", workspaceId: "w5", root: "C:\\work\\app" },
    { session: "work", workspaceId: "w7", root: "/Users/me/work" },
  ]])
})

it("waits for the Spaces to settle, and sends once", async () => {
  render(<CompanionSpaceRootsBridge />)
  hold({ default: spaces({ id: "w1", path: "/a" }) })
  await wait(499)
  expect(roots()).toHaveLength(0)
  hold({ default: spaces({ id: "w1", path: "/a" }, { id: "w2", path: "/b" }) })
  await wait(499)
  expect(roots()).toHaveLength(0)
  await wait(1)
  expect(roots()).toEqual([[
    { session: "default", workspaceId: "w1", root: "/a" },
    { session: "default", workspaceId: "w2", root: "/b" },
  ]])
})

it("sends only when the folders changed", async () => {
  hold({ default: spaces({ id: "w1", path: "/a" }) })
  render(<CompanionSpaceRootsBridge />)
  await wait(500)
  expect(roots()).toHaveLength(1)
  // A new snapshot with the same folders, and a Space without one, change nothing.
  hold({ default: spaces({ id: "w1", path: "/a" }, { id: "w2", path: null }) })
  await wait(1000)
  expect(roots()).toHaveLength(1)
  hold({ default: spaces({ id: "w1", path: "/moved" }) })
  await wait(500)
  expect(roots()).toHaveLength(2)
  expect(roots()[1]).toEqual([{ session: "default", workspaceId: "w1", root: "/moved" }])
})

it("has nothing to tell a Companion that starts out knowing no folders", async () => {
  render(<CompanionSpaceRootsBridge />)
  await wait(1000)
  expect(roots()).toHaveLength(0)
})

it("sends nothing while the Experimental Companion is off, and catches up when it turns on", async () => {
  experimental(false)
  hold({ default: spaces({ id: "w1", path: "/a" }) })
  render(<CompanionSpaceRootsBridge />)
  await wait(1000)
  expect(roots()).toHaveLength(0)
  experimental(true)
  await wait(500)
  expect(roots()).toEqual([[{ session: "default", workspaceId: "w1", root: "/a" }]])
})

it("reads the Companion state itself when nothing has yet", async () => {
  useCompanionStore.setState({ overview: null })
  hold({ default: spaces({ id: "w1", path: "/a" }) })
  render(<CompanionSpaceRootsBridge />)
  await wait(500)
  expect(roots()).toHaveLength(1)
})

it("tries again with the next change after a failed send", async () => {
  const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined)
  replies.companion_space_roots_set = { reject: "companion-timeout" }
  hold({ default: spaces({ id: "w1", path: "/a" }) })
  render(<CompanionSpaceRootsBridge />)
  await wait(500)
  expect(roots()).toHaveLength(1)
  expect(warn).toHaveBeenCalledWith("Companion Space roots update failed", "companion-timeout")
  replies.companion_space_roots_set = undefined
  hold({ default: spaces({ id: "w1", path: "/a" }, { id: "w2", path: "/b" }) })
  await wait(500)
  expect(roots()).toHaveLength(2)
  warn.mockRestore()
})
