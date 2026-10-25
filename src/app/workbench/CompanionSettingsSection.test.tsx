import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { beforeEach, expect, it, vi } from "vitest"
import type { CompanionOverview } from "@/lib/companionIpc"
import { useCompanionStore } from "@/state/companionStore"
import { useHerdrStore } from "@/state/herdrStore"
import { companionFixtures as fx, companionInvoke, type CompanionReplies } from "@/test/companionFixtures"

const ipc = vi.hoisted(() => ({ invoke: vi.fn() }))
vi.mock("@/lib/ipc", async importOriginal => ({ ...(await importOriginal<typeof import("@/lib/ipc")>()), invoke: ipc.invoke }))
const ui = vi.hoisted(() => ({ toast: Object.assign(vi.fn(), { error: vi.fn() }), openUrl: vi.fn(), writeText: vi.fn() }))
vi.mock("sonner", () => ({ toast: ui.toast }))
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: ui.openUrl }))
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: ui.writeText }))
import { CompanionSettingsSection } from "./CompanionSettingsSection"

let replies: CompanionReplies
const on = (patch: Partial<CompanionOverview["settings"]> = {}, overview: Partial<CompanionOverview> = {}): CompanionOverview =>
  ({ ...fx.overviewOff, ...overview, settings: { ...fx.overviewOff.settings, experimental: true, ...patch } })
const calls = (command: string) => ipc.invoke.mock.calls.filter(([name]) => name === command).map(([, args]) => args)

/** A node that signed in before: only enabling is left. */
const signedIn = (patch: Partial<CompanionOverview> = {}) => on({}, { net: fx.netStopped, ...patch })

beforeEach(() => {
  replies = { companion_status: fx.overviewOff, companion_devices: [] }
  ipc.invoke.mockReset().mockImplementation(companionInvoke(() => replies))
  ui.toast.mockReset()
  ui.toast.error.mockReset()
  ui.openUrl.mockReset().mockResolvedValue(undefined)
  ui.writeText.mockReset().mockResolvedValue(undefined)
  useCompanionStore.setState({ overview: null, devices: [], lastGitRead: {}, claims: [], settled: {}, welcomePending: false, enableError: null })
  useHerdrStore.setState({ sessions: [] })
})

it("shows only the Experimental switch while the flag is off, and turns it on", async () => {
  replies.companion_experimental_set = on()
  render(<CompanionSettingsSection />)
  const flag = await screen.findByRole("switch", { name: /Companion \(phone remote control\)/ })
  expect(flag).not.toBeChecked()
  expect(screen.queryByText("Tailscale connection")).not.toBeInTheDocument()
  fireEvent.click(flag)
  await waitFor(() => expect(calls("companion_experimental_set")).toEqual([{ enabled: true }]))
  expect(await screen.findByText("Remote control of this computer")).toBeInTheDocument()
  expect(screen.getByText("Tailscale connection")).toBeInTheDocument()
})

it("locks every mutation when another Yuzora owns the Companion", async () => {
  replies.companion_status = signedIn({ owner: false })
  render(<CompanionSettingsSection />)
  expect(await screen.findByText("Another Yuzora owns the Companion")).toBeInTheDocument()
  expect(screen.getByRole("switch", { name: /Companion/ })).toBeDisabled()
  expect(screen.getByRole("button", { name: "Enable" })).toBeDisabled()
  expect(screen.getByRole("button", { name: "Sign out" })).toBeDisabled()
})

it("signs in to Tailscale from the card when the node has no login yet", async () => {
  replies.companion_status = on({}, { net: fx.netLoggedOut })
  replies.companion_net_login = { ...fx.netLoggedOut, state: "starting", mode: "login" }
  render(<CompanionSettingsSection />)
  expect(await screen.findByText(/This computer is not connected yet/)).toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Enable" })).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Sign in to Tailscale" }))
  await waitFor(() => expect(calls("companion_net_login")).toHaveLength(1))
  // The node's own events carry the sign-in from here on.
  act(() => useCompanionStore.getState().applyEvent({ type: "net", net: fx.netNeedsLogin }))
  expect(await screen.findByText("Finish signing in with your browser. This updates by itself.")).toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Sign in to Tailscale" })).not.toBeInTheDocument()
})

