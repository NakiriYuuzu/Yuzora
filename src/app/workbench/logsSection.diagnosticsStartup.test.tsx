import { afterEach, beforeEach, expect, it } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import {
  flushHerdrTerminalDiagnostics,
  initHerdrTerminalDiagnosticsFromLogLevel,
  isHerdrTerminalDiagnosticsEnabled,
  recordHerdrTerminalMetric,
  setHerdrTerminalDiagnosticsEnabled,
} from "@/terminal/herdrTerminalDiagnostics"
import { LogsSection } from "./LogsSection"

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((ok, fail) => { resolve = ok; reject = fail })
  return { promise, resolve, reject }
}
let startup: ReturnType<typeof deferred<string>>
let levelReads: number
let levelWrites: string[]
let panelLevel: string
let saveFails: boolean
let logEvents: unknown[]
const verbose = () => screen.getByRole("checkbox", { name: "Verbose logging (debug)" })

beforeEach(() => {
  setHerdrTerminalDiagnosticsEnabled(false)
  startup = deferred<string>()
  levelReads = 0
  levelWrites = []
  panelLevel = "debug"
  saveFails = false
  logEvents = []
  mockIPC((command, args) => {
    if (command === "get_log_level") return ++levelReads === 1 ? startup.promise : panelLevel
    if (command === "get_log_enabled") return true
    if (command === "log_sources" || command === "log_query") return []
    if (command === "set_log_level") {
      levelWrites.push((args as { level: string }).level)
      if (saveFails) throw new Error("fixture save failed")
      panelLevel = levelWrites.at(-1)!
    }
    if (command === "log_event") logEvents.push(args)
    return undefined
  })
})
afterEach(() => { cleanup(); setHerdrTerminalDiagnosticsEnabled(false); clearMocks() })

async function openWithPendingStartup() {
  const boot = initHerdrTerminalDiagnosticsFromLogLevel()
  await waitFor(() => expect(levelReads).toBe(1))
  render(<LogsSection />)
  await waitFor(() => expect(verbose()).toBeEnabled())
  return { boot }
}

it("keeps a completed verbose-off intent when startup debug arrives and diagnostics was already false", async () => {
  const { boot } = await openWithPendingStartup()
  expect(verbose()).toBeChecked()
  expect(isHerdrTerminalDiagnosticsEnabled()).toBe(false)
  fireEvent.click(verbose())
  await waitFor(() => { expect(levelWrites).toEqual(["info"]); expect(verbose()).toBeEnabled() })
  expect(verbose()).not.toBeChecked()
  await act(async () => { startup.resolve("debug"); await boot })
  expect(isHerdrTerminalDiagnosticsEnabled()).toBe(false)
  recordHerdrTerminalMetric({ kind: "wheel", strategy: "terminal", rows: 1 })
  expect(flushHerdrTerminalDiagnostics()).toBeNull()
  expect(logEvents).toEqual([])
})

it("preserves the user's latest completed on-then-off changes over delayed startup debug", async () => {
  panelLevel = "info"
  const { boot } = await openWithPendingStartup()
  expect(verbose()).not.toBeChecked()
  fireEvent.click(verbose())
  await waitFor(() => { expect(levelWrites).toEqual(["debug"]); expect(verbose()).toBeEnabled() })
  expect(isHerdrTerminalDiagnosticsEnabled()).toBe(true)
  fireEvent.click(verbose())
  await waitFor(() => { expect(levelWrites).toEqual(["debug", "info"]); expect(verbose()).toBeEnabled() })
  expect(isHerdrTerminalDiagnosticsEnabled()).toBe(false)
  await act(async () => { startup.resolve("debug"); await boot })
  expect(isHerdrTerminalDiagnosticsEnabled()).toBe(false)
  expect(verbose()).not.toBeChecked()
})

it("allows startup debug after a failed off save rolls the UI back", async () => {
  saveFails = true
  const { boot } = await openWithPendingStartup()
  fireEvent.click(verbose())
  await waitFor(() => { expect(levelWrites).toEqual(["info"]); expect(verbose()).toBeEnabled(); expect(verbose()).toBeChecked() })
  expect(screen.getByText(/fixture save failed/)).toBeInTheDocument()
  await act(async () => { startup.resolve("debug"); await boot })
  expect(isHerdrTerminalDiagnosticsEnabled()).toBe(true)
})

it.each([["debug", true], ["info", false]] as const)("initializes %s when no newer intent exists", async (level, expected) => {
  const boot = initHerdrTerminalDiagnosticsFromLogLevel()
  startup.resolve(level)
  await boot
  expect(isHerdrTerminalDiagnosticsEnabled()).toBe(expected)
  expect(levelWrites).toEqual([])
})

it("preserves an already enabled diagnostics setting on a non-debug startup response", async () => {
  setHerdrTerminalDiagnosticsEnabled(true)
  const boot = initHerdrTerminalDiagnosticsFromLogLevel()
  startup.resolve("info")
  await boot
  expect(isHerdrTerminalDiagnosticsEnabled()).toBe(true)
})

it("leaves current state intact when startup loading fails", async () => {
  const boot = initHerdrTerminalDiagnosticsFromLogLevel()
  startup.reject(new Error("fixture read failed"))
  await expect(boot).resolves.toBeUndefined()
  expect(isHerdrTerminalDiagnosticsEnabled()).toBe(false)
  expect(logEvents).toEqual([])
})
