import { fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { beforeEach, expect, it, vi } from "vitest"
import type { CompanionOverview } from "@/lib/companionIpc"
import { useCompanionStore } from "@/state/companionStore"
import { useHerdrStore } from "@/state/herdrStore"
import { companionFixtures as fx, companionInvoke, type CompanionReplies } from "@/test/companionFixtures"

const ipc = vi.hoisted(() => ({ invoke: vi.fn() }))
vi.mock("@/lib/ipc", async importOriginal => ({ ...(await importOriginal<typeof import("@/lib/ipc")>()), invoke: ipc.invoke }))
import { CompanionSettingsSection } from "./CompanionSettingsSection"

let replies: CompanionReplies
const on = (patch: Partial<CompanionOverview["settings"]> = {}, overview: Partial<CompanionOverview> = {}): CompanionOverview =>
  ({ ...fx.overviewOff, ...overview, settings: { ...fx.overviewOff.settings, experimental: true, ...patch } })
const calls = (command: string) => ipc.invoke.mock.calls.filter(([name]) => name === command).map(([, args]) => args)

beforeEach(() => {
  replies = { companion_status: fx.overviewOff, companion_devices: [], companion_tailscale_probe: fx.probeReady }
  ipc.invoke.mockReset().mockImplementation(companionInvoke(() => replies))
  useCompanionStore.setState({ overview: null, devices: [], claims: [], settled: {} })
  useHerdrStore.setState({ sessions: [] })
})

it("shows only the Experimental switch while the flag is off, and turns it on", async () => {
  replies.companion_experimental_set = on()
  render(<CompanionSettingsSection />)
  const flag = await screen.findByRole("switch", { name: /Companion \(phone remote control\)/ })
  expect(flag).not.toBeChecked()
  expect(screen.queryByText("Tailscale")).not.toBeInTheDocument()
  expect(calls("companion_tailscale_probe")).toHaveLength(0)
  fireEvent.click(flag)
  await waitFor(() => expect(calls("companion_experimental_set")).toEqual([{ enabled: true }]))
  expect(await screen.findByText("Remote control of this computer")).toBeInTheDocument()
  expect(screen.getByText("Tailscale")).toBeInTheDocument()
})

it("locks every mutation when another Yuzora owns the Companion", async () => {
  replies.companion_status = on({}, { owner: false })
  render(<CompanionSettingsSection />)
  expect(await screen.findByText("Another Yuzora owns the Companion")).toBeInTheDocument()
  expect(screen.getByRole("switch", { name: /Companion/ })).toBeDisabled()
  await screen.findByText("desktop.tail1234.ts.net")
  expect(screen.getByRole("button", { name: "Enable" })).toBeDisabled()
})

it("shows the exact Serve command and enables only after confirming it", async () => {
  replies.companion_status = on()
  replies.companion_enable = fx.overviewRunning.status
  render(<CompanionSettingsSection />)
  expect(await screen.findByText("desktop.tail1234.ts.net")).toBeInTheDocument()
  expect(screen.getByText("user@example.com")).toBeInTheDocument()
  fireEvent.click(await screen.findByRole("button", { name: "Enable" }))
  const dialog = await screen.findByRole("alertdialog")
  expect(within(dialog).getByTestId("companion-serve-command")).toHaveTextContent(/^tailscale serve --bg --https=8443 http:\/\/127\.0\.0\.1:47110$/)
  expect(calls("companion_enable")).toHaveLength(0)
  fireEvent.click(within(dialog).getByRole("button", { name: "Run and enable" }))
  await waitFor(() => expect(calls("companion_enable")).toEqual([{ window: "8h", confirm: true }]))
})

it.each([
  ["probeNoCli", "Tailscale is not installed"],
  ["probeLoggedOut", "This computer is not signed in to a tailnet with MagicDNS. Sign in to Tailscale first."],
  ["probeTagged", "This computer is a tagged device, so Serve sends no identity. Sign it in with a personal account."],
  ["probePortTaken", "The Companion port is already used by another Serve handler. Remove that handler or change the port."],
] as const)("blocks enabling for %s with guidance", async (probe, guidance) => {
  replies.companion_status = on()
  replies.companion_tailscale_probe = fx[probe]
  render(<CompanionSettingsSection />)
  expect(await screen.findByText(guidance)).toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Enable" })).toBeDisabled()
})

it("shows the time left, or until exit, while running, and disables", async () => {
  vi.useFakeTimers({ toFake: ["Date"], now: Date.parse("2026-10-18T18:30:00Z") })
  try {
    replies.companion_status = fx.overviewRunning
    replies.companion_disable = undefined
    const view = render(<CompanionSettingsSection />)
    expect(await screen.findByText("1h 30m left")).toBeInTheDocument()
    expect(screen.getByTestId("companion-status")).toHaveTextContent("Running")
    fireEvent.click(screen.getByRole("button", { name: "Disable" }))
    await waitFor(() => expect(calls("companion_disable")).toHaveLength(1))
    view.unmount()
    useCompanionStore.setState({ overview: null })
    replies.companion_status = { ...fx.overviewRunning, settings: { ...fx.overviewRunning.settings, duration: "untilExit" } }
    render(<CompanionSettingsSection />)
    expect(await screen.findByText("Until Yuzora quits")).toBeInTheDocument()
  } finally {
    vi.useRealTimers()
  }
})

it("opens the address for an already paired phone while running", async () => {
  replies.companion_status = fx.overviewRunning
  replies.companion_phone_link = fx.phoneLink
  Object.assign(URL, { createObjectURL: () => "blob:tauri://localhost/link", revokeObjectURL: () => undefined })
  render(<CompanionSettingsSection />)
  fireEvent.click(await screen.findByRole("button", { name: "Open on phone" }))
  const dialog = await screen.findByTestId("companion-phone-link-dialog")
  expect(await within(dialog).findByText(fx.phoneLink.url)).toBeInTheDocument()
  expect(calls("companion_phone_link")).toHaveLength(1)
})

it("offers to remove a leftover Serve handler and explains a failed start", async () => {
  replies.companion_status = { ...on({ applied: fx.overviewRunning.settings.applied }), status: fx.statusFailed }
  replies.companion_tailscale_remove = undefined
  render(<CompanionSettingsSection />)
  expect(await screen.findByText("Yuzora applied a handler on desktop.tail1234.ts.net:8443 and has not confirmed it is gone.")).toBeInTheDocument()
  expect(screen.getByText("The Companion stopped unexpectedly.")).toBeInTheDocument()
  // On the node it was applied on, removing works: the record is never just forgotten.
  expect(await screen.findByText("desktop.tail1234.ts.net")).toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Forget this record" })).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Remove it" }))
  await waitFor(() => expect(calls("companion_tailscale_remove")).toHaveLength(1))
})