it("opens or copies the sign-in page, and gives up on request", async () => {
  replies.companion_status = on({}, { net: fx.netNeedsLogin })
  replies.companion_net_cancel = fx.netLoggedOut
  render(<CompanionSettingsSection />)
  fireEvent.click(await screen.findByRole("button", { name: "Open in browser" }))
  expect(ui.openUrl).toHaveBeenCalledWith("https://login.tailscale.com/a/abc123")
  fireEvent.click(screen.getByRole("button", { name: "Copy link" }))
  await waitFor(() => expect(ui.writeText).toHaveBeenCalledWith("https://login.tailscale.com/a/abc123"))
  expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
  await waitFor(() => expect(calls("companion_net_cancel")).toHaveLength(1))
})

it("opens nothing but an https sign-in page", async () => {
  replies.companion_status = on({}, { net: { ...fx.netNeedsLogin, authUrl: "file:///etc/passwd" } })
  render(<CompanionSettingsSection />)
  expect(await screen.findByRole("button", { name: "Open in browser" })).toBeDisabled()
  expect(screen.getByRole("button", { name: "Copy link" })).toBeDisabled()
})

it("shows a sign-in that is still connecting with a way to cancel", async () => {
  replies.companion_status = on({}, { net: { ...fx.netLoggedOut, state: "starting", mode: "login" } })
  render(<CompanionSettingsSection />)
  expect(await screen.findByText("Connecting to Tailscale…")).toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Open in browser" })).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Cancel" })).toBeInTheDocument()
})

/** The node signed in but waits for a tailnet admin to approve the device. */
const approval = (patch: Partial<typeof fx.netNeedsLogin> = {}) => ({ ...fx.netNeedsLogin, state: "needsApproval" as const, authUrl: null, ...patch })

it("tells the user which machine to approve while the node waits for device approval", async () => {
  replies.companion_status = on({}, { net: approval() })
  replies.companion_net_cancel = fx.netLoggedOut
  render(<CompanionSettingsSection />)
  expect(await screen.findByText(/Waiting for approval\. In the Tailscale admin console, approve the new machine marked “Needs approval” on the Machines page/)).toBeInTheDocument()
  expect(screen.queryByText("Connecting to Tailscale…")).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Open in browser" })).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Copy link" })).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Open the Tailscale Machines page" }))
  expect(ui.openUrl).toHaveBeenCalledWith("https://login.tailscale.com/admin/machines")
  // There is no timeout: waiting for the admin can be given up on.
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
  await waitFor(() => expect(calls("companion_net_cancel")).toHaveLength(1))
})

it("names the machine after the first label of the name the node reports while it waits", async () => {
  // A second computer on the tailnet: the pending machine is "yuzora-1", not the approved "yuzora".
  replies.companion_status = on({}, { net: approval({ dnsName: "yuzora-1.tail1234.ts.net" }) })
  render(<CompanionSettingsSection />)
  const waiting = await screen.findByText(/approve the machine "yuzora-1" on the Machines page/)
  expect(waiting).not.toHaveTextContent(/machine "yuzora"/)
  expect(waiting).not.toHaveTextContent("Needs approval")
})

it("names no machine while the node has not reported its name", async () => {
  replies.companion_status = on({}, { net: approval({ dnsName: null }) })
  render(<CompanionSettingsSection />)
  const waiting = await screen.findByText(/^Waiting for approval\./)
  expect(waiting).toHaveTextContent("approve the new machine marked “Needs approval” on the Machines page")
  // The old fallback named the machine "yuzora": the other, already approved computer.
  expect(waiting.textContent).not.toMatch(/yuzora/i)
})

