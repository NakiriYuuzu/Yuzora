import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { useCompanionStore } from "@/state/companionStore"
import { companionFixtures as fx, companionInvoke, type CompanionReplies } from "@/test/companionFixtures"

const ipc = vi.hoisted(() => ({ invoke: vi.fn() }))
vi.mock("@/lib/ipc", async importOriginal => ({ ...(await importOriginal<typeof import("@/lib/ipc")>()), invoke: ipc.invoke }))
import { CompanionPairingDialog } from "./CompanionPairingDialog"

let replies: CompanionReplies
const calls = (command: string) => ipc.invoke.mock.calls.filter(([name]) => name === command).map(([, args]) => args)
const createObjectURL = vi.fn((_blob: Blob) => "blob:tauri://localhost/qr")
const revokeObjectURL = vi.fn()

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date", "setInterval", "clearInterval"], now: Date.parse("2026-10-18T12:00:00Z") })
  replies = { companion_pairing_create: fx.pairingQr, companion_pairing_decide: undefined }
  ipc.invoke.mockReset().mockImplementation(companionInvoke(() => replies))
  createObjectURL.mockClear()
  revokeObjectURL.mockClear()
  Object.assign(URL, { createObjectURL, revokeObjectURL })
  useCompanionStore.setState({ claims: [], settled: {} })
})
afterEach(() => vi.useRealTimers())

it("renders the QR from a blob URL with a countdown, and cancels when closed", async () => {
  const onClose = vi.fn()
  const view = render(<CompanionPairingDialog onClose={onClose} />)
  const image = await screen.findByRole("img", { name: "Pairing QR code" })
  expect(image).toHaveAttribute("src", "blob:tauri://localhost/qr")
  expect(await createObjectURL.mock.calls[0][0].text()).toBe(fx.pairingQr.qrSvg)
  expect(screen.getByRole("timer")).toHaveTextContent("Expires in 5:00")
  expect(calls("companion_pairing_create")).toEqual([{ path: "tailscale" }])
  fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" })
  expect(onClose).toHaveBeenCalled()
  view.unmount()
  expect(revokeObjectURL).toHaveBeenCalledWith("blob:tauri://localhost/qr")
  await waitFor(() => expect(calls("companion_pairing_decide")).toEqual([{ pairingId: "pairing-1", decision: "cancel" }]))
})

it("hands a claimed pairing to the approval dialog without cancelling it", async () => {
  const onClose = vi.fn()
  const view = render(<CompanionPairingDialog onClose={onClose} />)
  await screen.findByRole("img", { name: "Pairing QR code" })
  act(() => useCompanionStore.getState().applyEvent({
    type: "pairing.claimed", pairingId: "pairing-1", deviceLabel: "iPhone", sas: "123456", via: "tailscale", tailscaleLogin: "user@example.com", userAgent: "Safari",
  }))
  expect(onClose).toHaveBeenCalled()
  view.unmount()
  await Promise.resolve()
  expect(calls("companion_pairing_decide")).toEqual([])
})

it("closes when the code expires", async () => {
  const onClose = vi.fn()
  render(<CompanionPairingDialog onClose={onClose} />)
  await screen.findByRole("img", { name: "Pairing QR code" })
  act(() => { vi.advanceTimersByTime(5 * 60_000) })
  expect(onClose).toHaveBeenCalled()
})

it("shows why a code could not be created", async () => {
  replies.companion_pairing_create = { reject: "companion-not-running" }
  render(<CompanionPairingDialog onClose={vi.fn()} />)
  expect(await screen.findByText("The Companion is not running.")).toBeInTheDocument()
})