it("forgets a Serve record only after confirming, once its node is out of reach", async () => {
  const applied = { ...fx.overviewRunning.settings.applied!, host: "old-name.tail1234.ts.net" }
  replies.companion_status = on({ applied })
  replies.companion_tailscale_forget = undefined
  const view = render(<CompanionSettingsSection />)
  expect(await screen.findByText("The current Tailscale node is not the one the handler was applied on, so it cannot be removed from here.")).toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Forget this record" }))
  expect(await screen.findByTestId("companion-serve-off-command")).toHaveTextContent("tailscale serve --https=8443 --set-path=/ off")
  expect(calls("companion_tailscale_forget")).toHaveLength(0)
  fireEvent.click(screen.getByRole("button", { name: "Forget" }))
  await waitFor(() => expect(calls("companion_tailscale_forget")).toHaveLength(1))
  view.unmount()

  // A probe that fails cannot reach the node either.
  useCompanionStore.setState({ overview: null })
  replies.companion_status = on({ applied: fx.overviewRunning.settings.applied })
  replies.companion_tailscale_probe = { reject: "tailscale-unavailable" }
  render(<CompanionSettingsSection />)
  expect(await screen.findByRole("button", { name: "Forget this record" })).toBeInTheDocument()
})

it("maps an enable failure with a detail to its guidance", async () => {
  replies.companion_status = on()
  replies.companion_enable = { reject: "companion-tailscale-refused: funnel" }
  render(<CompanionSettingsSection />)
  fireEvent.click(await screen.findByRole("button", { name: "Enable" }))
  fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Run and enable" }))
  expect(await screen.findByText("Tailscale Funnel is publishing something from this computer to the internet. Turn Funnel off first.")).toBeInTheDocument()
})