it("lets the user leave a pending approval for another account, and signing in again starts afresh", async () => {
  let backend = on({}, { net: approval({ dnsName: "yuzora-1.tail1234.ts.net" }) })
  replies.companion_status = () => backend
  replies.companion_net_logout = () => { backend = on({}, { net: fx.netLoggedOut }); return fx.netLoggedOut }
  replies.companion_net_login = () => { backend = on({}, { net: { ...fx.netLoggedOut, state: "starting", mode: "login" } }); return backend.net }
  render(<CompanionSettingsSection />)
  const other = await screen.findByRole("button", { name: "Use another account" })
  // Next to the Machines page and Cancel, which stay.
  expect(screen.getByRole("button", { name: "Open the Tailscale Machines page" })).toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Cancel" })).toBeInTheDocument()
  fireEvent.click(other)
  const dialog = await screen.findByRole("alertdialog")
  expect(within(dialog).getByText("Use another Tailscale account?")).toBeInTheDocument()
  // The pending machine is not removed by this: an admin has to.
  expect(dialog).toHaveTextContent(/stays listed in the Tailscale admin console until an admin removes it/)
  expect(calls("companion_net_logout")).toHaveLength(0)
  fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }))
  expect(calls("companion_net_logout")).toHaveLength(0)
  fireEvent.click(screen.getByRole("button", { name: "Use another account" }))
  fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Use another account" }))
  await waitFor(() => expect(calls("companion_net_logout")).toHaveLength(1))
  // Signed out: the waiting is over and a new registration can start.
  fireEvent.click(await screen.findByRole("button", { name: "Sign in to Tailscale" }))
  await waitFor(() => expect(calls("companion_net_login")).toHaveLength(1))
  expect(screen.queryByText(/Waiting for approval\./)).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Use another account" })).not.toBeInTheDocument()
  // The fresh registration brings its own sign-in page.
  act(() => useCompanionStore.getState().applyEvent({ type: "net", net: { ...fx.netNeedsLogin, authUrl: "https://login.tailscale.com/a/fresh456" } }))
  fireEvent.click(await screen.findByRole("button", { name: "Open in browser" }))
  expect(ui.openUrl).toHaveBeenCalledWith("https://login.tailscale.com/a/fresh456")
})

it("offers 'Use another account' only while a sign-in waits for approval", async () => {
  replies.companion_status = on({}, { net: fx.netNeedsLogin })
  render(<CompanionSettingsSection />)
  await screen.findByRole("button", { name: "Open in browser" })
  expect(screen.queryByRole("button", { name: "Use another account" })).not.toBeInTheDocument()
})

it("follows a sign-in into the wait for approval and out of it", async () => {
  replies.companion_status = on({}, { net: fx.netNeedsLogin })
  render(<CompanionSettingsSection />)
  expect(await screen.findByText("Finish signing in with your browser. This updates by itself.")).toBeInTheDocument()
  act(() => useCompanionStore.getState().applyEvent({ type: "net", net: approval() }))
  expect(screen.getByText(/Waiting for approval\./)).toBeInTheDocument()
  expect(screen.queryByText("Finish signing in with your browser. This updates by itself.")).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Open the Tailscale Machines page" })).toBeInTheDocument()
  // The admin approved: the node runs and, signed in, stops.
  act(() => useCompanionStore.getState().applyEvent({ type: "net", net: { ...fx.netStopped, state: "running", mode: "login" } }))
  expect(screen.queryByText(/Waiting for approval\./)).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Open the Tailscale Machines page" })).not.toBeInTheDocument()
})

it("explains an enable that stopped for device approval and links to the Machines page", async () => {
  replies.companion_status = signedIn()
  replies.companion_enable = { reject: "companion-net-approval-required" }
  render(<CompanionSettingsSection />)
  fireEvent.click(await screen.findByRole("button", { name: "Enable" }))
  fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Enable remote control" }))
  expect(await screen.findByText(/This computer is waiting for device approval on your tailnet/)).toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Open the Tailscale Machines page" }))
  expect(ui.openUrl).toHaveBeenCalledWith("https://login.tailscale.com/admin/machines")
  // The guidance stays after the click.
  expect(screen.getByRole("button", { name: "Open the Tailscale Machines page" })).toBeInTheDocument()
  expect(ui.toast).not.toHaveBeenCalled()
  expect(useCompanionStore.getState().welcomePending).toBe(false)
})

it("shows the enable progress and no Cancel when the card opens in the middle of an enable", async () => {
  // Cancel only stops a sign-in: on an enable it would wait for the command and do nothing.
  replies.companion_status = on({}, { net: fx.netCertPending })
  render(<CompanionSettingsSection />)
  expect(await screen.findByText("Getting the HTTPS certificate. The first time takes about 30 seconds…")).toBeInTheDocument()
  expect(screen.queryByText("Connecting to Tailscale…")).not.toBeInTheDocument()
  for (const name of ["Cancel", "Enable", "Sign in to Tailscale", "Sign out", "Open in browser"]) {
    expect(screen.queryByRole("button", { name })).not.toBeInTheDocument()
  }
  act(() => useCompanionStore.getState().applyEvent({ type: "net", net: fx.netEnabling }))
  expect(screen.getByText("Connecting to Tailscale…")).toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument()
})

