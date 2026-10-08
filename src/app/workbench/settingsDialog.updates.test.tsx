import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"

const { check, getVersion, relaunch, updateHerdrProcesses, updateStopHerdr } = vi.hoisted(() => ({
  check: vi.fn(),
  getVersion: vi.fn(async () => "0.0.3"),
  relaunch: vi.fn(async () => undefined),
  updateHerdrProcesses: vi.fn(),
  updateStopHerdr: vi.fn(),
}))

vi.mock("@tauri-apps/api/app", () => ({ getVersion }))
vi.mock("@tauri-apps/plugin-updater", () => ({ check }))
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch }))
vi.mock("@/lib/updateChannel", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/updateChannel")>()),
  updateHerdrProcesses,
  updateStopHerdr,
}))
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}))

import { SettingsDialog } from "@/app/workbench/SettingsDialog"
import { useUpdateStore } from "@/state/updateStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

function installLocalStorage(): void {
  const store = new Map<string, string>()
  const mock = {
    getItem: (key: string) => (store.has(key) ? store.get(key)! : null),
    setItem: (key: string, value: string) => void store.set(key, String(value)),
    removeItem: (key: string) => void store.delete(key),
    clear: () => store.clear(),
    key: (index: number) => [...store.keys()][index] ?? null,
    get length() {
      return store.size
    },
  }
  Object.defineProperty(globalThis, "localStorage", {
    value: mock,
    configurable: true,
    writable: true,
  })
}

const originalUserAgent = window.navigator.userAgent

function setUserAgent(value: string): void {
  Object.defineProperty(window.navigator, "userAgent", {
    configurable: true,
    value,
  })
}

afterEach(() => {
  setUserAgent(originalUserAgent)
})

beforeEach(() => {
  cleanup()
  vi.clearAllMocks()
  check.mockResolvedValue(null)
  updateHerdrProcesses.mockResolvedValue({
    path: "C:\\Program Files\\Yuzora\\herdr\\windows-x86_64\\herdr.exe",
    version: "0.9.3",
    pids: [101, 202],
  })
  updateStopHerdr.mockResolvedValue(undefined)
  getVersion.mockResolvedValue("0.0.3")
  relaunch.mockResolvedValue(undefined)
  useUpdateStore.getState().reset()
  useWorkspaceStore.setState({
    workspacePath: null,
    groups: [{ tabs: [], activePath: null }],
    activeGroupIndex: 0,
    pendingReveal: null,
  })
  installLocalStorage()
})