it("allowlists local HERDR Sessions and says when a restart is needed", async () => {
  useHerdrStore.setState({ sessions: [
    { name: "default", default: true, running: true, sessionDir: "", socketPath: "" },
    { name: "work", default: false, running: false, sessionDir: "", socketPath: "" },
    { name: "remote", hostId: "ssh-1", default: false, running: true, sessionDir: "", socketPath: "" },
  ] })
  replies.companion_status = on({ sessionAllowlist: ["default", "gone"] })
  replies.companion_sessions_set = fx.sessionsSet
  render(<CompanionSettingsSection />)
  const work = await screen.findByRole("checkbox", { name: /work/ })
  expect(screen.getByRole("checkbox", { name: /default/ })).toBeChecked()
  expect(screen.getByRole("checkbox", { name: /gone/ })).toBeChecked()
  expect(screen.queryByRole("checkbox", { name: /remote/ })).not.toBeInTheDocument()
  fireEvent.click(work)
  await waitFor(() => expect(calls("companion_sessions_set")).toEqual([{ sessions: ["default", "gone", "work"] }]))
  expect(await screen.findByText("The Companion is running: disable and enable it again to apply.")).toBeInTheDocument()
})

it("changes one grant field at a time and copies the other one back", async () => {
  vi.useFakeTimers({ toFake: ["Date"], now: Date.parse("2026-10-18T12:00:00Z") })
  try {
    replies.companion_status = on()
    replies.companion_devices = [fx.device]
    replies.companion_device_update = (args: Record<string, unknown>) => ({ ...fx.device, ...(args.grants ? { grants: args.grants } : {}), ...(args.label ? { label: args.label } : {}) })
    render(<CompanionSettingsSection />)
    const row = await screen.findByTestId("companion-device")
    expect(within(row).getByText(/^Control until/)).toBeInTheDocument()
    fireEvent.click(within(row).getByRole("button", { name: "Control 1 hour" }))
    await waitFor(() => expect(calls("companion_device_update")).toHaveLength(1))
    expect(calls("companion_device_update")[0]).toEqual({ deviceId: fx.device.deviceId, grants: { control: { until: "2026-10-18T13:00:00Z" }, full: false } })
    fireEvent.click(within(row).getByRole("button", { name: "View only" }))
    await waitFor(() => expect(calls("companion_device_update")).toHaveLength(2))
    expect(calls("companion_device_update")[1]).toEqual({ deviceId: fx.device.deviceId, grants: { control: null, full: false } })
    expect(await within(row).findByText("View only", { selector: "span" })).toBeInTheDocument()
    fireEvent.change(within(row).getByRole("textbox", { name: "Device name" }), { target: { value: "Work phone" } })
    fireEvent.click(within(row).getByRole("button", { name: "Save name" }))
    await waitFor(() => expect(calls("companion_device_update")[2]).toEqual({ deviceId: fx.device.deviceId, label: "Work phone" }))
    await waitFor(() => expect(within(row).queryByRole("button", { name: "Save name" })).not.toBeInTheDocument())
    expect(within(row).getByRole("textbox", { name: "Device name" })).toHaveValue("Work phone")
  } finally {
    vi.useRealTimers()
  }
})

it("revokes a device only after confirming, and unlocks a locked one", async () => {
  replies.companion_status = on()
  replies.companion_devices = [{ ...fx.device, locked: true }]
  replies.companion_device_unlock = fx.device
  replies.companion_device_revoke = undefined
  render(<CompanionSettingsSection />)
  const row = await screen.findByTestId("companion-device")
  fireEvent.click(within(row).getByRole("button", { name: "Unlock" }))
  await waitFor(() => expect(calls("companion_device_unlock")).toEqual([{ deviceId: fx.device.deviceId }]))
  fireEvent.click(within(row).getByRole("button", { name: "Revoke" }))
  const dialog = await screen.findByRole("alertdialog")
  expect(within(dialog).getByText("Revoke iPhone?")).toBeInTheDocument()
  expect(calls("companion_device_revoke")).toHaveLength(0)
  replies.companion_devices = []
  fireEvent.click(within(dialog).getByRole("button", { name: "Revoke" }))
  await waitFor(() => expect(calls("companion_device_revoke")).toEqual([{ deviceId: fx.device.deviceId }]))
  expect(await screen.findByText("No paired phones.")).toBeInTheDocument()
})