it("does not take a node that runs for no mode for a sign-in", async () => {
  replies.companion_status = signedIn({ net: { ...fx.netStopped, state: "running", mode: null } })
  render(<CompanionSettingsSection />)
  expect(await screen.findByRole("button", { name: "Enable" })).toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument()
  expect(screen.queryByText("Connecting to Tailscale…")).not.toBeInTheDocument()
})

it("offers nothing to press while the node signs out", async () => {
  replies.companion_status = on({}, { net: fx.netSigningOut })
  render(<CompanionSettingsSection />)
  expect(await screen.findByText("Signing out of Tailscale…")).toBeInTheDocument()
  expect(screen.queryByTestId("companion-status")).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: /^(Cancel|Enable|Sign in to Tailscale|Sign out|Open in browser)$/ })).not.toBeInTheDocument()
})

it("shows the node, and enables only after confirming the remote control warning", async () => {
  replies.companion_status = signedIn()
  replies.companion_enable = fx.overviewRunning.status
  render(<CompanionSettingsSection />)
  expect(await screen.findByText("desktop.tail1234.ts.net")).toBeInTheDocument()
  expect(screen.getByText("user@example.com")).toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Enable" }))
  const dialog = await screen.findByRole("alertdialog")
  expect(within(dialog).getByText(/equivalent to giving it shell access to this account/)).toBeInTheDocument()
  expect(dialog).not.toHaveTextContent(/tailscale serve/i)
  expect(calls("companion_enable")).toHaveLength(0)
  fireEvent.click(within(dialog).getByRole("button", { name: "Enable remote control" }))
  await waitFor(() => expect(calls("companion_enable")).toEqual([{ window: "8h", confirm: true }]))
})

it("follows an enable through the node's events and leaves the welcome to the host", async () => {
  replies.companion_status = signedIn()
  let finish: () => void = () => undefined
  replies.companion_enable = () => new Promise(resolve => { finish = () => resolve(fx.overviewRunning.status) })
  render(<CompanionSettingsSection />)
  fireEvent.click(await screen.findByRole("button", { name: "Enable" }))
  fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Enable remote control" }))
  await waitFor(() => expect(calls("companion_enable")).toHaveLength(1))
  const net = (value: typeof fx.netStopped) => act(() => useCompanionStore.getState().applyEvent({ type: "net", net: value }))
  expect(await screen.findByText("Connecting to Tailscale…")).toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Enable" })).not.toBeInTheDocument()
  net(fx.netEnabling)
  expect(screen.getByText("Connecting to Tailscale…")).toBeInTheDocument()
  net(fx.netCertPending)
  expect(screen.getByText("Getting the HTTPS certificate. The first time takes about 30 seconds…")).toBeInTheDocument()
  net({ ...fx.netCertPending, cert: { state: "error", notAfter: null, error: "rate limited" } })
  expect(screen.getByText("The certificate request failed. Trying again…")).toBeInTheDocument()
  net({ ...fx.netRunning, serving: false })
  expect(screen.getByText("Starting the Companion…")).toBeInTheDocument()
  // A welcome is due from the moment the user started; the host shows it, not this card.
  expect(useCompanionStore.getState().welcomePending).toBe(true)
  replies.companion_status = fx.overviewRunning
  finish()
  expect(await screen.findByRole("button", { name: "Disable" })).toBeInTheDocument()
  expect(useCompanionStore.getState().welcomePending).toBe(true)
  expect(screen.queryByTestId("companion-pairing-dialog")).not.toBeInTheDocument()
  expect(ui.toast).not.toHaveBeenCalled()
})

it("owes no welcome when the enable did not leave the Companion running", async () => {
  replies.companion_status = signedIn()
  replies.companion_enable = fx.overviewRunning.status
  render(<CompanionSettingsSection />)
  fireEvent.click(await screen.findByRole("button", { name: "Enable" }))
  fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Enable remote control" }))
  await waitFor(() => expect(calls("companion_enable")).toHaveLength(1))
  await waitFor(() => expect(screen.getByRole("button", { name: "Enable" })).toBeEnabled())
  expect(useCompanionStore.getState().welcomePending).toBe(false)
})