describe("Settings · About & Updates pane", () => {
  it("shows the runtime App version in the pane and sidebar footer", async () => {
    render(
      <SettingsDialog
        open
        onOpenChange={() => {}}
        theme="light"
        onThemeChange={() => {}}
        initialSection="about"
      />,
    )

    expect(await screen.findByRole("heading", { name: "About & Updates" })).toBeInTheDocument()
    expect(await screen.findAllByText("Yuzora v0.0.3")).toHaveLength(2)
  })

  it("shows readable release notes for the version currently in use", async () => {
    render(
      <SettingsDialog
        open
        onOpenChange={() => {}}
        theme="light"
        onThemeChange={() => {}}
        initialSection="about"
      />,
    )

    expect(await screen.findByText("What's new in this version")).toBeInTheDocument()
    expect(
      await screen.findByText("「關於與更新」現在會列出目前版本及可用更新帶來的主要改變。")
    ).toBeInTheDocument()
  })

  it("checks manually and reports when the current version is up to date", async () => {
    render(
      <SettingsDialog
        open
        onOpenChange={() => {}}
        theme="light"
        onThemeChange={() => {}}
        initialSection="about"
      />,
    )

    fireEvent.click(await screen.findByRole("button", { name: "Check for updates" }))

    expect(await screen.findByText("You're up to date")).toBeInTheDocument()
    expect(check).toHaveBeenCalledTimes(1)
  })

  it("shows the stable update version when one is available", async () => {
    check.mockResolvedValue({ version: "0.0.4" })

    render(
      <SettingsDialog
        open
        onOpenChange={() => {}}
        theme="light"
        onThemeChange={() => {}}
        initialSection="about"
      />,
    )

    fireEvent.click(await screen.findByRole("button", { name: "Check for updates" }))

    expect(await screen.findByText("Yuzora v0.0.4 is available")).toBeInTheDocument()
    expect(check).toHaveBeenCalledTimes(1)
  })

  it("shows readable release notes for an available update", async () => {
    check.mockResolvedValue({
      version: "0.0.4",
      body: "### 改善\n\n- 更新後會清楚顯示這個版本帶來的改變。",
    })

    render(
      <SettingsDialog
        open
        onOpenChange={() => {}}
        theme="light"
        onThemeChange={() => {}}
        initialSection="about"
      />,
    )

    fireEvent.click(await screen.findByRole("button", { name: "Check for updates" }))

    expect(await screen.findByText("What's new in Yuzora v0.0.4")).toBeInTheDocument()
    expect(
      screen.getByText("更新後會清楚顯示這個版本帶來的改變。")
    ).toBeInTheDocument()
  })

  it("shows a sanitized failure and retries from Settings", async () => {
    check.mockRejectedValueOnce(new Error("https://updates.invalid/?token=do-not-render"))

    render(
      <SettingsDialog
        open
        onOpenChange={() => {}}
        theme="light"
        onThemeChange={() => {}}
        initialSection="about"
      />,
    )

    fireEvent.click(await screen.findByRole("button", { name: "Check for updates" }))

    expect(await screen.findByText("Couldn't check for updates")).toBeInTheDocument()
    expect(screen.queryByText(/do-not-render/)).not.toBeInTheDocument()

    check.mockResolvedValueOnce(null)
    fireEvent.click(screen.getByRole("button", { name: "Retry" }))

    expect(await screen.findByText("You're up to date")).toBeInTheDocument()
    expect(check).toHaveBeenCalledTimes(2)
  })

  it("downloads only after explicit consent, shows progress, and waits before installing", async () => {
    let finishDownload!: () => void
    const install = vi.fn(async () => undefined)
    const download = vi.fn(
      async (onEvent?: (event: unknown) => void) => {
        onEvent?.({ event: "Started", data: { contentLength: 100 } })
        onEvent?.({ event: "Progress", data: { chunkLength: 25 } })
        await new Promise<void>((resolve) => {
          finishDownload = resolve
        })
        onEvent?.({ event: "Progress", data: { chunkLength: 75 } })
        onEvent?.({ event: "Finished" })
      }
    )
    check.mockResolvedValue({ version: "0.0.4", download, install })

    render(
      <SettingsDialog
        open
        onOpenChange={() => {}}
        theme="light"
        onThemeChange={() => {}}
        initialSection="about"
      />,
    )

    fireEvent.click(await screen.findByRole("button", { name: "Check for updates" }))
    expect(await screen.findByText("Yuzora v0.0.4 is available")).toBeInTheDocument()
    expect(download).not.toHaveBeenCalled()

    fireEvent.click(screen.getByRole("button", { name: "Download update" }))
    expect(await screen.findByText("Downloading update… 25%")).toBeInTheDocument()

    finishDownload()
    expect(await screen.findByText("Download complete")).toBeInTheDocument()
    expect(install).not.toHaveBeenCalled()
    expect(relaunch).not.toHaveBeenCalled()
  })

  it("keeps the discovered update and retries a failed download", async () => {
    const install = vi.fn(async () => undefined)
    const download = vi
      .fn()
      .mockRejectedValueOnce(new Error("transient download failure"))
      .mockImplementationOnce(async (onEvent?: (event: unknown) => void) => {
        onEvent?.({ event: "Started", data: { contentLength: 10 } })
        onEvent?.({ event: "Progress", data: { chunkLength: 10 } })
        onEvent?.({ event: "Finished" })
      })
    check.mockResolvedValue({ version: "0.0.4", download, install })

    render(
      <SettingsDialog
        open
        onOpenChange={() => {}}
        theme="light"
        onThemeChange={() => {}}
        initialSection="about"
      />,
    )

    fireEvent.click(await screen.findByRole("button", { name: "Check for updates" }))
    fireEvent.click(await screen.findByRole("button", { name: "Download update" }))

    expect(await screen.findByText("Couldn't download the update")).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Retry download" }))

    expect(await screen.findByText("Download complete")).toBeInTheDocument()
    expect(download).toHaveBeenCalledTimes(2)
    expect(check).toHaveBeenCalledTimes(1)
  })

  it("blocks installation while any document has unsaved changes", async () => {
    const install = vi.fn(async () => undefined)
    const download = vi.fn(async (onEvent?: (event: unknown) => void) => {
      onEvent?.({ event: "Finished" })
    })
    check.mockResolvedValue({ version: "0.0.4", download, install })
    useWorkspaceStore.setState({
      groups: [
        {
          tabs: [
            {
              path: "/tmp/dirty.ts",
              name: "dirty.ts",
              dirty: true,
              externallyModified: false,
            },
          ],
          activePath: "/tmp/dirty.ts",
        },
      ],
    })

    render(
      <SettingsDialog
        open
        onOpenChange={() => {}}
        theme="light"
        onThemeChange={() => {}}
        initialSection="about"
      />,
    )

    fireEvent.click(await screen.findByRole("button", { name: "Check for updates" }))
    fireEvent.click(await screen.findByRole("button", { name: "Download update" }))
    fireEvent.click(await screen.findByRole("button", { name: "Install and restart" }))

    expect(
      await screen.findByText(
        "Save or discard all unsaved documents before installing the update."
      )
    ).toBeInTheDocument()
    expect(install).not.toHaveBeenCalled()
    expect(relaunch).not.toHaveBeenCalled()
  })

  it("requires confirmation, supports cancel, then installs before relaunching", async () => {
    let finishInstall!: () => void
    const install = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          finishInstall = resolve
        })
    )
    const download = vi.fn(async (onEvent?: (event: unknown) => void) => {
      onEvent?.({ event: "Finished" })
    })
    check.mockResolvedValue({ version: "0.0.4", download, install })

    render(
      <SettingsDialog
        open
        onOpenChange={() => {}}
        theme="light"
        onThemeChange={() => {}}
        initialSection="about"
      />,
    )

    fireEvent.click(await screen.findByRole("button", { name: "Check for updates" }))
    fireEvent.click(await screen.findByRole("button", { name: "Download update" }))
    const installButton = await screen.findByRole("button", { name: "Install and restart" })
    fireEvent.click(installButton)

    let confirmation = await screen.findByRole("alertdialog", {
      name: "Install update and restart?",
    })
    expect(
      within(confirmation).getByText(
        "Installing the update will end running Terminal, Agent, SSH, and Preview work."
      )
    ).toBeInTheDocument()
    fireEvent.click(within(confirmation).getByRole("button", { name: "Cancel" }))

    await waitFor(() =>
      expect(
        screen.queryByRole("alertdialog", { name: "Install update and restart?" })
      ).not.toBeInTheDocument()
    )
    expect(install).not.toHaveBeenCalled()
    expect(relaunch).not.toHaveBeenCalled()

    fireEvent.click(installButton)
    confirmation = await screen.findByRole("alertdialog", {
      name: "Install update and restart?",
    })
    fireEvent.click(
      within(confirmation).getByRole("button", { name: "Install and restart" })
    )

    expect(
      await screen.findByRole("button", { name: "Installing update…" })
    ).toBeDisabled()
    expect(install).toHaveBeenCalledTimes(1)
    expect(relaunch).not.toHaveBeenCalled()

    finishInstall()
    await waitFor(() => expect(relaunch).toHaveBeenCalledTimes(1))
  })

  it("does not stop HERDR outside Windows", async () => {
    setUserAgent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)")
    const install = vi.fn(async () => undefined)
    const download = vi.fn(async (onEvent?: (event: unknown) => void) => {
      onEvent?.({ event: "Finished" })
    })
    check.mockResolvedValue({ version: "0.0.4", download, install })

    await openInstallConfirmation()
    const confirmation = await screen.findByRole("alertdialog", {
      name: "Install update and restart?",
    })
    expect(within(confirmation).queryByText(/force-stops the HERDR in use/)).not.toBeInTheDocument()
    fireEvent.click(within(confirmation).getByRole("button", { name: "Install and restart" }))

    await waitFor(() => expect(install).toHaveBeenCalledTimes(1))
    expect(updateHerdrProcesses).not.toHaveBeenCalled()
    expect(updateStopHerdr).not.toHaveBeenCalled()
  })

  it("on Windows, warns about lost progress, shows the HERDR in use, and stops it before installing", async () => {
    setUserAgent("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
    const order: string[] = []
    updateStopHerdr.mockImplementation(async () => {
      order.push("stop")
    })
    const install = vi.fn(async () => {
      order.push("install")
    })
    const download = vi.fn(async (onEvent?: (event: unknown) => void) => {
      onEvent?.({ event: "Finished" })
    })
    check.mockResolvedValue({ version: "0.0.4", download, install })

    await openInstallConfirmation()
    const confirmation = await screen.findByRole("alertdialog", {
      name: "Install update and restart?",
    })
    expect(
      within(confirmation).getByText(
        "The Windows installer can't replace a running HERDR. Continuing force-stops the HERDR in use and every Session it runs — running commands, Agents, and terminal progress will be lost and can't be recovered.",
      ),
    ).toBeInTheDocument()
    expect(await within(confirmation).findByText("0.9.3")).toBeInTheDocument()
    expect(
      within(confirmation).getByText("C:\\Program Files\\Yuzora\\herdr\\windows-x86_64\\herdr.exe"),
    ).toBeInTheDocument()
    expect(within(confirmation).getByText("2")).toBeInTheDocument()
    expect(updateStopHerdr).not.toHaveBeenCalled()

    fireEvent.click(within(confirmation).getByRole("button", { name: "Stop HERDR and install" }))

    await waitFor(() => expect(install).toHaveBeenCalledTimes(1))
    expect(order).toEqual(["stop", "install"])
    await waitFor(() => expect(relaunch).toHaveBeenCalledTimes(1))
  })

  it("on Windows, lists a user-installed HERDR as informational and still stops the bundled one", async () => {
    setUserAgent("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
    updateHerdrProcesses.mockResolvedValue({
      path: "C:\\Program Files\\Yuzora\\herdr\\windows-x86_64\\herdr.exe",
      version: "0.9.3",
      pids: [101],
      external: { path: "C:\\Tools\\herdr.exe", version: "0.9.2", pids: [303, 404] },
    })
    const install = vi.fn(async () => undefined)
    const download = vi.fn(async (onEvent?: (event: unknown) => void) => {
      onEvent?.({ event: "Finished" })
    })
    check.mockResolvedValue({ version: "0.0.4", download, install })

    await openInstallConfirmation()
    const confirmation = await screen.findByRole("alertdialog", { name: "Install update and restart?" })
    const note = await within(confirmation).findByTestId("herdr-external-note")
    expect(within(note).getByText("You use a self-installed HERDR")).toBeInTheDocument()
    expect(within(note).getByText("C:\\Tools\\herdr.exe")).toBeInTheDocument()
    expect(within(note).getByText("303, 404")).toBeInTheDocument()
    expect(note.className).not.toContain("destructive")
    expect(within(note).queryByText(/force-stops/)).not.toBeInTheDocument()
    // Only the bundled HERDR is stopped, so the warning and the action must not claim the HERDR in use.
    expect(within(confirmation).getByText(/force-stops the bundled HERDR/)).toBeInTheDocument()
    expect(within(confirmation).queryByText(/force-stops the HERDR in use/)).not.toBeInTheDocument()
    expect(within(confirmation).getByRole("button", { name: "Stop bundled HERDR and install" })).toBeInTheDocument()
    expect(updateStopHerdr).not.toHaveBeenCalled()
  })

  it("on Windows, a user-installed-only HERDR shows no kill warning and is never stopped", async () => {
    setUserAgent("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
    updateHerdrProcesses.mockResolvedValue({
      path: null,
      version: null,
      pids: [],
      external: { path: "C:\\Tools\\herdr.exe", version: null, pids: [303] },
    })
    const install = vi.fn(async () => undefined)
    const download = vi.fn(async (onEvent?: (event: unknown) => void) => {
      onEvent?.({ event: "Finished" })
    })
    check.mockResolvedValue({ version: "0.0.4", download, install })

    await openInstallConfirmation()
    const confirmation = await screen.findByRole("alertdialog", { name: "Install update and restart?" })
    await within(confirmation).findByTestId("herdr-external-note")
    expect(within(confirmation).queryByText(/force-stops/)).not.toBeInTheDocument()
    expect(within(confirmation).queryByRole("button", { name: "Stop HERDR and install" })).not.toBeInTheDocument()
    fireEvent.click(within(confirmation).getByRole("button", { name: "Install and restart" }))
    await waitFor(() => expect(install).toHaveBeenCalledTimes(1))
    expect(updateStopHerdr).not.toHaveBeenCalled()
  })

  it("on Windows, still runs the final stop when the managed HERDR had no PID at preflight but an external one is active", async () => {
    setUserAgent("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
    updateHerdrProcesses.mockResolvedValue({
      path: "C:\\Program Files\\Yuzora\\herdr\\windows-x86_64\\herdr.exe",
      version: "0.9.3",
      pids: [],
      external: { path: "C:\\Tools\\herdr.exe", version: null, pids: [303] },
    })
    const install = vi.fn(async () => undefined)
    const download = vi.fn(async (onEvent?: (event: unknown) => void) => {
      onEvent?.({ event: "Finished" })
    })
    check.mockResolvedValue({ version: "0.0.4", download, install })

    await openInstallConfirmation()
    const confirmation = await screen.findByRole("alertdialog", { name: "Install update and restart?" })
    await within(confirmation).findByTestId("herdr-external-note")
    fireEvent.click(within(confirmation).getByRole("button", { name: "Stop bundled HERDR and install" }))
    await waitFor(() => expect(install).toHaveBeenCalledTimes(1))
    expect(updateStopHerdr).toHaveBeenCalledTimes(1)
  })

  it("on Windows, keeps the update uninstalled when HERDR cannot be stopped", async () => {
    setUserAgent("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
    updateStopHerdr.mockRejectedValueOnce("HERDR processes are still running: 101")
    const install = vi.fn(async () => undefined)
    const download = vi.fn(async (onEvent?: (event: unknown) => void) => {
      onEvent?.({ event: "Finished" })
    })
    check.mockResolvedValue({ version: "0.0.4", download, install })

    await openInstallConfirmation()
    const confirmation = await screen.findByRole("alertdialog", {
      name: "Install update and restart?",
    })
    await within(confirmation).findByText("0.9.3")
    fireEvent.click(within(confirmation).getByRole("button", { name: "Stop HERDR and install" }))

    expect(
      await within(confirmation).findByText(
        "Couldn't stop HERDR, so the update wasn't installed: HERDR processes are still running: 101",
      ),
    ).toBeInTheDocument()
    expect(install).not.toHaveBeenCalled()
    expect(relaunch).not.toHaveBeenCalled()
    expect(
      within(confirmation).getByRole("button", { name: "Stop HERDR and install" }),
    ).toBeEnabled()
  })
})

async function openInstallConfirmation(): Promise<void> {
  render(
    <SettingsDialog
      open
      onOpenChange={() => {}}
      theme="light"
      onThemeChange={() => {}}
      initialSection="about"
    />,
  )

  fireEvent.click(await screen.findByRole("button", { name: "Check for updates" }))
  fireEvent.click(await screen.findByRole("button", { name: "Download update" }))
  fireEvent.click(await screen.findByRole("button", { name: "Install and restart" }))
}
