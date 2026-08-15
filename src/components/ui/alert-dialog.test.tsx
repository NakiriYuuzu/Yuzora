import { cleanup, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"

afterEach(() => {
  cleanup()
})

describe("AlertDialogContent compact contract", () => {
  it("is content-sized, non-resizable, and has no dialog size handles", () => {
    render(
      <AlertDialog open>
        <AlertDialogContent data-testid="alert-content">
          <AlertDialogHeader>
            <AlertDialogTitle>Confirm</AlertDialogTitle>
            <AlertDialogDescription>Short alert body</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction>OK</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>,
    )

    const content = screen.getByTestId("alert-content")
    expect(content).toHaveAttribute("data-slot", "alert-dialog-content")
    expect(content).not.toHaveAttribute("data-dialog-size-id")
    expect(content).not.toHaveAttribute("data-resizing")
    expect(content.style.width).toBe("")
    expect(content.style.height).toBe("")
    expect(content.className).toMatch(/sm:max-w-\[420px\]/)
    expect(content.className).not.toMatch(/max-w-none/)
    expect(
      content.querySelectorAll('[data-slot="dialog-resize-handle"]').length,
    ).toBe(0)
    expect(screen.getByRole("alertdialog")).toBeInTheDocument()
  })
})