it("explains a tailnet without HTTPS certificates and links to where to turn them on", async () => {
  replies.companion_status = signedIn()
  replies.companion_enable = { reject: "companion-net-https-disabled" }
  render(<CompanionSettingsSection />)
  fireEvent.click(await screen.findByRole("button", { name: "Enable" }))
  fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Enable remote control" }))
  expect(await screen.findByText(/HTTPS certificates are not enabled for this tailnet/)).toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Open the Tailscale DNS settings" }))
  expect(ui.openUrl).toHaveBeenCalledWith("https://login.tailscale.com/admin/dns")
  // The guidance stays after the click.
  expect(screen.getByRole("button", { name: "Open the Tailscale DNS settings" })).toBeInTheDocument()
  expect(ui.toast).not.toHaveBeenCalled()
  expect(useCompanionStore.getState().welcomePending).toBe(false)
})

it("offers to sign in again when the node needs it", async () => {
  replies.companion_status = signedIn()
  replies.companion_enable = { reject: "companion-net-login-required" }
  render(<CompanionSettingsSection />)
  expect(screen.queryByRole("button", { name: "Sign in to Tailscale" })).not.toBeInTheDocument()
  fireEvent.click(await screen.findByRole("button", { name: "Enable" }))
  fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Enable remote control" }))
  expect(await screen.findByText("Tailscale needs you to sign in again.")).toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Sign in to Tailscale" })).toBeInTheDocument()
})

it("explains an enable that failed before the card was opened, and drops it when the next enable starts", async () => {
  replies.companion_status = signedIn()
  useCompanionStore.setState({ enableError: "companion-net-https-disabled" })
  let finish: () => void = () => undefined
  replies.companion_enable = () => new Promise(resolve => { finish = () => resolve(fx.overviewRunning.status) })
  render(<CompanionSettingsSection />)
  expect(await screen.findByText(/HTTPS certificates are not enabled for this tailnet/)).toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Open the Tailscale DNS settings" }))
  expect(ui.openUrl).toHaveBeenCalledWith("https://login.tailscale.com/admin/dns")
  fireEvent.click(screen.getByRole("button", { name: "Enable" }))
  fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Enable remote control" }))
  await waitFor(() => expect(calls("companion_enable")).toHaveLength(1))
  expect(useCompanionStore.getState().enableError).toBeNull()
  expect(screen.queryByText(/HTTPS certificates are not enabled for this tailnet/)).not.toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Open the Tailscale DNS settings" })).not.toBeInTheDocument()
  finish()
})

it("offers to sign in again on a card opened after the enable failed for it, once", async () => {
  replies.companion_status = signedIn()
  useCompanionStore.setState({ enableError: "companion-net-login-required" })
  replies.companion_net_login = { ...fx.netLoggedOut, state: "starting", mode: "login" }
  render(<CompanionSettingsSection />)
  expect(await screen.findByText("Tailscale needs you to sign in again.")).toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Sign in to Tailscale" }))
  await waitFor(() => expect(calls("companion_net_login")).toHaveLength(1))
  // Signing in starts over: the old explanation does not wait around for the next enable.
  expect(useCompanionStore.getState().enableError).toBeNull()
})

it("shows a key that expired while serving once, and offers to sign in", async () => {
  // The gateway halts with the node's code; the node is stopped and no longer logged in.
  const net = { ...fx.netLoggedOut, error: "companion-net-login-required" }
  replies.companion_status = { ...on({}, { net }), status: { state: "failed", code: "companion-net-login-required" } }
  render(<CompanionSettingsSection />)
  expect(await screen.findByText("Tailscale needs you to sign in again.")).toBeInTheDocument()
  expect(screen.getAllByText("Tailscale needs you to sign in again.")).toHaveLength(1)
  expect(screen.getByTestId("companion-status")).toHaveTextContent("Stopped with an error")
  expect(screen.getByRole("button", { name: "Sign in to Tailscale" })).toBeEnabled()
  expect(screen.queryByRole("button", { name: "Enable" })).not.toBeInTheDocument()
  expect(screen.queryByText("Starting the Companion…")).not.toBeInTheDocument()
})

