import { render } from "@testing-library/react"
import { I18nextProvider } from "react-i18next"
import { expect, it } from "vitest"
import { i18n } from "../i18n"
import { parseUnifiedDiff } from "../lib/diff"
import { DiffText } from "./DiffText"

const text = ["diff --git a/a.ts b/a.ts", "--- a/a.ts", "+++ b/a.ts", "@@ -1,1 +1,1 @@", "-old", "+new", ""].join("\n")

it("scrolls wide hunks in a ScrollArea rather than a bare overflow", () => {
  const { container } = render(<I18nextProvider i18n={i18n}><DiffText files={parseUnifiedDiff(text)} /></I18nextProvider>)
  const area = container.querySelector('[data-slot="scroll-area"]')
  expect(area).not.toBeNull()
  expect(area).toHaveTextContent("@@ -1,1 +1,1 @@")
  expect(container.querySelector(".overflow-x-auto")).toBeNull()
})
