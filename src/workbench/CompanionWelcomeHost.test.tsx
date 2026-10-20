import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { beforeEach, expect, it, vi } from "vitest"
import type { CompanionOverview } from "@/lib/companionIpc"
import { useAnyOverlayOpen } from "@/state/overlayStore"
import { useCompanionStore } from "@/state/companionStore"
import { useUiStore } from "@/state/uiStore"
import { companionFixtures as fx, companionInvoke, type CompanionReplies } from "@/test/companionFixtures"

const ipc = vi.hoisted(() => ({ invoke: vi.fn() }))
vi.mock("@/lib/ipc", async importOriginal => ({ ...(await importOriginal<typeof import("@/lib/ipc")>()), invoke: ipc.invoke }))
const ui = vi.hoisted(() => ({ toast: Object.assign(vi.fn(), { error: vi.fn() }), openUrl: vi.fn(), writeText: vi.fn() }))
vi.mock("sonner", () => ({ toast: ui.toast }))
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: ui.openUrl }))
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: ui.writeText }))
import { CompanionSettingsSection } from "@/app/workbench/CompanionSettingsSection"
import { CompanionWelcomeHost } from "./CompanionWelcomeHost"

let replies: CompanionReplies
const calls = (command: string) => ipc.invoke.mock.calls.filter(([name]) => name === command).map(([, args]) => args)
const running = (patch: Partial<CompanionOverview["net"]> = {}): CompanionOverview => ({ ...fx.overviewRunning, net: { ...fx.netRunning, ...patch } })
const store = () => useCompanionStore.getState()

/** Reports whether any overlay is open, for the native Browser layer's z-order gate. */
function OverlayProbe() {
  return <span data-testid="overlay-open">{String(useAnyOverlayOpen())}</span>
}

beforeEach(() => {
  replies = { companion_status: fx.overviewOff, companion_devices: [], companion_pairing_create: fx.pairingQr, companion_phone_link: fx.phoneLink }
  ipc.invoke.mockReset().mockImplementation(companionInvoke(() => replies))
  ui.toast.mockReset()
  ui.toast.error.mockReset()
  ui.openUrl.mockReset().mockResolvedValue(undefined)
  Object.assign(URL, { createObjectURL: () => "blob:tauri://localhost/qr", revokeObjectURL: () => undefined })
  useCompanionStore.setState({ overview: fx.overviewOff, devices: [], claims: [], settled: {}, welcomePending: false, enableError: null })
  useUiStore.setState({ settingsOpen: false })
})

it("opens the pairing dialog once the Companion serves and no phone is paired", async () => {
  useCompanionStore.setState({ welcomePending: true })
  render(<><CompanionWelcomeHost /><OverlayProbe /></>)
  expect(screen.getByTestId("overlay-open")).toHaveTextContent("false")
  act(() => useCompanionStore.setState({ overview: running() }))
  expect(await screen.findByTestId("companion-pairing-dialog")).toBeInTheDocument()
  expect(ui.toast).not.toHaveBeenCalled()
  expect(store().welcomePending).toBe(false)
  // Shown outside Settings, so the native layer must know about it.
  expect(screen.getByTestId("overlay-open")).toHaveTextContent("true")
})

it("offers the phone address in a notice when a phone is already paired", async () => {
  useCompanionStore.setState({ welcomePending: true, devices: [fx.device] })
  render(<CompanionWelcomeHost />)
  act(() => useCompanionStore.setState({ overview: running() }))
  await waitFor(() => expect(ui.toast).toHaveBeenCalledTimes(1))
  expect(screen.queryByTestId("companion-pairing-dialog")).not.toBeInTheDocument()
  const [message, options] = ui.toast.mock.calls[0] as [string, { action: { label: string; onClick: () => void } }]
  expect(message).toBe("The Companion is on")
  expect(options.action.label).toBe("Open on phone")
  act(() => options.action.onClick())
  expect(await screen.findByTestId("companion-phone-link-dialog")).toBeInTheDocument()
  // The host renders it itself: Settings stays closed.
  expect(useUiStore.getState().settingsOpen).toBe(false)
})

