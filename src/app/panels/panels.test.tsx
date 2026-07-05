import { afterEach, describe, expect, it } from "vitest"
import { fireEvent, render, screen, within } from "@testing-library/react"

import { AppShell } from "@/app/AppShell"
import { AgentZonePanel } from "@/app/panels/AgentZonePanel"
import { GitPanel } from "@/app/panels/GitPanel"
import { PreviewPanel } from "@/app/panels/PreviewPanel"
import { GitNavContent } from "@/app/workbench/GitNavContent"
import { useContextMenuStore } from "@/state/contextMenuStore"
import { initialGitState, useGitStore } from "@/state/gitStore"

afterEach(() => {
  useContextMenuStore.setState({ kind: null, x: 0, y: 0, payload: {} })
  // gitStore persists across the module graph; environment set in one test
  // leaks into the next (e.g. the "No repository status" nav assertion relies
  // on a null environment). Reset to the initial snapshot after each test.
  useGitStore.setState(initialGitState)
})

// Covers the Git/Database/SSH/Agent mode entry states (Task E2) plus the
// Settings dialog content. The mode switcher tablist is named
// "Workbench mode" (ProjectNavPanel) so it can be scoped precisely — some
// mode panels have their own internal tabs (e.g. SSH's "SSH" segment vs.
// the rail's "SSH" mode tab) that would otherwise collide on an unscoped
// getByRole("tab", { name: ... }) query.
function switchMode(name: string) {
  const modeSwitcher = screen.getByRole("tablist", { name: "Workbench mode" })
  fireEvent.click(within(modeSwitcher).getByRole("tab", { name }))
}

describe("Git/Database/SSH/Agent mode entry states", () => {
  it("shows the git nav and enables all three git view tabs", () => {
    render(<AppShell />)
    switchMode("Git")

    const nav = screen.getByTestId("nav-mode-content-git")
    expect(within(nav).getByText("No repository status")).toBeInTheDocument()

    // Log (default), Local changes and Console are all live now.
    const gitViews = screen.getByRole("tablist", { name: "Git views" })
    expect(within(gitViews).getByRole("tab", { name: /Log/ })).not.toBeDisabled()
    expect(within(gitViews).getByRole("tab", { name: "Local changes" })).not.toBeDisabled()
    expect(within(gitViews).getByRole("tab", { name: "Console" })).not.toBeDisabled()
  })

  it("shows the database nav and main entry states", () => {
    render(<AppShell />)
    switchMode("Database")

    const nav = screen.getByTestId("nav-mode-content-database")
    expect(within(nav).getByText("No database connections")).toBeInTheDocument()
    expect(within(nav).getByRole("button", { name: "New connection" })).toBeInTheDocument()
    expect(screen.getByText("Database connections are not configured")).toBeInTheDocument()
  })

  it("shows the ssh nav and main entry states, and switches the SFTP/SSH tabs", () => {
    render(<AppShell />)
    switchMode("SSH")

    const nav = screen.getByTestId("nav-mode-content-ssh")
    expect(within(nav).getByText("No hosts yet")).toBeInTheDocument()
    expect(within(nav).getByRole("button", { name: "New host" })).toBeInTheDocument()

    expect(screen.getAllByText("Remote sessions are not configured").length).toBeGreaterThan(0)
    expect(screen.getByText("Connect a host to transfer files here.")).toBeInTheDocument()

    // Radix's Tabs.Trigger switches on mousedown (not click) — see
    // @radix-ui/react-tabs's Trigger, which wires activation to onMouseDown
    // (plus onKeyDown/onFocus). fireEvent.click alone never fires that
    // handler, so tab-switching assertions use mouseDown here and below.
    const viewSwitcher = screen.getByRole("tablist", { name: "SFTP or SSH" })
    fireEvent.mouseDown(within(viewSwitcher).getByRole("tab", { name: "SSH" }))

    expect(screen.getByText("Connect a host to open a terminal session here.")).toBeInTheDocument()
    expect(screen.queryByText("Connect a host to transfer files here.")).not.toBeInTheDocument()
  })

  it("shows the agent nav and main entry states", () => {
    render(<AppShell />)
    switchMode("AgentZone")

    const nav = screen.getByTestId("nav-mode-content-agent")
    expect(within(nav).getByText("No sessions yet")).toBeInTheDocument()
    expect(within(nav).getByRole("button", { name: "New session" })).toBeInTheDocument()
    expect(screen.getByText("ACP sessions will be managed here")).toBeInTheDocument()
  })
})

