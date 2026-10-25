import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { beforeEach, expect, it, vi } from "vitest"
import type { CompanionClaim } from "@/lib/companionIpc"
import { useCompanionStore } from "@/state/companionStore"
import { companionInvoke, type CompanionReplies } from "@/test/companionFixtures"

const ipc = vi.hoisted(() => ({ invoke: vi.fn() }))
vi.mock("@/lib/ipc", async importOriginal => ({ ...(await importOriginal<typeof import("@/lib/ipc")>()), invoke: ipc.invoke }))
import { CompanionApprovalHost } from "./CompanionApprovalHost"

let replies: CompanionReplies
const calls = (command: string) => ipc.invoke.mock.calls.filter(([name]) => name === command).map(([, args]) => args)
const claim: CompanionClaim = {
  pairingId: "pairing-1", deviceLabel: "iPhone", sas: "482913", via: "tailscale", tailscaleLogin: "user@example.com", userAgent: "Mozilla/5.0 (iPhone) Safari",
}
const claimed = (item: CompanionClaim) => act(() => useCompanionStore.getState().applyEvent({ type: "pairing.claimed", ...item }))

beforeEach(() => {
  replies = { companion_pairing_decide: undefined }
  ipc.invoke.mockReset().mockImplementation(companionInvoke(() => replies))
  useCompanionStore.setState({ claims: [], settled: {} })
})

it("shows the code and who claimed it, with Reject focused", async () => {
  render(<CompanionApprovalHost />)
  expect(screen.queryByTestId("companion-approval-dialog")).not.toBeInTheDocument()
  claimed(claim)
  const dialog = await screen.findByTestId("companion-approval-dialog")
  expect(within(dialog).getByTestId("companion-approval-sas")).toHaveTextContent("482913")
  expect(within(dialog).getByText("iPhone")).toBeInTheDocument()
  expect(within(dialog).getByText("user@example.com")).toBeInTheDocument()
  expect(within(dialog).getByText("Mozilla/5.0 (iPhone) Safari")).toBeInTheDocument()
  await waitFor(() => expect(within(dialog).getByRole("button", { name: "Reject" })).toHaveFocus())
})

it("approves only from the Approve button, then shows the next claim", async () => {
  render(<CompanionApprovalHost />)
  claimed(claim)
  claimed({ ...claim, pairingId: "pairing-2", sas: "111111" })
  fireEvent.click(await screen.findByRole("button", { name: "Approve" }))
  await waitFor(() => expect(calls("companion_pairing_decide")).toEqual([{ pairingId: "pairing-1", decision: "approve" }]))
  expect(await screen.findByText("111111")).toBeInTheDocument()
})

it("rejects when dismissed", async () => {
  render(<CompanionApprovalHost />)
  claimed(claim)
  await screen.findByTestId("companion-approval-dialog")
  fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" })
  await waitFor(() => expect(calls("companion_pairing_decide")).toEqual([{ pairingId: "pairing-1", decision: "reject" }]))
  await waitFor(() => expect(screen.queryByTestId("companion-approval-dialog")).not.toBeInTheDocument())
})

it("keeps the claim and explains a failed approval", async () => {
  replies.companion_pairing_decide = { reject: "companion-timeout" }
  render(<CompanionApprovalHost />)
  claimed(claim)
  fireEvent.click(await screen.findByRole("button", { name: "Approve" }))
  expect(await screen.findByText("The Companion did not reply in time and was stopped.")).toBeInTheDocument()
  expect(screen.getByTestId("companion-approval-sas")).toHaveTextContent("482913")
})

it("closes when the gateway settles the pairing first", async () => {
  render(<CompanionApprovalHost />)
  claimed(claim)
  await screen.findByTestId("companion-approval-dialog")
  act(() => useCompanionStore.getState().applyEvent({ type: "pairing.state", pairingId: "pairing-1", state: "expired", reason: "ttl" }))
  await waitFor(() => expect(screen.queryByTestId("companion-approval-dialog")).not.toBeInTheDocument())
  expect(calls("companion_pairing_decide")).toEqual([])
})