it("does not carry the failure of an earlier run into a sign-in under way, and ends on the signed-in node", async () => {
  // The key expired while serving; the node forgot its login and the Companion stopped with that code.
  let backend: CompanionOverview = {
    ...on({}, { net: { ...fx.netLoggedOut, error: "companion-net-login-required" } }),
    status: { state: "failed", code: "companion-net-login-required" },
  }
  replies.companion_status = () => backend
  // The node is up for the sign-in, but the backend's reset of the old failure has not been seen yet.
  replies.companion_net_login = () => {
    backend = { ...backend, net: { ...fx.netLoggedOut, state: "starting", mode: "login" } }
    return backend.net
  }
  render(<CompanionSettingsSection />)
  expect(await screen.findByText("Tailscale needs you to sign in again.")).toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Sign in to Tailscale" }))
  expect(await screen.findByText("Connecting to Tailscale…")).toBeInTheDocument()
  expect(screen.queryByText("Tailscale needs you to sign in again.")).not.toBeInTheDocument()
  // The backend's order: the old failure is cleared first, then the node's own events.
  const apply = (event: Parameters<ReturnType<typeof useCompanionStore.getState>["applyEvent"]>[0]) => act(() => useCompanionStore.getState().applyEvent(event))
  backend = { ...backend, status: { state: "disabled" }, net: fx.netNeedsLogin }
  apply({ type: "status", status: { state: "disabled" } })
  apply({ type: "net", net: fx.netNeedsLogin })
  expect(await screen.findByText("Finish signing in with your browser. This updates by itself.")).toBeInTheDocument()
  expect(screen.queryByText("Tailscale needs you to sign in again.")).not.toBeInTheDocument()
  backend = { ...backend, net: fx.netStopped }
  apply({ type: "net", net: { ...fx.netStopped, state: "running", mode: "login" } })
  apply({ type: "net", net: fx.netStopped })
  expect(await screen.findByRole("button", { name: "Enable" })).toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Sign out" })).toBeInTheDocument()
  expect(screen.getByText("desktop.tail1234.ts.net")).toBeInTheDocument()
  expect(screen.getByTestId("companion-status")).toHaveTextContent("Off")
  expect(screen.queryByRole("button", { name: "Sign in to Tailscale" })).not.toBeInTheDocument()
  expect(screen.queryByText("Tailscale needs you to sign in again.")).not.toBeInTheDocument()
  // Only the experimental warning is left.
  expect(screen.getAllByRole("alert")).toHaveLength(1)
})

it("shows the progress of a re-enable after a runtime failure, also on a card opened in the middle of it", async () => {
  let backend: CompanionOverview = { ...signedIn(), status: fx.statusFailed }
  replies.companion_status = () => backend
  let finish: () => void = () => undefined
  replies.companion_enable = () => new Promise(resolve => { finish = () => resolve(fx.overviewRunning.status) })
  const first = render(<CompanionSettingsSection />)
  expect(await screen.findByText("The Companion stopped unexpectedly.")).toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Enable" }))
  fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Enable remote control" }))
  await waitFor(() => expect(calls("companion_enable")).toHaveLength(1))
  // The card that started it shows progress, not the failure it replaces.
  expect(await screen.findByText("Connecting to Tailscale…")).toBeInTheDocument()
  expect(screen.queryByText("The Companion stopped unexpectedly.")).not.toBeInTheDocument()
  // The backend clears the old failure before it starts the node, then the node reports.
  backend = { ...backend, status: { state: "disabled" }, net: fx.netCertPending }
  act(() => {
    useCompanionStore.getState().applyEvent({ type: "status", status: { state: "disabled" } })
    useCompanionStore.getState().applyEvent({ type: "net", net: fx.netCertPending })
  })
  // Settings are closed and opened again in the middle of it.
  first.unmount()
  render(<CompanionSettingsSection />)
  expect(await screen.findByText("Getting the HTTPS certificate. The first time takes about 30 seconds…")).toBeInTheDocument()
  expect(screen.queryByText("The Companion stopped unexpectedly.")).not.toBeInTheDocument()
  expect(screen.queryByTestId("companion-status")).not.toBeInTheDocument()
  for (const name of ["Enable", "Sign out", "Sign in to Tailscale", "Cancel"]) {
    expect(screen.queryByRole("button", { name })).not.toBeInTheDocument()
  }
  backend = fx.overviewRunning
  finish()
  expect(await screen.findByRole("button", { name: "Disable" })).toBeInTheDocument()
})

