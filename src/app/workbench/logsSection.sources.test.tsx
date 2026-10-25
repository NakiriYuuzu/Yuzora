import { afterEach, beforeEach, expect, it } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"
import type { LogQueryFilters } from "@/features/logs/logQuery"
import { LogsSection } from "./LogsSection"

const row = (source: string, event: string) => ({
  timestamp: "2026-10-24T00:00:00Z", run_id: "owned-run", level: "info", kind: "debug",
  source, event, message: event, workspace_path: null, metadata: {},
})
let records: ReturnType<typeof row>[]
let sourceResult: string[] | Promise<string[]>
let sourceCalls: number
let queries: LogQueryFilters[]
const select = () => screen.getByRole("combobox", { name: "source filter" }) as HTMLSelectElement
const options = () => Array.from(select().options, option => option.value)

beforeEach(() => {
  records = [row("app", "initial")]
  sourceResult = ["app"]
  sourceCalls = 0
  queries = []
  mockIPC((command, args) => {
    if (command === "log_sources") { sourceCalls++; return sourceResult }
    if (command === "get_log_enabled") return true
    if (command === "get_log_level") return "info"
    if (command === "log_query") {
      queries.push((args as { filters: LogQueryFilters }).filters)
      return records
    }
    return null
  })
})

afterEach(() => { cleanup(); clearMocks() })

it("adds a source first seen in a later query and sends the selected source filter", async () => {
  render(<LogsSection />)
  await screen.findByTestId("log-row-initial")
  expect(options()).toEqual(["", "app"])
  records = [row("new-worker", "later")]
  fireEvent.change(screen.getByRole("searchbox", { name: "Text search" }), { target: { value: "later" } })
  await screen.findByTestId("log-row-later")
  expect(screen.getByRole("option", { name: "new-worker" })).toBeInTheDocument()
  fireEvent.change(select(), { target: { value: "new-worker" } })
  await waitFor(() => expect(queries.at(-1)).toEqual({ limit: 500, text: "later", sources: ["new-worker"] }))
  expect(select().value).toBe("new-worker")
  expect(sourceCalls).toBe(1)
})

it("merges a late source index and retains discovered sources across empty and replacement results", async () => {
  let finish!: (sources: string[]) => void
  sourceResult = new Promise(resolve => { finish = resolve })
  records = [row("new-worker", "discovered"), row("new-worker", "duplicate")]
  render(<LogsSection />)
  await screen.findByTestId("log-row-discovered")
  expect(options()).toEqual(["", "new-worker"])
  await act(async () => { finish(["app", "app", ""]) })
  expect(options()).toEqual(["", "app", "new-worker"])

  records = []
  fireEvent.change(select(), { target: { value: "new-worker" } })
  await waitFor(() => expect(screen.queryByTestId("log-row-discovered")).toBeNull())
  expect(queries.at(-1)?.sources).toEqual(["new-worker"])
  expect(select().value).toBe("new-worker")
  records = [row("app", "replacement")]
  fireEvent.change(select(), { target: { value: "" } })
  await screen.findByTestId("log-row-replacement")
  expect(queries.at(-1)?.sources).toBeUndefined()
  expect(options()).toEqual(["", "app", "new-worker"])
  expect(sourceCalls).toBe(1)
})

it("shows an initial source absent from the index even when the query is empty, including a new open intent", async () => {
  records = []
  const { rerender } = render(<LogsSection initialSource="not-indexed" openNonce={1} />)
  await waitFor(() => expect(queries.at(-1)?.sources).toEqual(["not-indexed"]))
  expect(select().value).toBe("not-indexed")
  expect(options()).toEqual(["", "app", "not-indexed"])
  rerender(<LogsSection initialSource="another-source" openNonce={2} />)
  await waitFor(() => expect(queries.at(-1)?.sources).toEqual(["another-source"]))
  expect(select().value).toBe("another-source")
  expect(options()).toEqual(["", "another-source", "app"])
  expect(sourceCalls).toBe(1)
})

it("preserves indexed source filtering and clearing when no records exist", async () => {
  sourceResult = ["app", "worker"]
  records = []
  render(<LogsSection />)
  await screen.findByRole("option", { name: "worker" })
  fireEvent.change(select(), { target: { value: "worker" } })
  await waitFor(() => expect(queries.at(-1)?.sources).toEqual(["worker"]))
  expect(select().value).toBe("worker")
  fireEvent.change(select(), { target: { value: "" } })
  await waitFor(() => expect(queries.at(-1)?.sources).toBeUndefined())
  expect(options()).toEqual(["", "app", "worker"])
  expect(sourceCalls).toBe(1)
})
