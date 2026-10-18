import { fireEvent, render, screen } from "@testing-library/react"
import { beforeEach, expect, it, vi } from "vitest"
import { companionFixtures as fx, companionInvoke, type CompanionReplies } from "@/test/companionFixtures"

const mocks = vi.hoisted(() => ({ invoke: vi.fn(), writeText: vi.fn() }))
vi.mock("@/lib/ipc", async importOriginal => ({ ...(await importOriginal<typeof import("@/lib/ipc")>()), invoke: mocks.invoke }))
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: mocks.writeText }))
import { CompanionPhoneLinkDialog } from "./CompanionPhoneLinkDialog"

let replies: CompanionReplies
const createObjectURL = vi.fn((_blob: Blob) => "blob:tauri://localhost/link")
const revokeObjectURL = vi.fn()

beforeEach(() => {
  replies = { companion_phone_link: fx.phoneLink }
  mocks.invoke.mockReset().mockImplementation(companionInvoke(() => replies))
  mocks.writeText.mockReset().mockResolvedValue(undefined)
  createObjectURL.mockClear()
  revokeObjectURL.mockClear()
  Object.assign(URL, { createObjectURL, revokeObjectURL })
})

it("shows the address with its QR and copies it", async () => {
  const view = render(<CompanionPhoneLinkDialog onClose={vi.fn()} />)
  const image = await screen.findByRole("img", { name: "QR code for the Companion address" })
  expect(image).toHaveAttribute("src", "blob:tauri://localhost/link")
  expect(await createObjectURL.mock.calls[0][0].text()).toBe(fx.phoneLink.qrSvg)
  expect(screen.getByText(fx.phoneLink.url)).toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Copy address" }))
  expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument()
  expect(mocks.writeText).toHaveBeenCalledWith(fx.phoneLink.url)
  view.unmount()
  expect(revokeObjectURL).toHaveBeenCalledWith("blob:tauri://localhost/link")
})

it("explains when the Companion is not running", async () => {
  replies = { companion_phone_link: { reject: "companion-not-running" } }
  render(<CompanionPhoneLinkDialog onClose={vi.fn()} />)
  expect(await screen.findByRole("alert")).toHaveTextContent("Companion is not running.")
  expect(screen.queryByRole("img")).not.toBeInTheDocument()
})
