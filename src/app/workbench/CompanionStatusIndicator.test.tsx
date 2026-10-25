import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import type { CompanionDevice, CompanionEvent, CompanionOverview } from "@/lib/companionIpc"
import type { HerdrNamedSession } from "@/lib/herdrTypes"
import { useCompanionStore } from "@/state/companionStore"
import { useHerdrStore } from "@/state/herdrStore"
import { useUiStore } from "@/state/uiStore"
import { companionFixtures as fx, companionInvoke, type CompanionReplies } from "@/test/companionFixtures"

const ipc = vi.hoisted(() => ({ invoke: vi.fn() }))
vi.mock("@/lib/ipc", async importOriginal => ({ ...(await importOriginal<typeof import("@/lib/ipc")>()), invoke: ipc.invoke }))
import { CompanionStatusIndicator } from "./CompanionStatusIndicator"

let replies: CompanionReplies
const calls = (command: string) => ipc.invoke.mock.calls.filter(([name]) => name === command).map(([, args]) => args)
const device = (deviceId: string, label: string, grants: CompanionDevice["grants"] = { control: null, full: false }): CompanionDevice =>
  ({ ...fx.device, deviceId, label, grants })
const running = (patch: Partial<CompanionOverview> = {}): CompanionOverview => ({ ...fx.overviewRunning, ...patch })
const grant = { until: "2026-10-18T19:00:00Z" }
const expired = { until: "2026-10-18T18:00:00Z" }
const event = (value: CompanionEvent) => act(() => useCompanionStore.getState().applyEvent(value))
const chip = () => screen.getByTestId("status-companion-chip")
const openPopover = async () => {
  fireEvent.click(chip())
  return screen.findByRole("dialog", { name: "Companion" })
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date"], now: Date.parse("2026-10-18T18:30:00Z") })
  replies = { companion_status: fx.overviewOff, companion_devices: [] }
  ipc.invoke.mockReset().mockImplementation(companionInvoke(() => replies))
  useCompanionStore.setState({ overview: running(), devices: [], peers: {}, streams: {}, streamSeq: {}, claims: [], settled: {} })
  useHerdrStore.setState({ sessions: [], runtimesBySession: {} })
})
afterEach(() => vi.useRealTimers())

it.each([
  ["no overview yet", null],
  ["off", fx.overviewOff],
  ["starting", running({ status: fx.statusStarting })],
  ["stopping", running({ status: fx.statusStopping })],
  ["failed", running({ status: fx.statusFailed })],
])("is hidden while the Companion is %s", (_name, overview) => {
  useCompanionStore.setState({ overview })
  render(<CompanionStatusIndicator />)
  expect(screen.queryByTestId("status-companion-chip")).not.toBeInTheDocument()
})

it("shows the time left, or until quit, and appears when the Companion starts running", () => {
  useCompanionStore.setState({ overview: fx.overviewOff })
  render(<CompanionStatusIndicator />)
  expect(screen.queryByTestId("status-companion-chip")).not.toBeInTheDocument()
  event({ type: "status", status: { state: "running", port: 47110 } })
  expect(chip()).toHaveAttribute("data-controlling", "false")
  act(() => useCompanionStore.setState({ overview: running() }))
  expect(chip()).toHaveTextContent("1h 30m")
  act(() => useCompanionStore.setState({ overview: running({ settings: { ...fx.overviewRunning.settings, duration: "untilExit" } }) }))
  expect(chip()).toHaveTextContent("Until quit")
})

it("counts the connected phones, each device once", () => {
  render(<CompanionStatusIndicator />)
  expect(chip()).not.toHaveTextContent(/\d+ connected/)
  expect(within(chip()).queryByTitle(/connected/)).not.toBeInTheDocument()
  event({ type: "session.opened", peerId: 1, deviceId: "dev-a" })
  event({ type: "session.opened", peerId: 2, deviceId: "dev-a" })
  event({ type: "session.opened", peerId: 3, deviceId: "dev-b" })
  expect(within(chip()).getByTitle("2 connected")).toHaveTextContent("2")
  event({ type: "session.closed", peerId: 3, deviceId: "dev-b", reason: "closed" })
  expect(within(chip()).getByTitle("1 connected")).toHaveTextContent("1")
})

