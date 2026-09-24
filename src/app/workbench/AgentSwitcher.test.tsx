import { afterEach, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import "@/lib/i18n"
import { AgentSwitcher, type AgentSwitcherItem } from "./AgentSwitcher"

afterEach(cleanup)

const items: AgentSwitcherItem[] = [
  { key: "a", title: "Pi", subtitle: "main · default", status: "idle", statusLabel: "Idle", logoKind: "pi", logoLabel: "pi" },
  { key: "b", title: "Codex", subtitle: "feature · default", status: "working", statusLabel: "Working", logoKind: "codex", logoLabel: "codex" },
]

it("renders a listbox with the highlighted Agent announced, without taking focus", () => {
  const focused = document.body.appendChild(document.createElement("textarea"))
  focused.focus()
  render(<AgentSwitcher items={items} index={1} holdLabel="Alt" onCommit={vi.fn()} onHighlight={vi.fn()} />)
  const options = screen.getAllByRole("option")
  expect(options).toHaveLength(2)
  expect(options[1]).toHaveAttribute("aria-selected", "true")
  expect(screen.getByRole("listbox")).toHaveAttribute("aria-activedescendant", "agent-switcher-1")
  expect(document.activeElement).toBe(focused)
  expect(screen.getByText("Codex", { selector: ".sr-only" })).toBeInTheDocument()
  focused.remove()
})

it("highlights on hover and commits on click without stealing focus", () => {
  const onCommit = vi.fn(), onHighlight = vi.fn()
  render(<AgentSwitcher items={items} index={1} holdLabel="Alt" onCommit={onCommit} onHighlight={onHighlight} />)
  const first = screen.getAllByRole("option")[0]
  fireEvent.mouseEnter(first)
  expect(onHighlight).toHaveBeenCalledWith(0)
  const press = new MouseEvent("mousedown", { bubbles: true, cancelable: true })
  first.dispatchEvent(press)
  expect(press.defaultPrevented).toBe(true)
  fireEvent.click(first)
  expect(onCommit).toHaveBeenCalledWith(0)
})