it("waits until the listener is open, not only the gateway", async () => {
  useCompanionStore.setState({ welcomePending: true })
  render(<CompanionWelcomeHost />)
  act(() => useCompanionStore.setState({ overview: running({ serving: false }) }))
  expect(screen.queryByTestId("companion-pairing-dialog")).not.toBeInTheDocument()
  expect(store().welcomePending).toBe(true)
  act(() => store().applyEvent({ type: "net", net: fx.netRunning }))
  expect(await screen.findByTestId("companion-pairing-dialog")).toBeInTheDocument()
})

it("greets nobody who did not just enable", () => {
  render(<CompanionWelcomeHost />)
  act(() => useCompanionStore.setState({ overview: running() }))
  expect(screen.queryByTestId("companion-pairing-dialog")).not.toBeInTheDocument()
  expect(ui.toast).not.toHaveBeenCalled()
})

it("does not greet while the Companion is not running", () => {
  useCompanionStore.setState({ welcomePending: true })
  render(<CompanionWelcomeHost />)
  act(() => useCompanionStore.setState({ overview: { ...running(), status: fx.statusStarting } }))
  expect(screen.queryByTestId("companion-pairing-dialog")).not.toBeInTheDocument()
  expect(ui.toast).not.toHaveBeenCalled()
  expect(store().welcomePending).toBe(true)
})

it("greets once per enable", async () => {
  useCompanionStore.setState({ welcomePending: true, devices: [fx.device] })
  render(<CompanionWelcomeHost />)
  act(() => useCompanionStore.setState({ overview: running() }))
  await waitFor(() => expect(ui.toast).toHaveBeenCalledTimes(1))
  act(() => useCompanionStore.setState({ overview: { ...running(), status: fx.statusStopping } }))
  act(() => useCompanionStore.setState({ overview: running() }))
  expect(ui.toast).toHaveBeenCalledTimes(1)
})

it("still greets after Settings closed in the middle of the enable", async () => {
  useCompanionStore.setState({ overview: { ...fx.overviewOff, settings: { ...fx.overviewOff.settings, experimental: true }, net: fx.netStopped } })
  replies.companion_status = useCompanionStore.getState().overview
  let finish: () => void = () => undefined
  replies.companion_enable = () => new Promise(resolve => { finish = () => resolve(fx.overviewRunning.status) })
  render(<CompanionWelcomeHost />)
  const card = render(<CompanionSettingsSection />)
  fireEvent.click(await card.findByRole("button", { name: "Enable" }))
  fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Enable remote control" }))
  await waitFor(() => expect(calls("companion_enable")).toHaveLength(1))
  // Settings closes while the certificate is still being issued.
  card.unmount()
  expect(store().welcomePending).toBe(true)
  replies.companion_status = fx.overviewRunning
  act(() => useCompanionStore.setState({ overview: fx.overviewRunning }))
  finish()
  expect(await screen.findByTestId("companion-pairing-dialog")).toBeInTheDocument()
})

/** A Settings card on a signed-in node that is about to enable; the enable fails when `fail()` is called. */
function enableThatFails() {
  const offline = { ...fx.overviewOff, settings: { ...fx.overviewOff.settings, experimental: true }, net: fx.netStopped }
  useCompanionStore.setState({ overview: offline })
  replies.companion_status = offline
  const control = { fail: (_code: string) => undefined as void }
  replies.companion_enable = () => new Promise((_resolve, reject) => { control.fail = code => reject(code) })
  return control
}

async function startEnable(card: ReturnType<typeof render>) {
  fireEvent.click(await card.findByRole("button", { name: "Enable" }))
  fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Enable remote control" }))
  await waitFor(() => expect(calls("companion_enable")).toHaveLength(1))
}