it("says a sign-in the enable also failed for once", async () => {
  // The enable rejected with the code the gateway then halted with: one sentence, not two.
  replies.companion_status = { ...on({}, { net: fx.netLoggedOut }), status: { state: "failed", code: "companion-net-login-required" } }
  useCompanionStore.setState({ enableError: "companion-net-login-required" })
  render(<CompanionSettingsSection />)
  expect(await screen.findByText("Tailscale needs you to sign in again.")).toBeInTheDocument()
  expect(screen.getAllByText("Tailscale needs you to sign in again.")).toHaveLength(1)
})

it("shows the normal status and footer while the Companion winds down with the node still up", async () => {
  // The node reports no mode once it is being stopped: that is not an enable in progress.
  const winding = { ...fx.netRunning, mode: null }
  replies.companion_status = { ...on({}, { net: winding }), status: { state: "disabled" } }
  render(<CompanionSettingsSection />)
  expect(await screen.findByTestId("companion-status")).toHaveTextContent("Off")
  expect(screen.queryByText("Starting the Companion…")).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Enable" })).toBeInTheDocument()
  act(() => useCompanionStore.getState().applyEvent({ type: "status", status: fx.statusStopping }))
  expect(screen.getByTestId("companion-status")).toHaveTextContent("Stopping")
  expect(screen.queryByText("Starting the Companion…")).not.toBeInTheDocument()
  expect(screen.getByRole("button", { name: "Disable" })).toBeInTheDocument()
})

it("signs the node out only after confirming", async () => {
  replies.companion_status = signedIn()
  replies.companion_net_logout = fx.netLoggedOut
  render(<CompanionSettingsSection />)
  fireEvent.click(await screen.findByRole("button", { name: "Sign out" }))
  const dialog = await screen.findByRole("alertdialog")
  expect(within(dialog).getByText("Sign this computer out of Tailscale?")).toBeInTheDocument()
  // The node name is the phones' passkey host: a new name orphans every paired phone.
  expect(dialog).toHaveTextContent(/If the name changes, paired phones can no longer log in and have to be paired again/)
  expect(calls("companion_net_logout")).toHaveLength(0)
  fireEvent.click(within(dialog).getByRole("button", { name: "Sign out" }))
  await waitFor(() => expect(calls("companion_net_logout")).toHaveLength(1))
})

it("says why the node or the Companion stopped", async () => {
  replies.companion_status = { ...signedIn({ net: { ...fx.netStopped, state: "failed", error: "companion-net-crashed" } }), status: fx.statusFailed }
  render(<CompanionSettingsSection />)
  expect(await screen.findByText("The Tailscale connection stopped unexpectedly, so the Companion was stopped.")).toBeInTheDocument()
  expect(screen.getByText("The Companion stopped unexpectedly.")).toBeInTheDocument()
  expect(screen.getByTestId("companion-status")).toHaveTextContent("Stopped with an error")
  expect(screen.getByRole("button", { name: "Enable" })).toBeInTheDocument()
})

