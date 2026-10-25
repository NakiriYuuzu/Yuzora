import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import {
  flushHerdrTerminalDiagnostics,
  recordHerdrTerminalMetric,
  setHerdrTerminalDiagnosticsEnabled,
  timeHerdrTerminalIpc,
} from "./herdrTerminalDiagnostics"

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((ok, fail) => { resolve = ok; reject = fail })
  return { promise, resolve, reject }
}

let logEvents: unknown[]
beforeEach(() => {
  vi.useFakeTimers()
  setHerdrTerminalDiagnosticsEnabled(false)
  logEvents = []
  mockIPC((command, args) => {
    if (command === "log_event") logEvents.push(args)
  })
})
afterEach(() => {
  setHerdrTerminalDiagnosticsEnabled(false)
  clearMocks()
  vi.useRealTimers()
})

it.each(["resolve", "reject"] as const)("ignores an old IPC %s after diagnostics is disabled and enabled again", async outcome => {
  setHerdrTerminalDiagnosticsEnabled(true)
  const request = deferred<object>()
  const run = vi.fn(() => request.promise)
  const measured = timeHerdrTerminalIpc("terminal.scroll", run)
  const value = { result: "original" }
  const error = new Error("original rejection")
  const assertion = outcome === "resolve" ? expect(measured).resolves.toBe(value) : expect(measured).rejects.toBe(error)
  setHerdrTerminalDiagnosticsEnabled(false)
  setHerdrTerminalDiagnosticsEnabled(true)
  if (outcome === "resolve") request.resolve(value)
  else request.reject(error)
  await assertion
  expect(run).toHaveBeenCalledTimes(1)
  expect(vi.getTimerCount()).toBe(0)
  expect(flushHerdrTerminalDiagnostics()).toBeNull()
  expect(logEvents).toEqual([])
})

it("keeps new-epoch observations when old and new IPC requests complete together", async () => {
  setHerdrTerminalDiagnosticsEnabled(true)
  const old = deferred<string>()
  const oldMeasured = timeHerdrTerminalIpc("pane.get", () => old.promise)
  setHerdrTerminalDiagnosticsEnabled(false)
  setHerdrTerminalDiagnosticsEnabled(true)
  const fresh = deferred<string>()
  const freshMeasured = timeHerdrTerminalIpc("pane.get", () => fresh.promise)
  fresh.resolve("fresh")
  old.resolve("old")
  await expect(Promise.all([oldMeasured, freshMeasured])).resolves.toEqual(["old", "fresh"])
  expect(flushHerdrTerminalDiagnostics()?.ipc["pane.get"]).toMatchObject({ n: 1, failed: 0 })
  expect(logEvents).toHaveLength(1)
  expect(vi.getTimerCount()).toBe(0)
})

it("preserves same-epoch success and failure across an unchanged enabled setting", async () => {
  setHerdrTerminalDiagnosticsEnabled(true)
  const success = deferred<number>(), failure = deferred<never>()
  const resolved = timeHerdrTerminalIpc("pane.scroll", () => success.promise)
  const rejected = timeHerdrTerminalIpc("pane.scroll", () => failure.promise)
  const error = new Error("same epoch")
  const rejection = expect(rejected).rejects.toBe(error)
  setHerdrTerminalDiagnosticsEnabled(true)
  success.resolve(7)
  failure.reject(error)
  await expect(resolved).resolves.toBe(7)
  await rejection
  expect(flushHerdrTerminalDiagnostics()?.ipc["pane.scroll"]).toMatchObject({ n: 2, failed: 1 })
})

it("records a current-epoch IPC in the next window after the normal five-second flush", async () => {
  setHerdrTerminalDiagnosticsEnabled(true)
  recordHerdrTerminalMetric({ kind: "wheel", strategy: "terminal", rows: 1 })
  const request = deferred<number>()
  const measured = timeHerdrTerminalIpc("terminal.scroll", () => request.promise)
  await vi.advanceTimersByTimeAsync(5000)
  expect(logEvents).toHaveLength(1)
  request.resolve(42)
  await expect(measured).resolves.toBe(42)
  expect(vi.getTimerCount()).toBe(1)
  expect(flushHerdrTerminalDiagnostics()?.ipc["terminal.scroll"]).toMatchObject({ n: 1, failed: 0 })
  expect(logEvents).toHaveLength(2)
  expect(vi.getTimerCount()).toBe(0)
})

it("keeps disabled-start calls unmeasured and preserves their original promise", async () => {
  const request = deferred<number>()
  const measured = timeHerdrTerminalIpc("pane.get", () => request.promise)
  expect(measured).toBe(request.promise)
  setHerdrTerminalDiagnosticsEnabled(true)
  request.resolve(9)
  await expect(measured).resolves.toBe(9)
  expect(flushHerdrTerminalDiagnostics()).toBeNull()
  expect(vi.getTimerCount()).toBe(0)
  expect(logEvents).toEqual([])
})
