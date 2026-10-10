import { act, fireEvent } from "@testing-library/react"
import type { Root } from "react-dom/client"
import { expect, it, vi } from "vitest"

const { options, roots } = vi.hoisted(() => ({
  options: [] as { minimumContrastRatio: number; theme: { black: string }; darkAtConstruction: boolean }[],
  roots: [] as Root[],
}))
vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    constructor(value: { minimumContrastRatio: number; theme: { black: string } }) {
      options.push({ ...value, darkAtConstruction: document.documentElement.classList.contains("dark") })
    }
    open() {}
    write() {}
    dispose() {}
  },
}))
vi.mock("react-dom/client", async (importOriginal) => {
  const original = await importOriginal<typeof import("react-dom/client")>()
  return { ...original, createRoot: (...args: Parameters<typeof original.createRoot>) => {
    const root = original.createRoot(...args)
    roots.push(root)
    return root
  } }
})

it("constructs all fixture terminals with the displayed theme on mount and repeated toggles", async () => {
  const originalDark = document.documentElement.classList.contains("dark")
  document.documentElement.classList.remove("dark")
  document.body.innerHTML = '<div id="root"></div>'
  try {
    await act(async () => { await import("./xterm-block-glyphs") })
    for (let index = 0; index < 5; index++) {
      const dark = index % 2 === 0
      expect(document.querySelector("h1")?.textContent).toBe(`xterm block glyphs (${dark ? "dark" : "light"})`)
      expect(options).toHaveLength((index + 1) * 8)
      for (const option of options.slice(-8)) {
        expect(option.darkAtConstruction).toBe(dark)
        expect(option.minimumContrastRatio).toBe(dark ? 1 : 3)
        expect(option.theme.black).toBe(dark ? "#0f0e13" : "#5c5a55")
      }
      if (index < 4) fireEvent.click(document.querySelector("button")!)
    }
  } finally {
    act(() => { for (const root of roots) root.unmount() })
    document.documentElement.classList.toggle("dark", originalDark)
    document.body.replaceChildren()
  }
})