describe("Settings dialog content", () => {
  it("has the design nav sections; the language-server list no longer lives under Editor", async () => {
    render(<AppShell />)
    fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    const dialog = await screen.findByRole("dialog")

    expect(within(dialog).getByRole("button", { name: "Appearance" })).toBeInTheDocument()
    expect(within(dialog).getByRole("button", { name: "Editor" })).toBeInTheDocument()
    expect(within(dialog).getByRole("button", { name: "Safety" })).toBeInTheDocument()

    fireEvent.click(within(dialog).getByRole("button", { name: "Editor" }))

    // The placeholder language-server list + fake format-on-save moved to the
    // live LSP pane (T12b-2); the editor pane keeps its own surface toggle.
    expect(within(dialog).getByRole("switch", { name: "Show minimap" })).toBeInTheDocument()
    expect(within(dialog).queryByText("TypeScript/JavaScript")).toBeNull()
    expect(within(dialog).queryByText("Not installed")).toBeNull()
  })

  it("switches the document theme from the Appearance tab", async () => {
    render(<AppShell />)
    fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    const dialog = await screen.findByRole("dialog")

    expect(document.documentElement).not.toHaveClass("dark")

    fireEvent.click(within(dialog).getByRole("radio", { name: "Dark" }))
    expect(document.documentElement).toHaveClass("dark")

    fireEvent.click(within(dialog).getByRole("radio", { name: "Light" }))
    expect(document.documentElement).not.toHaveClass("dark")
  })

  it("git section shows detection state and remote-check controls", async () => {
    useGitStore.setState({
      environment: { status: "ready", root: "/w", version: "2.50.1" },
      remoteCheck: { mode: "probe", intervalSec: 180 },
    })
    render(<AppShell />)
    fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    const dialog = await screen.findByRole("dialog")

    fireEvent.click(within(dialog).getByRole("button", { name: /^Git/ }))
    expect(within(dialog).getByText(/2\.50\.1/)).toBeInTheDocument()
    expect(within(dialog).getByRole("button", { name: "唯讀檢查" })).toHaveAttribute(
      "aria-pressed",
      "true"
    )

    fireEvent.click(within(dialog).getByRole("button", { name: "自動 fetch" }))
    expect(useGitStore.getState().remoteCheck.mode).toBe("autofetch")
  })

  it("remote-check control uses role=group (aria-pressed buttons), not radiogroup (T19)", async () => {
    useGitStore.setState({ environment: { status: "ready", root: "/w", version: "2.50.1" } })
    render(<AppShell />)
    fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    const dialog = await screen.findByRole("dialog")
    fireEvent.click(within(dialog).getByRole("button", { name: /^Git/ }))

    expect(within(dialog).getByRole("group", { name: "遠端檢查" })).toBeInTheDocument()
    expect(
      within(dialog).queryByRole("radiogroup", { name: "遠端檢查" })
    ).not.toBeInTheDocument()
  })

  it("clamps the remote-check interval on blur and allows intermediate typing (T19)", async () => {
    useGitStore.setState({
      environment: { status: "ready", root: "/w", version: "2.50.1" },
      remoteCheck: { mode: "probe", intervalSec: 180 },
    })
    render(<AppShell />)
    fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    const dialog = await screen.findByRole("dialog")
    fireEvent.click(within(dialog).getByRole("button", { name: /^Git/ }))

    const input = within(dialog).getByRole("spinbutton") as HTMLInputElement
    // A sub-minimum keystroke is accepted while typing (no immediate rejection)…
    fireEvent.change(input, { target: { value: "4" } })
    expect(input.value).toBe("4")
    expect(useGitStore.getState().remoteCheck.intervalSec).toBe(180)
    // …and only clamps + commits on blur.
    fireEvent.blur(input)
    expect(useGitStore.getState().remoteCheck.intervalSec).toBe(30)
  })
})

describe("Git guided setup", () => {
  it("git panel shows guided setup when git missing", () => {
    useGitStore.setState({ environment: { status: "missing", reason: "git not found" } })
    render(<GitPanel />)
    expect(screen.getByText("未偵測到 Git")).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "重新偵測" })).toBeInTheDocument()
  })

  it("git nav shows guided setup when git missing", () => {
    useGitStore.setState({ environment: { status: "missing", reason: "git not found" } })
    render(<GitNavContent />)
    expect(screen.getByText("未偵測到 Git")).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "重新偵測" })).toBeInTheDocument()
  })
})

it("右鍵 preview 面板被完全吃掉：不彈選單、default 被擋", () => {
  render(<PreviewPanel />)
  const nativeMenuShown = fireEvent.contextMenu(screen.getByText("No dev server"))
  expect(nativeMenuShown).toBe(false)
  expect(useContextMenuStore.getState().kind).toBeNull()
})

it("右鍵 Git 面板開啟 git 選單", () => {
  render(<GitPanel />)
  // Default tab is Log; its details panel always shows this prompt.
  fireEvent.contextMenu(screen.getByText("Select a commit to view details"))
  expect(useContextMenuStore.getState().kind).toBe("git")
})

it("右鍵 Agent 面板開啟 agent 選單", () => {
  render(<AgentZonePanel />)
  fireEvent.contextMenu(screen.getByText("ACP sessions will be managed here"))
  expect(useContextMenuStore.getState().kind).toBe("agent")
})