it("turns red and says controlling while a control or full stream is open", () => {
  render(<CompanionStatusIndicator />)
  expect(chip()).not.toHaveTextContent("Controlling")
  event({ type: "control.state", deviceId: "dev-a", peerId: 1, stream: "6", seq: 1, session: "default", target: "w1:p1", takeover: false, open: true })
  expect(chip()).toHaveTextContent("Controlling")
  expect(chip()).toHaveAttribute("data-controlling", "true")
  // Its phone's session event was lost: the open stream still counts the phone.
  expect(within(chip()).getByTitle("1 connected")).toBeInTheDocument()
  event({ type: "control.state", deviceId: "dev-a", peerId: 1, stream: "6", seq: 2, session: "default", target: "w1:p1", takeover: false, open: false })
  expect(chip()).not.toHaveTextContent("Controlling")
  event({ type: "full.state", deviceId: "dev-a", peerId: 1, stream: "9", seq: 1, session: "default", open: true })
  expect(chip()).toHaveTextContent("Controlling")
})

it("lists what each connected device is doing, naming the pane it drives", async () => {
  useHerdrStore.setState({
    sessions: [{ name: "default", running: true, default: true } as HerdrNamedSession],
    runtimesBySession: {
      default: { snapshot: { agents: [{ id: "a1", name: "pi", status: "idle", workspaceId: "w1", paneId: "w1:p1" }] } },
    } as never,
  })
  useCompanionStore.setState({ devices: [device("dev-a", "iPhone"), device("dev-b", "Pixel"), device("dev-c", "iPad")] })
  render(<CompanionStatusIndicator />)
  expect(within(await openPopover()).getByText("No phone is connected.")).toBeInTheDocument()
  event({ type: "session.opened", peerId: 1, deviceId: "dev-a" })
  event({ type: "session.opened", peerId: 2, deviceId: "dev-b" })
  event({ type: "session.opened", peerId: 3, deviceId: "dev-c" })
  event({ type: "control.state", deviceId: "dev-b", peerId: 2, stream: "6", seq: 1, session: "default", target: "w1:p1", takeover: false, open: true })
  event({ type: "full.state", deviceId: "dev-c", peerId: 3, stream: "9", seq: 1, session: "default", open: true })
  const rows = screen.getAllByTestId("status-companion-device").map(row => row.textContent)
  expect(rows).toEqual(["iPhoneViewing", "PixelControlling pi", "iPadFull session on default"])
})

it("names a connected device that is no longer registered", async () => {
  render(<CompanionStatusIndicator />)
  event({ type: "session.opened", peerId: 1, deviceId: "gone" })
  await openPopover()
  expect(screen.getByTestId("status-companion-device")).toHaveTextContent("A phoneViewing")
})

it("revokes every live control grant and full session, one device at a time, and says phones are signed out", async () => {
  useCompanionStore.setState({
    devices: [
      device("dev-a", "iPhone", { control: grant, full: false }),
      device("dev-b", "Pixel", { control: null, full: true }),
      device("dev-c", "iPad", { control: grant, full: true }),
      device("dev-d", "Galaxy", { control: expired, full: false }),
      device("dev-e", "Nokia"),
    ],
  })
  // A second command while one is in flight would overlap.
  let inFlight = 0
  let overlapped = false
  replies.companion_device_update = async (args: Record<string, unknown>) => {
    overlapped ||= ++inFlight > 1
    await new Promise(resolve => setTimeout(resolve, 0))
    inFlight--
    const found = useCompanionStore.getState().devices.find(item => item.deviceId === args.deviceId)!
    return { ...found, grants: args.grants }
  }
  render(<CompanionStatusIndicator />)
  const popover = await openPopover()
  // The button and its description both say what happens to the phones.
  const button = within(popover).getByRole("button", { name: "Revoke control, sign phones out" })
  expect(button).toHaveAccessibleDescription(/The phones concerned are signed out and have to log in again/)
  fireEvent.click(button)
  await waitFor(() => expect(calls("companion_device_update")).toHaveLength(3))
  // One tightening update each: control and full both go, so the gateway takes it in one step.
  expect(calls("companion_device_update")).toEqual([
    { deviceId: "dev-a", grants: { control: null, full: false } },
    { deviceId: "dev-b", grants: { control: null, full: false } },
    { deviceId: "dev-c", grants: { control: null, full: false } },
  ])
  await waitFor(() => expect(button).toBeDisabled())
  expect(useCompanionStore.getState().devices.find(item => item.deviceId === "dev-d")?.grants.control).toEqual(expired)
  expect(overlapped).toBe(false)
})