it("tells the user once that an enable failed after Settings closed, and the reopened card explains it", async () => {
  const enable = enableThatFails()
  useUiStore.setState({ settingsOpen: true })
  render(<CompanionWelcomeHost />)
  const card = render(<CompanionSettingsSection />)
  await startEnable(card)
  card.unmount()
  useUiStore.setState({ settingsOpen: false })
  await act(async () => enable.fail("companion-net-https-disabled"))
  await waitFor(() => expect(ui.toast.error).toHaveBeenCalledTimes(1))
  expect(ui.toast.error.mock.calls[0][0]).toMatch(/^HTTPS certificates are not enabled for this tailnet/)
  expect(store().welcomePending).toBe(false)
  // What the notice cannot carry is still on the card the user opens next.
  const reopened = render(<CompanionSettingsSection />)
  expect(await reopened.findByText(/HTTPS certificates are not enabled for this tailnet/)).toBeInTheDocument()
  fireEvent.click(reopened.getByRole("button", { name: "Open the Tailscale DNS settings" }))
  expect(ui.openUrl).toHaveBeenCalledWith("https://login.tailscale.com/admin/dns")
  expect(ui.toast.error).toHaveBeenCalledTimes(1)
  expect(ui.toast).not.toHaveBeenCalled()
})

it("names the configured port in the notice of an enable that found it taken", () => {
  useCompanionStore.setState({ welcomePending: true, overview: { ...fx.overviewOff, settings: { ...fx.overviewOff.settings, gatewayPort: 47999 } } })
  render(<CompanionWelcomeHost />)
  act(() => store().setEnableError("port-in-use"))
  expect(ui.toast.error).toHaveBeenCalledTimes(1)
  expect(ui.toast.error).toHaveBeenLastCalledWith("Another program is using port 47999, which the Companion needs. Quit that program and try again.")
})

it("keeps the generic notice of a taken port while no overview is loaded", () => {
  useCompanionStore.setState({ welcomePending: true, overview: null })
  render(<CompanionWelcomeHost />)
  act(() => store().setEnableError("port-in-use"))
  expect(ui.toast.error).toHaveBeenLastCalledWith("Another program is using the Companion's port. Quit that program and try again.")
})

it("leaves the failure to the card while Settings is open", async () => {
  const enable = enableThatFails()
  useUiStore.setState({ settingsOpen: true })
  render(<CompanionWelcomeHost />)
  const card = render(<CompanionSettingsSection />)
  await startEnable(card)
  await act(async () => enable.fail("companion-net-login-required"))
  expect(await card.findByText("Tailscale needs you to sign in again.")).toBeInTheDocument()
  expect(ui.toast.error).not.toHaveBeenCalled()
})

it("does not announce a failure of an enable it did not track", () => {
  render(<CompanionWelcomeHost />)
  act(() => store().setEnableError("companion-net-https-disabled"))
  expect(ui.toast.error).not.toHaveBeenCalled()
})

it("announces a tracked failure with the code's text, and a new one again", () => {
  useCompanionStore.setState({ welcomePending: true })
  render(<CompanionWelcomeHost />)
  act(() => store().setEnableError("companion-net-start-failed: No such file or directory"))
  expect(ui.toast.error).toHaveBeenCalledTimes(1)
  expect(ui.toast.error).toHaveBeenLastCalledWith("The Tailscale connection could not start. (No such file or directory)")
  // The same failure is not read twice, a later enable that fails the same way is.
  act(() => useCompanionStore.setState({ overview: fx.overviewOff }))
  expect(ui.toast.error).toHaveBeenCalledTimes(1)
  act(() => store().setEnableError(null))
  act(() => store().setEnableError("companion-net-start-failed: No such file or directory"))
  expect(ui.toast.error).toHaveBeenCalledTimes(2)
})
