import { fireEvent, render, screen } from "@testing-library/react"
import { beforeEach, expect, it } from "vitest"

import { requestAppConfirmation, showAppMessage, useAppDialogStore } from "@/state/appDialogStore"
import { AppDialogHost } from "@/workbench/AppDialogHost"

beforeEach(() => {
  useAppDialogStore.setState({ pending: null })
})

it("renders confirmations with app-owned UI and resolves cancel", async () => {
  render(<AppDialogHost />)
  const result = requestAppConfirmation({
    title: "Close Herdr pane",
    description: "Running processes will end.",
    destructive: true
  })

  expect(await screen.findByRole("alertdialog")).toBeInTheDocument()
  expect(screen.getByText("Close Herdr pane")).toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
  await expect(result).resolves.toBe(false)
})

it("renders message feedback with a single app-owned action", async () => {
  render(<AppDialogHost />)
  const result = showAppMessage({
    title: "Action failed",
    description: "Runtime refused the action.",
    kind: "error"
  })

  expect(await screen.findByText("Runtime refused the action.")).toBeInTheDocument()
  expect(screen.queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument()
  fireEvent.click(screen.getByRole("button", { name: "OK" }))
  await expect(result).resolves.toBeUndefined()
})

it("stays compact and non-resizable while long runtime errors remain readable", async () => {
  render(<AppDialogHost />)
  const longError = Array.from({ length: 80 }, (_, i) =>
    `os error line ${i + 1}: ENOENT /very/long/path/to/runtime/asset-${i + 1}.bin`,
  ).join("\n")

  void showAppMessage({
    title: "Action failed",
    description: longError,
    kind: "error",
  })

  const content = await screen.findByTestId("app-dialog-content")
  expect(content).toHaveAttribute("data-slot", "alert-dialog-content")
  expect(content).not.toHaveAttribute("data-dialog-size-id")
  expect(content.style.width).toBe("")
  expect(content.style.height).toBe("")
  expect(content.className).toMatch(/sm:max-w-\[420px\]/)
  expect(
    content.querySelectorAll('[data-slot="dialog-resize-handle"]').length,
  ).toBe(0)

  const body = screen.getByTestId("app-dialog-body")
  expect(body).toHaveAttribute("data-slot", "scroll-area")
  expect(body.className).toMatch(/max-h-\[40vh\]/)
  expect(screen.getByText(/os error line 1:/)).toBeInTheDocument()
  expect(screen.getByText(/os error line 80:/)).toBeInTheDocument()
  expect(screen.getByRole("button", { name: "OK" })).toBeInTheDocument()
})

it("wraps long unbroken runtime/OS error tokens without becoming resizable", async () => {
  render(<AppDialogHost />)
  // Wider than the compact sm:max-w-[420px] surface and without soft break points.
  const unbrokenToken =
    "ENOENT:" + "/very/long/runtime/path/segment".repeat(40) + "/asset.bin"

  void showAppMessage({
    title: "Action failed",
    description: unbrokenToken,
    kind: "error",
  })

  const content = await screen.findByTestId("app-dialog-content")
  expect(content).not.toHaveAttribute("data-dialog-size-id")
  expect(content.style.width).toBe("")
  expect(content.style.height).toBe("")
  expect(
    content.querySelectorAll('[data-slot="dialog-resize-handle"]').length,
  ).toBe(0)

  const description = screen.getByText(unbrokenToken)
  expect(description.className).toMatch(/break-words/)
  expect(description.className).toMatch(/\[overflow-wrap:anywhere\]/)
  expect(description.className).toMatch(/whitespace-pre-wrap/)
  expect(screen.getByRole("button", { name: "OK" })).toBeInTheDocument()
})