it("stops at the first device it cannot revoke and says why", async () => {
  useCompanionStore.setState({
    devices: [device("dev-a", "iPhone", { control: grant, full: false }), device("dev-c", "iPad", { control: grant, full: false })],
  })
  replies.companion_device_update = { reject: "companion-timeout" }
  render(<CompanionStatusIndicator />)
  fireEvent.click(within(await openPopover()).getByRole("button", { name: "Revoke control, sign phones out" }))
  expect(await screen.findByText("The Companion did not reply in time and was stopped.")).toBeInTheDocument()
  expect(calls("companion_device_update")).toHaveLength(1)
})

it("can end a full session, which holds no control grant", async () => {
  useCompanionStore.setState({ devices: [device("dev-b", "Pixel", { control: null, full: true })] })
  event({ type: "full.state", deviceId: "dev-b", peerId: 2, stream: "9", seq: 1, session: "default", open: true })
  replies.companion_device_update = (args: Record<string, unknown>) => ({ ...device("dev-b", "Pixel"), grants: args.grants })
  render(<CompanionStatusIndicator />)
  expect(chip()).toHaveTextContent("Controlling")
  const button = within(await openPopover()).getByRole("button", { name: "Revoke control, sign phones out" })
  expect(button).toBeEnabled()
  fireEvent.click(button)
  await waitFor(() => expect(calls("companion_device_update")).toEqual([{ deviceId: "dev-b", grants: { control: null, full: false } }]))
})

it.each([
  ["no device holds control", [device("dev-a", "iPhone")]],
  ["the only control grant has run out", [device("dev-a", "iPhone", { control: expired, full: false })]],
])("has nothing to revoke when %s", async (_name, devices) => {
  useCompanionStore.setState({ devices })
  render(<CompanionStatusIndicator />)
  const popover = await openPopover()
  expect(within(popover).getByRole("button", { name: "Revoke control, sign phones out" })).toBeDisabled()
  expect(within(popover).queryByText(/signed out and have to log in again/)).not.toBeInTheDocument()
})

it("opens the Companion settings and closes the popover", async () => {
  render(<CompanionStatusIndicator />)
  fireEvent.click(within(await openPopover()).getByRole("button", { name: "Companion settings" }))
  expect(useUiStore.getState()).toMatchObject({ settingsOpen: true, settingsSection: "companion" })
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "Companion" })).not.toBeInTheDocument())
})

it("disables the Companion and re-reads its state", async () => {
  replies.companion_disable = undefined
  replies.companion_status = fx.overviewOff
  render(<CompanionStatusIndicator />)
  fireEvent.click(within(await openPopover()).getByRole("button", { name: "Disable" }))
  await waitFor(() => expect(calls("companion_disable")).toHaveLength(1))
  // The stop makes the chip disappear with the refreshed overview.
  await waitFor(() => expect(screen.queryByTestId("status-companion-chip")).not.toBeInTheDocument())
})

it("is corrected by the gateway's list: a phone the events never announced appears, a ghost whose close was dropped goes", async () => {
  replies.companion_peers = { peers: [{ peerId: 5, deviceId: "dev-live" }], streams: [] }
  render(<CompanionStatusIndicator />)
  event({ type: "session.opened", peerId: 4, deviceId: "dev-ghost" })
  expect(within(chip()).getByTitle("1 connected")).toBeInTheDocument()
  await act(async () => { await useCompanionStore.getState().reconcilePeers() })
  expect(within(chip()).getByTitle("1 connected")).toBeInTheDocument()
  expect(useCompanionStore.getState().peers).toEqual({ 5: "dev-live" })
  const dialog = await openPopover()
  expect(within(dialog).getAllByTestId("status-companion-device")).toHaveLength(1)
  // A stream the events missed shows who drives which pane.
  replies.companion_peers = {
    peers: [{ peerId: 5, deviceId: "dev-live" }],
    streams: [{ stream: "9", deviceId: "dev-live", peerId: 5, session: "default", scope: "control", target: "w1:p1" }],
  }
  await act(async () => { await useCompanionStore.getState().reconcilePeers() })
  expect(chip()).toHaveAttribute("data-controlling", "true")
})