it("says a node crash once, although both the Companion and the node report it", async () => {
  replies.companion_status = {
    ...signedIn({ net: { ...fx.netStopped, state: "failed", error: "companion-net-crashed" } }),
    status: { state: "failed", code: "companion-net-crashed" },
  }
  render(<CompanionSettingsSection />)
  await screen.findByText("The Tailscale connection stopped unexpectedly, so the Companion was stopped.")
  expect(screen.getAllByText("The Tailscale connection stopped unexpectedly, so the Companion was stopped.")).toHaveLength(1)
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

it("keeps the phone link closed while another action is in progress", async () => {
  replies.companion_status = fx.overviewRunning
  replies.companion_disable = () => new Promise(() => undefined)
  render(<CompanionSettingsSection />)
  const link = await screen.findByRole("button", { name: "Open on phone" })
  expect(link).toBeEnabled()
  fireEvent.click(screen.getByRole("button", { name: "Disable" }))
  await waitFor(() => expect(link).toBeDisabled())
})

it("closes the phone link dialog once the Companion stops running", async () => {
  replies.companion_status = fx.overviewRunning
  replies.companion_phone_link = fx.phoneLink
  Object.assign(URL, { createObjectURL: () => "blob:tauri://localhost/link", revokeObjectURL: () => undefined })
  render(<CompanionSettingsSection />)
  fireEvent.click(await screen.findByRole("button", { name: "Open on phone" }))
  await screen.findByTestId("companion-phone-link-dialog")
  act(() => useCompanionStore.getState().applyEvent({ type: "status", status: fx.statusStopping }))
  await waitFor(() => expect(screen.queryByTestId("companion-phone-link-dialog")).not.toBeInTheDocument())
})

it("maps an enable failure with a detail to its guidance", async () => {
  replies.companion_status = signedIn()
  replies.companion_enable = { reject: "companion-net-start-failed: No such file or directory" }
  render(<CompanionSettingsSection />)
  fireEvent.click(await screen.findByRole("button", { name: "Enable" }))
  fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Enable remote control" }))
  expect(await screen.findByText("The Tailscale connection could not start. (No such file or directory)")).toBeInTheDocument()
})

it("names the configured port when the enable failed because it was taken", async () => {
  replies.companion_status = on({ gatewayPort: 47999 }, { net: fx.netStopped })
  replies.companion_enable = { reject: "port-in-use" }
  render(<CompanionSettingsSection />)
  fireEvent.click(await screen.findByRole("button", { name: "Enable" }))
  fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Enable remote control" }))
  expect(await screen.findByText(/Another program is using port 47999, which the Companion needs/)).toBeInTheDocument()
  expect(screen.queryByText(/47110/)).not.toBeInTheDocument()
})

it("names the configured port of a failed status whose bind was refused", async () => {
  replies.companion_status = on({ gatewayPort: 47999 }, { net: fx.netStopped, status: { state: "failed", code: "port-in-use" } })
  render(<CompanionSettingsSection />)
  expect(await screen.findByText(/Another program is using port 47999, which the Companion needs/)).toBeInTheDocument()
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

it("shows which phones get notifications and when one last read Git", async () => {
  vi.useFakeTimers({ toFake: ["Date"], now: Date.parse("2026-10-20T12:00:00Z") })
  try {
    replies.companion_status = on({}, { push: fx.pushOverview })
    replies.companion_devices = [fx.device, { ...fx.device, deviceId: "BBBBBBBBBBBBBBBBBBBBBB", label: "iPad" }]
    render(<CompanionSettingsSection />)
    const rows = await screen.findAllByTestId("companion-device")
    const [iphone, ipad] = rows
    expect(within(iphone).getByText("Notifications on")).toBeInTheDocument()
    expect(within(ipad).queryByText("Notifications on")).not.toBeInTheDocument()
    expect(screen.queryByText(/Last read Git/)).not.toBeInTheDocument()

    act(() => useCompanionStore.getState().applyEvent({
      type: "audit", deviceId: fx.device.deviceId, peerId: 1, scope: "view", action: "git.read", session: "default", target: "w1",
      summary: { method: "git.status" }, at: "2026-10-20T11:55:00Z",
    }))
    expect(await within(iphone).findByText("Last read Git 5 minutes ago")).toBeInTheDocument()
    expect(within(ipad).queryByText(/Last read Git/)).not.toBeInTheDocument()

    // Turning notifications off elsewhere updates the badge.
    act(() => useCompanionStore.getState().applyEvent({ type: "push", push: { available: true, subscribed: [] } }))
    await waitFor(() => expect(within(iphone).queryByText("Notifications on")).not.toBeInTheDocument())
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

it("keeps the phone link and pairing off until the node serves the Companion, even on a freshly opened card", async () => {
  // The gateway is up (status running) but the node has not started serving it yet.
  replies.companion_status = { ...fx.overviewRunning, net: { ...fx.netRunning, serving: false } }
  render(<CompanionSettingsSection />)
  const link = await screen.findByRole("button", { name: "Open on phone" })
  expect(link).toBeDisabled()
  expect(screen.getByRole("button", { name: "Pair a phone" })).toBeDisabled()
  replies.companion_status = fx.overviewRunning
  act(() => useCompanionStore.getState().applyEvent({ type: "net", net: { ...fx.netRunning, serving: true } }))
  await waitFor(() => expect(screen.getByRole("button", { name: "Open on phone" })).toBeEnabled())
  expect(screen.getByRole("button", { name: "Pair a phone" })).toBeEnabled()
})
