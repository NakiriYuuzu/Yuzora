import { beforeEach, describe, expect, it, vi } from "vitest"
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { EditorView } from "@codemirror/view"

import { DatabasePanel, reorderColumns } from "@/app/panels/DatabasePanel"
import { formatDbValue } from "@/lib/types"
import type {
  DbQueryRun,
  DbQueryRunRequest,
  DbQueryResult,
  DbResultPage,
  DbResultSessionOwner,
} from "@/lib/types"
import { resultPageKey, useDbStore } from "@/state/dbStore"

vi.mock("@/lib/ipc", () => ({
  dbListTables: vi.fn(),
  dbQueryRun: vi.fn(),
  dbQueryCancel: vi.fn(),
  dbResultPagePrevious: vi.fn(),
  dbResultPageNext: vi.fn(),
  dbResultSessionRelease: vi.fn(),
  dbProfileCreate: vi.fn(),
  dbProfileDisconnect: vi.fn(),
  dbProfileForget: vi.fn(),
  dbProfileImportLegacy: vi.fn(),
  dbProfileList: vi.fn(),
  dbProfileOpen: vi.fn(),
  dbProfileRecover: vi.fn(),
  dbProfileRemoveCredential: vi.fn(),
  dbProfileUpdate: vi.fn(),
}))

import {
  dbListTables,
  dbProfileCreate,
  dbQueryCancel,
  dbQueryRun,
  dbResultPageNext,
  dbResultPagePrevious,
  dbResultSessionRelease,
} from "@/lib/ipc"

const mockList = vi.mocked(dbListTables)
const mockProfileCreate = vi.mocked(dbProfileCreate)
const mockQueryRun = vi.mocked(dbQueryRun)
const mockQueryCancel = vi.mocked(dbQueryCancel)
const mockResultPagePrevious = vi.mocked(dbResultPagePrevious)
const mockResultPageNext = vi.mocked(dbResultPageNext)
const mockResultSessionRelease = vi.mocked(dbResultSessionRelease)

const threeCol: Extract<DbQueryResult, { kind: "select" }> = {
  kind: "select",
  columns: ["id", "name", "age"],
  rows: [
    [
      { kind: "integer", value: "1" },
      { kind: "text", value: "alice" },
      { kind: "integer", value: "30" },
    ],
    [
      { kind: "integer", value: "2" },
      { kind: "text", value: "bob" },
      { kind: "integer", value: "25" },
    ],
  ],
  truncated: false,
  affectedRows: null,
  effectOutcome: "unknown",
}

function panelRunFromResult(request: DbQueryRunRequest, result: DbQueryResult): DbQueryRun {
  const statementExecutionId = `${request.queryRunId}:statement:0` as never
  const owner = {
    descriptorId: request.descriptorId,
    connectionId: request.connectionId,
    connectionGeneration: request.connectionGeneration,
    queryRunId: request.queryRunId,
    statementExecutionId,
    resultSessionId: `${request.queryRunId}:result:0` as never,
  }
  const firstResult = result.kind === "select"
    ? {
        kind: "rows" as const,
        resultSession: {
          owner,
          columns: result.columns,
          initialPage: {
            owner,
            pageIndex: 0,
            columns: result.columns,
            rows: result.rows,
            hasPrevious: false,
            hasNext: result.truncated,
            effectOutcome: result.effectOutcome,
            lifecycle: result.truncated ? "streaming" : "complete",
            resultLimitReached: false,
          },
        },
        affectedRows: result.affectedRows,
      }
    : { kind: "execute" as const, affectedRows: result.affectedRows }
  return {
    descriptorId: request.descriptorId,
    connectionId: request.connectionId,
    connectionGeneration: request.connectionGeneration,
    queryRunId: request.queryRunId,
    statements: request.statements.map((statement, statementIndex) => ({
      statementExecutionId: `${request.queryRunId}:statement:${statementIndex}` as never,
      statementIndex,
      sql: statement.sql,
      effectOutcome: statementIndex === 0 ? result.effectOutcome : "none" as const,
      result: statementIndex === 0 ? firstResult : { kind: "skipped" as const },
    })) as unknown as DbQueryRun["statements"],
    transactionMayBeOpen: false,
    connectionTerminated: false,
  }
}

function mockRunResultOnce(result: DbQueryResult): void {
  mockQueryRun.mockImplementationOnce(async (request) => panelRunFromResult(request, result))
}

function panelWirePage(
  owner: DbResultSessionOwner,
  overrides: Partial<Omit<DbResultPage, "owner">> = {},
): DbResultPage {
  return {
    owner,
    pageIndex: 0,
    columns: ["value"],
    rows: [[{ kind: "integer", value: "1" }]],
    hasPrevious: false,
    hasNext: false,
    effectOutcome: "none",
    lifecycle: "complete",
    resultLimitReached: false,
    ...overrides,
  }
}

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason?: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

function installLocalStorage(): void {
  const store = new Map<string, string>()
  const mock = {
    getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
    setItem: (k: string, v: string) => void store.set(k, String(v)),
    removeItem: (k: string) => void store.delete(k),
    clear: () => store.clear(),
    key: (i: number) => [...store.keys()][i] ?? null,
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

beforeEach(() => {
  installLocalStorage()
  localStorage.clear()
  useDbStore.getState().reset()
  vi.clearAllMocks()
  mockList.mockResolvedValue([])
  mockQueryRun.mockImplementation(async (request) => panelRunFromResult(request, threeCol))
  mockQueryCancel.mockResolvedValue({ outcome: "cancelled" })
  mockResultPagePrevious.mockImplementation(async (owner) => panelWirePage(owner))
  mockResultPageNext.mockImplementation(async (owner) => panelWirePage(owner))
  mockResultSessionRelease.mockImplementation(async (owner) => panelWirePage(owner, {
    lifecycle: "released",
  }))
  mockProfileCreate.mockImplementation(async (request) => {
    const descriptorId = "panel-profile" as never
    return {
      outcome: "connected",
      profile: {
        descriptorId,
        configGeneration: 1,
        name: request.name,
        target: request.target,
        credentialState: request.target.kind === "sqlite" ? "notRequired" : "stored"
      },
      connection: {
        descriptorId,
        connectionId: "db-1" as never,
        connectionGeneration: "panel-generation" as never,
        engine: request.target.kind
      }
    }
  })
})

// Seed an open connection whose console already holds a three-column result, so
// tests render straight into the sortable/reorderable ResultTable.
async function openWithResult(): Promise<void> {
  await useDbStore.getState().openConnection("/a.db")
  useDbStore.getState().setSql("SELECT * FROM t")
  await useDbStore.getState().runQuery()
}

async function openWithStreamingResult(): Promise<DbResultSessionOwner> {
  await useDbStore.getState().openConnection("/a.db")
  mockRunResultOnce({ ...threeCol, truncated: true })
  useDbStore.getState().setSql("SELECT * FROM paged_t")
  await useDbStore.getState().runQuery()
  const statement = useDbStore.getState().queryBuckets["panel-profile"].runGroup!.run!.statements[0]
  if (statement.result.kind !== "rows" || !statement.result.resultSession) {
    throw new Error("expected streaming result owner")
  }
  return statement.result.resultSession.owner
}

async function openWithCachedLifecycle(
  lifecycle: DbResultPage["lifecycle"],
  resultLimitReached = false,
): Promise<DbResultSessionOwner> {
  await useDbStore.getState().openConnection("/a.db")
  mockQueryRun.mockImplementationOnce(async (request) => {
    const run = panelRunFromResult(request, { ...threeCol, truncated: true })
    const statement = run.statements[0]
    if (statement.result.kind !== "rows" || !statement.result.resultSession) {
      throw new Error("expected cached row result")
    }
    const resultSession = {
      ...statement.result.resultSession,
      initialPage: {
        ...statement.result.resultSession.initialPage,
        lifecycle,
        hasNext: true,
        resultLimitReached,
      },
    }
    const result = resultLimitReached
      ? { kind: "resultLimitReached" as const, affectedRows: null, resultSession }
      : { ...statement.result, resultSession }
    return {
      ...run,
      statements: [{ ...statement, result }] as unknown as DbQueryRun["statements"],
    }
  })
  useDbStore.getState().setSql("SELECT cached_page();")
  await useDbStore.getState().runQuery()
  const statement = useDbStore.getState().queryBuckets["panel-profile"].runGroup!.run!.statements[0]
  if (statement.result.kind === "rows" && statement.result.resultSession) {
    return statement.result.resultSession.owner
  }
  if (statement.result.kind === "resultLimitReached") {
    return statement.result.resultSession.owner
  }
  throw new Error("expected cached result owner")
}

function headerTexts(): string[] {
  return screen.getAllByRole("columnheader").map((c) => c.textContent ?? "")
}

function bodyCellTexts(): string[] {
  return screen.getAllByRole("cell").map((cell) => cell.textContent ?? "")
}

function mountedEditorView(): EditorView {
  const view = EditorView.findFromDOM(screen.getByRole("textbox"))
  if (!view) throw new Error("expected a mounted CodeMirror EditorView")
  return view
}

const readerDescriptorId = "analytics-reader"
const adminDescriptorId = "analytics-admin"
const analyticsTargetKey = "postgres:analytics.internal:5432:warehouse"

function seedSameTargetProfiles(): void {
  useDbStore.setState({
    saved: [
      {
        id: readerDescriptorId,
        configGeneration: 1,
        targetKey: analyticsTargetKey,
        kind: "postgres",
        name: "Analytics reader",
        credentialState: "stored",
        host: "analytics.internal",
        port: 5432,
        database: "warehouse",
        user: "analyst",
        ssl: false,
        trustCert: false,
      },
      {
        id: adminDescriptorId,
        configGeneration: 1,
        targetKey: analyticsTargetKey,
        kind: "postgres",
        name: "Analytics admin",
        credentialState: "stored",
        host: "analytics.internal",
        port: 5432,
        database: "warehouse",
        user: "analyst",
        ssl: true,
        trustCert: false,
      },
    ],
    connections: [
      {
        connId: "reader-connection",
        connectionGeneration: "reader-generation" as never,
        kind: "postgres",
        name: "Analytics reader",
        descriptorId: readerDescriptorId,
        targetKey: analyticsTargetKey,
        title: "analyst@analytics.internal:5432/warehouse",
      },
      {
        connId: "admin-connection",
        connectionGeneration: "admin-generation" as never,
        kind: "postgres",
        name: "Analytics admin",
        descriptorId: adminDescriptorId,
        targetKey: analyticsTargetKey,
        title: "analyst@analytics.internal:5432/warehouse",
      },
    ],
    activeDescriptorId: readerDescriptorId,
    activeConnId: "reader-connection",
  })
  useDbStore.getState().setSql("SELECT 'reader'")
  useDbStore.getState().setActiveDescriptor(adminDescriptorId)
  useDbStore.getState().setSql("SELECT 'admin'")
  useDbStore.getState().setActiveDescriptor(readerDescriptorId)
}

describe("reorderColumns", () => {
  it("moves a column to a later display position", () => {
    expect(reorderColumns([0, 1, 2], 0, 2)).toEqual([1, 2, 0])
  })

  it("moves a column to an earlier display position", () => {
    expect(reorderColumns([0, 1, 2], 2, 0)).toEqual([2, 0, 1])
    expect(reorderColumns([0, 1, 2, 3], 3, 1)).toEqual([0, 3, 1, 2])
  })

  it("does not mutate the input array", () => {
    const input = [0, 1, 2]
    reorderColumns(input, 0, 1)
    expect(input).toEqual([0, 1, 2])
  })
})

describe("formatDbValue", () => {
  it("keeps precision strings exact and renders null/blob deliberately", () => {
    expect(formatDbValue({ kind: "integer", value: "9223372036854775807" })).toBe(
      "9223372036854775807"
    )
    expect(formatDbValue({ kind: "decimal", value: "1234567890.123456789" })).toBe(
      "1234567890.123456789"
    )
    expect(formatDbValue({ kind: "null" })).toBeNull()
    expect(formatDbValue({ kind: "binary", hex: "0001ff" })).toBe("<blob 3 bytes>")
  })
})

describe("DatabasePanel execution controls", () => {
  it("exposes distinct Primary Run, Run All, and Cancel actions", async () => {
    await useDbStore.getState().openConnection("/a.db")
    useDbStore.getState().setSql("SELECT 1; SELECT 2;")
    render(<DatabasePanel />)

    expect(screen.getByRole("button", {
      name: "Run selection or current statement",
    })).toBeEnabled()
    expect(screen.getByRole("button", { name: "Run all statements" })).toBeEnabled()
    expect(screen.getByRole("button", { name: "Cancel running query" })).toBeDisabled()

    fireEvent.click(screen.getByRole("button", { name: "Run all statements" }))
    await waitFor(() => expect(mockQueryRun).toHaveBeenCalledWith(expect.objectContaining({
      mode: "script",
      statements: [
        { sql: "SELECT 1;", transactionBoundary: "none" },
        { sql: "SELECT 2;", transactionBoundary: "none" },
      ],
    })))
  })

  it("maps Mod-Enter to Primary Run and Mod-Shift-Enter to Run All", async () => {
    await useDbStore.getState().openConnection("/a.db")
    useDbStore.getState().setSql("SELECT 1; SELECT 2;")
    render(<DatabasePanel />)
    const editor = screen.getByRole("textbox")

    fireEvent.keyDown(editor, { key: "Enter", code: "Enter", ctrlKey: true })
    await waitFor(() => expect(mockQueryRun).toHaveBeenCalledWith(expect.objectContaining({
      mode: "primary",
      statements: [{ sql: "SELECT 1;", transactionBoundary: "none" }],
    })))

    mockQueryRun.mockClear()
    fireEvent.keyDown(editor, {
      key: "Enter",
      code: "Enter",
      ctrlKey: true,
      shiftKey: true,
    })
    await waitFor(() => expect(mockQueryRun).toHaveBeenCalledWith(expect.objectContaining({
      mode: "script",
      statements: [
        { sql: "SELECT 1;", transactionBoundary: "none" },
        { sql: "SELECT 2;", transactionBoundary: "none" },
      ],
    })))
  })

  it("renders statement tabs, initial pages, effects, limits, and transaction warnings", async () => {
    await useDbStore.getState().openConnection("/a.db")
    const sqlText = [
      "SELECT 1;",
      "UPDATE counters SET value = 1;",
      "SELECT many_rows();",
      "COMMIT;",
    ].join("\n")
    useDbStore.getState().setSql(sqlText)
    mockQueryRun.mockImplementationOnce(async (request) => {
      const base = panelRunFromResult(request, threeCol)
      const limitStatementId = `${request.queryRunId}:statement:2` as never
      const limitOwner = {
        descriptorId: request.descriptorId,
        connectionId: request.connectionId,
        connectionGeneration: request.connectionGeneration,
        queryRunId: request.queryRunId,
        statementExecutionId: limitStatementId,
        resultSessionId: `${request.queryRunId}:result:2` as never,
      }
      return {
        ...base,
        transactionMayBeOpen: true,
        statements: [
          { ...base.statements[0], effectOutcome: "none" },
          {
            statementExecutionId: `${request.queryRunId}:statement:1` as never,
            statementIndex: 1,
            sql: request.statements[1].sql,
            effectOutcome: "committed" as const,
            result: { kind: "execute" as const, affectedRows: "1" },
          },
          {
            statementExecutionId: limitStatementId,
            statementIndex: 2,
            sql: request.statements[2].sql,
            effectOutcome: "unknown" as const,
            result: {
              kind: "resultLimitReached" as const,
              affectedRows: null,
              resultSession: {
                owner: limitOwner,
                columns: ["value"],
                initialPage: {
                  owner: limitOwner,
                  pageIndex: 0,
                  columns: ["value"],
                  rows: [[{ kind: "integer" as const, value: "1" }]],
                  hasPrevious: false,
                  hasNext: false,
                  effectOutcome: "unknown" as const,
                  lifecycle: "cancelled" as const,
                  resultLimitReached: true,
                },
              },
            },
          },
          {
            statementExecutionId: `${request.queryRunId}:statement:3` as never,
            statementIndex: 3,
            sql: request.statements[3].sql,
            effectOutcome: "none" as const,
            result: { kind: "skipped" as const },
          },
        ],
      }
    })
    render(<DatabasePanel />)

    fireEvent.click(screen.getByRole("button", { name: "Run all statements" }))

    expect(await screen.findByRole("tablist", { name: "Statement results" })).toBeInTheDocument()
    expect(screen.getByRole("tab", { name: "Statement 1: Rows" })).toBeInTheDocument()
    expect(screen.getByRole("tab", { name: "Statement 2: Executed" })).toBeInTheDocument()
    expect(screen.getByRole("tab", { name: "Statement 3: Result limit reached" })).toHaveAttribute(
      "aria-selected",
      "true",
    )
    expect(screen.getByRole("tab", { name: "Statement 4: Skipped" })).toBeInTheDocument()
    expect(screen.getByText("A transaction may still be open.")).toBeInTheDocument()
    expect(screen.getByText("Effect unknown")).toBeInTheDocument()
    expect(screen.getAllByText("Result limit reached").length).toBeGreaterThan(1)
    expect(screen.getByText("Result cancelled")).toBeInTheDocument()
    expect(screen.getByText("Page 1")).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Load next result page" })).toBeDisabled()
    expect(screen.getByRole("button", { name: "Release this result session" })).toBeDisabled()
    const editor = mountedEditorView()
    const limitedSql = "SELECT many_rows();"
    await waitFor(() => expect(editor.state.selection.main).toMatchObject({
      from: sqlText.indexOf(limitedSql),
      to: sqlText.indexOf(limitedSql) + limitedSql.length,
    }))

    fireEvent.click(screen.getByRole("tab", { name: "Statement 2: Executed" }))
    expect(screen.getByText("1 row affected")).toBeInTheDocument()
    expect(screen.getByText("Committed")).toBeInTheDocument()
    const executedSql = "UPDATE counters SET value = 1;"
    await waitFor(() => expect(editor.state.selection.main).toMatchObject({
      from: sqlText.indexOf(executedSql),
      to: sqlText.indexOf(executedSql) + executedSql.length,
    }))

    fireEvent.keyDown(screen.getByRole("tab", { name: "Statement 2: Executed" }), {
      key: "ArrowRight",
    })
    expect(screen.getByRole("tab", { name: "Statement 3: Result limit reached" })).toHaveAttribute(
      "aria-selected",
      "true",
    )
  })

  it("reports error and cancelled statement tabs without presenting either as success", async () => {
    await useDbStore.getState().openConnection("/a.db")
    useDbStore.getState().setSql("SELECT broken();\nSELECT slow();")
    mockQueryRun.mockImplementationOnce(async (request) => ({
      ...panelRunFromResult(request, threeCol),
      statements: [
        {
          statementExecutionId: `${request.queryRunId}:statement:0` as never,
          statementIndex: 0,
          sql: request.statements[0].sql,
          effectOutcome: "rolledBack" as const,
          result: {
            kind: "error" as const,
            error: {
              engine: "sqlite" as const,
              message: "syntax error",
              code: "SQLITE_ERROR",
              position: null,
              detail: null,
              hint: null,
              retryability: "notRetryable" as const,
            },
          },
        },
        {
          statementExecutionId: `${request.queryRunId}:statement:1` as never,
          statementIndex: 1,
          sql: request.statements[1].sql,
          effectOutcome: "unknown" as const,
          result: {
            kind: "cancelled" as const,
            error: {
              engine: "yuzora" as const,
              message: "query cancelled",
              code: "cancelled",
              position: null,
              detail: null,
              hint: null,
              retryability: "notRetryable" as const,
            },
          },
        },
      ],
    }))
    render(<DatabasePanel />)

    fireEvent.click(screen.getByRole("button", { name: "Run all statements" }))

    expect(await screen.findByRole("tab", { name: "Statement 1: Error" })).toHaveAttribute(
      "aria-selected",
      "true",
    )
    expect(screen.getByRole("alert")).toHaveTextContent("syntax error")
    expect(screen.getByText("Rolled back")).toBeInTheDocument()

    fireEvent.keyDown(screen.getByRole("tab", { name: "Statement 1: Error" }), {
      key: "ArrowRight",
    })
    expect(screen.getByRole("tab", { name: "Statement 2: Cancelled" })).toHaveAttribute(
      "aria-selected",
      "true",
    )
    expect(screen.getByRole("alert")).toHaveTextContent("query cancelled")
    expect(screen.queryByText(/rows? affected/i)).not.toBeInTheDocument()
  })

  it("shows a structured parse error and selects its exact editor range before IPC", async () => {
    await useDbStore.getState().openConnection("/a.db")
    const sqlText = "SELECT 1;\nSELECT 'unterminated"
    const quoteOffset = sqlText.indexOf("'")
    useDbStore.getState().setSql(sqlText)
    render(<DatabasePanel />)

    fireEvent.click(screen.getByRole("button", { name: "Run all statements" }))

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "The SQL contains an unterminated string.",
    )
    const editor = mountedEditorView()
    await waitFor(() => expect(editor.state.selection.main).toMatchObject({
      from: quoteOffset,
      to: quoteOffset + 1,
    }))
    expect(mockQueryRun).not.toHaveBeenCalled()
  })

  it("cancels the exact running owner and disables duplicate cancellation", async () => {
    await useDbStore.getState().openConnection("/a.db")
    useDbStore.getState().setSql("SELECT slow();")
    let request!: DbQueryRunRequest
    let settleRun!: (run: DbQueryRun) => void
    mockQueryRun.mockImplementationOnce((nextRequest) => {
      request = nextRequest
      return new Promise((resolve) => {
        settleRun = resolve
      })
    })
    let settleCancel!: (result: { outcome: "cancelled" }) => void
    mockQueryCancel.mockImplementationOnce(() => new Promise((resolve) => {
      settleCancel = resolve
    }))
    render(<DatabasePanel />)

    fireEvent.click(screen.getByRole("button", { name: "Run all statements" }))
    const cancel = screen.getByRole("button", { name: "Cancel running query" })
    await waitFor(() => expect(cancel).toBeEnabled())
    fireEvent.click(cancel)

    expect(mockQueryCancel).toHaveBeenCalledWith({
      descriptorId: request.descriptorId,
      connectionId: request.connectionId,
      connectionGeneration: request.connectionGeneration,
      queryRunId: request.queryRunId,
    })
    expect(cancel).toBeDisabled()
    expect(cancel).toHaveTextContent("Cancelling…")

    await act(async () => {
      settleCancel({ outcome: "cancelled" })
      settleRun(panelRunFromResult(request, threeCol))
    })
    await waitFor(() => expect(cancel).toBeDisabled())
    expect(mockQueryCancel).toHaveBeenCalledTimes(1)
  })

  it("keeps terminated cancellation tabs visible in an offline read-only console", async () => {
    await useDbStore.getState().openConnection("/a.db")
    useDbStore.getState().setSql("SELECT slow();\nSELECT later();")
    mockQueryRun.mockImplementationOnce(async (request) => {
      const base = panelRunFromResult(request, threeCol)
      return {
        ...base,
        connectionTerminated: true,
        statements: [
          {
            statementExecutionId: `${request.queryRunId}:statement:0` as never,
            statementIndex: 0,
            sql: request.statements[0].sql,
            effectOutcome: "unknown" as const,
            result: {
              kind: "cancelled" as const,
              error: {
                engine: "yuzora" as const,
                message: "cancelled by closing the connection",
                code: "cancelled",
                position: null,
                detail: null,
                hint: null,
                retryability: "notRetryable" as const,
              },
            },
          },
          {
            statementExecutionId: `${request.queryRunId}:statement:1` as never,
            statementIndex: 1,
            sql: request.statements[1].sql,
            effectOutcome: "none" as const,
            result: { kind: "skipped" as const },
          },
        ],
      }
    })
    mockList.mockClear()
    render(<DatabasePanel />)

    fireEvent.click(screen.getByRole("button", { name: "Run all statements" }))

    expect(await screen.findByText(
      "The connection was closed to cancel this query. Reconnect to run again.",
    )).toBeInTheDocument()
    expect(useDbStore.getState().activeDescriptorId).toBeNull()
    expect(screen.getByRole("tab", { name: "Statement 1: Cancelled" })).toBeInTheDocument()
    expect(screen.getByRole("tab", { name: "Statement 2: Skipped" })).toBeInTheDocument()
    expect(screen.getByRole("button", {
      name: "Run selection or current statement",
    })).toBeDisabled()
    expect(screen.getByRole("button", { name: "Run all statements" })).toBeDisabled()
    expect(screen.getByRole("button", { name: "Cancel running query" })).toBeDisabled()
    expect(screen.getByRole("textbox")).toHaveAttribute("contenteditable", "false")

    fireEvent.click(screen.getByRole("tab", { name: "Statement 2: Skipped" }))
    expect(screen.getByRole("tab", { name: "Statement 2: Skipped" })).toHaveAttribute(
      "aria-selected",
      "true",
    )
    expect(mockList).not.toHaveBeenCalled()
  })
})

describe("DatabasePanel structured query errors", () => {
  const structuredFailure = {
    code: "queryFailed",
    message: "database query failed",
    error: {
      engine: "postgres",
      message: "syntax error",
      code: "42601",
      position: null,
      detail: "near FROM",
      hint: "check the select list",
      retryability: "notRetryable"
    }
  }

  it("shows typed engine detail from the exact executed query error", async () => {
    await useDbStore.getState().openConnection("/a.db")
    useDbStore.getState().setSql("SELECT FROM data")
    mockQueryRun.mockRejectedValueOnce(structuredFailure)
    render(<DatabasePanel />)
    const run = screen.getByRole("button", { name: "Run selection or current statement" })

    fireEvent.click(run)

    const alert = await screen.findByRole("alert")
    expect(alert).toHaveTextContent("syntax error")
    expect(alert).toHaveTextContent("near FROM")
    expect(alert).toHaveTextContent("check the select list")
  })

  it("does not jump when the editor changed before the old error settled", async () => {
    await useDbStore.getState().openConnection("/a.db")
    useDbStore.getState().setSql("SELECT FROM data")
    let reject!: (error: unknown) => void
    mockQueryRun.mockImplementationOnce(() => new Promise((_resolve, rejectQuery) => {
      reject = rejectQuery
    }))
    render(<DatabasePanel />)
    const run = screen.getByRole("button", { name: "Run selection or current statement" })
    fireEvent.click(run)
    await act(async () => {
      useDbStore.getState().setSql("SELECT edited")
    })
    await act(async () => {
      reject(structuredFailure)
    })

    expect(await screen.findByRole("alert")).toHaveTextContent("syntax error")
    expect(screen.getByRole("textbox")).toHaveTextContent("SELECT edited")
    expect(useDbStore.getState().queryBuckets["panel-profile"].error).toMatchObject({
      executedSql: "SELECT FROM data"
    })
  })
})

describe("DatabasePanel active profile header", () => {
  it("identifies the exact saved profile with its non-secret engine and address", () => {
    seedSameTargetProfiles()

    render(<DatabasePanel />)

    const header = screen.getByRole("group", {
      name: "Active database profile: Analytics reader",
    })
    expect(header).toHaveTextContent("Active profile")
    expect(header).toHaveTextContent("Analytics reader")
    expect(header).toHaveTextContent("PostgreSQL")
    expect(header).toHaveTextContent("analyst@analytics.internal:5432/warehouse")
  })

  it("switches same-target profiles and runs against the exact active descriptor", async () => {
    seedSameTargetProfiles()
    render(<DatabasePanel />)
    mockQueryRun.mockClear()

    act(() => useDbStore.getState().setActiveDescriptor(adminDescriptorId))

    expect(screen.getByRole("group", {
      name: "Active database profile: Analytics admin",
    })).toHaveTextContent("Analytics admin")
    expect(screen.queryByRole("group", {
      name: "Active database profile: Analytics reader",
    })).not.toBeInTheDocument()
    expect(screen.getByRole("textbox")).toHaveTextContent("SELECT 'admin'")

    fireEvent.click(screen.getByRole("button", { name: "Run selection or current statement" }))

    await waitFor(() => expect(mockQueryRun).toHaveBeenCalledWith(expect.objectContaining({
      descriptorId: adminDescriptorId,
      connectionId: "admin-connection",
      connectionGeneration: "admin-generation",
      statements: [{ sql: "SELECT 'admin'", transactionBoundary: "none" }],
    })))
  })
})

describe("DatabasePanel result table", () => {
  it("isolates loaded-page sorting when switching between two rows tabs", async () => {
    await useDbStore.getState().openConnection("/a.db")
    useDbStore.getState().setSql("SELECT first_value();\nSELECT second_value();")
    mockQueryRun.mockImplementationOnce(async (request) => {
      const statement = (statementIndex: number, values: string[]) => {
        const statementExecutionId = `${request.queryRunId}:statement:${statementIndex}` as never
        const owner = {
          descriptorId: request.descriptorId,
          connectionId: request.connectionId,
          connectionGeneration: request.connectionGeneration,
          queryRunId: request.queryRunId,
          statementExecutionId,
          resultSessionId: `${request.queryRunId}:result:${statementIndex}` as never,
        }
        return {
          statementExecutionId,
          statementIndex,
          sql: request.statements[statementIndex].sql,
          effectOutcome: "none" as const,
          result: {
            kind: "rows" as const,
            affectedRows: null,
            resultSession: {
              owner,
              columns: ["value"],
              initialPage: {
                owner,
                pageIndex: 0,
                columns: ["value"],
                rows: values.map((value) => [{ kind: "integer" as const, value }]),
                hasPrevious: false,
                hasNext: false,
                effectOutcome: "none" as const,
                lifecycle: "complete" as const,
                resultLimitReached: false,
              },
            },
          },
        }
      }
      return {
        ...panelRunFromResult(request, threeCol),
        statements: [statement(0, ["2", "1"]), statement(1, ["20", "10"])],
      }
    })
    render(<DatabasePanel />)
    fireEvent.click(screen.getByRole("button", { name: "Run all statements" }))

    expect(await screen.findByRole("tab", { name: "Statement 1: Rows" })).toHaveAttribute(
      "aria-selected",
      "true",
    )
    expect(bodyCellTexts()).toEqual(["2", "1"])
    mockQueryRun.mockClear()
    fireEvent.click(screen.getByRole("button", { name: "Sort by value" }))
    await waitFor(() => expect(bodyCellTexts()).toEqual(["1", "2"]))

    fireEvent.click(screen.getByRole("tab", { name: "Statement 2: Rows" }))
    expect(screen.getByRole("button", { name: "Sort by value" }).closest("th")).toHaveAttribute(
      "aria-sort",
      "none",
    )
    expect(bodyCellTexts()).toEqual(["20", "10"])
    fireEvent.click(screen.getByRole("button", { name: "Sort by value" }))
    await waitFor(() => expect(bodyCellTexts()).toEqual(["10", "20"]))

    const group = useDbStore.getState().queryBuckets["panel-profile"].runGroup
    const run = group?.run
    const rowValues = (statementIndex: number) => {
      const result = run?.statements[statementIndex].result
      if (result?.kind !== "rows") throw new Error("expected rows statement")
      const owner = result.resultSession?.owner
      if (!owner) throw new Error("expected result owner")
      return group?.resultPages[resultPageKey(owner)].page.rows.map((row) => row[0])
    }
    expect(rowValues(0)).toEqual([
      { kind: "integer", value: "1" },
      { kind: "integer", value: "2" },
    ])
    expect(rowValues(1)).toEqual([
      { kind: "integer", value: "10" },
      { kind: "integer", value: "20" },
    ])
    expect(mockQueryRun).not.toHaveBeenCalled()
  })

  it.each([
    ["1", "1 row affected"],
    ["3", "3 rows affected"],
    ["9007199254740993", "9007199254740993 rows affected"],
  ])("renders the exact affected-row string %s", async (affectedRows, expected) => {
    await useDbStore.getState().openConnection("/a.db")
    mockRunResultOnce({
      kind: "execute",
      affectedRows,
      effectOutcome: "unknown"
    })
    useDbStore.getState().setSql("UPDATE items SET refreshed = 1")
    await useDbStore.getState().runQuery()

    render(<DatabasePanel />)

    expect(screen.getByText(expected)).toBeInTheDocument()
  })

  it("shows unavailable affected rows without fabricating zero", async () => {
    await useDbStore.getState().openConnection("/a.db")
    mockRunResultOnce({
      kind: "execute",
      affectedRows: null,
      effectOutcome: "unknown"
    })
    useDbStore.getState().setSql("SET NOCOUNT ON")
    await useDbStore.getState().runQuery()

    render(<DatabasePanel />)

    expect(screen.getByText("Affected row count unavailable")).toBeInTheDocument()
    expect(screen.queryByText(/0 rows? affected/i)).not.toBeInTheDocument()
  })

  it("renders from activeDescriptorId even if the compatibility conn projection is stale", async () => {
    await openWithResult()
    useDbStore.setState({ activeConnId: "stale-compatibility-conn" })

    render(<DatabasePanel />)

    expect(headerTexts()).toEqual(["id", "name", "age"])
  })

  it("renders the columns in order with divider borders", async () => {
    await openWithResult()
    render(<DatabasePanel />)

    expect(headerTexts()).toEqual(["id", "name", "age"])
  })

  it("clicking a header sorts only the loaded page without dispatching SQL", async () => {
    await openWithResult()
    mockQueryRun.mockClear()
    render(<DatabasePanel />)

    fireEvent.click(screen.getByRole("button", { name: "Sort by name" }))

    await waitFor(() => expect(
      screen.getByRole("button", { name: "Sort by name" }).closest("th")
    ).toHaveAttribute("aria-sort", "ascending"))
    expect(mockQueryRun).not.toHaveBeenCalled()
  })

  it("never rewrites a trailing semicolon/comment while sorting", async () => {
    await useDbStore.getState().openConnection("/a.db")
    useDbStore.getState().setSql("SELECT * FROM t; -- latest\n")
    await useDbStore.getState().runQuery()
    mockQueryRun.mockClear()
    render(<DatabasePanel />)

    fireEvent.click(screen.getByRole("button", { name: "Sort by id" }))

    await waitFor(() => expect(
      screen.getByRole("button", { name: "Sort by id" }).closest("th")
    ).toHaveAttribute("aria-sort", "ascending"))
    expect(mockQueryRun).not.toHaveBeenCalled()
  })

  it("toggles the arrow asc → desc → cleared via aria-sort", async () => {
    await openWithResult()
    render(<DatabasePanel />)
    const idHeader = () => screen.getByRole("button", { name: "Sort by id" }).closest("th")!
    expect(idHeader()).toHaveAttribute("aria-sort", "none")

    fireEvent.click(screen.getByRole("button", { name: "Sort by id" }))
    await waitFor(() => expect(idHeader()).toHaveAttribute("aria-sort", "ascending"))
    expect(document.querySelector(".lucide-chevron-up")).not.toBeNull()

    fireEvent.click(screen.getByRole("button", { name: "Sort by id" }))
    await waitFor(() => expect(idHeader()).toHaveAttribute("aria-sort", "descending"))
    expect(document.querySelector(".lucide-chevron-down")).not.toBeNull()

    fireEvent.click(screen.getByRole("button", { name: "Sort by id" }))
    await waitFor(() => expect(idHeader()).toHaveAttribute("aria-sort", "none"))
  })

  it("drag-reorder keeps sort keyed to the original column, not the display slot", async () => {
    await openWithResult()
    render(<DatabasePanel />)

    // Drag "id" (display 0, original 0) onto "age" (display 2) → [name, age, id].
    fireEvent.dragStart(screen.getByRole("button", { name: "Sort by id" }))
    fireEvent.drop(screen.getByRole("button", { name: "Sort by age" }))
    expect(headerTexts()).toEqual(["name", "age", "id"])

    // The header now in display slot 0 is still the original "name" column.
    mockQueryRun.mockClear()
    fireEvent.click(screen.getByRole("button", { name: "Sort by name" }))
    await waitFor(() => expect(
      screen.getByRole("button", { name: "Sort by name" }).closest("th")
    ).toHaveAttribute("aria-sort", "ascending"))
    expect(mockQueryRun).not.toHaveBeenCalled()
  })

  it("preserves a dragged order when the same columns come back, resets on new columns", async () => {
    await openWithResult()
    render(<DatabasePanel />)

    fireEvent.dragStart(screen.getByRole("button", { name: "Sort by id" }))
    fireEvent.drop(screen.getByRole("button", { name: "Sort by age" }))
    expect(headerTexts()).toEqual(["name", "age", "id"])

    // A page-local header sort keeps the dragged display order.
    mockQueryRun.mockClear()
    fireEvent.click(screen.getByRole("button", { name: "Sort by name" }))
    await waitFor(() => expect(
      screen.getByRole("button", { name: "Sort by name" }).closest("th")
    ).toHaveAttribute("aria-sort", "ascending"))
    expect(mockQueryRun).not.toHaveBeenCalled()
    expect(headerTexts()).toEqual(["name", "age", "id"])

    // A genuinely different result (different column names) → order resets.
    mockRunResultOnce({
      kind: "select",
      columns: ["x", "y"],
      rows: [[{ kind: "integer", value: "1" }, { kind: "integer", value: "2" }]],
      truncated: false,
      affectedRows: null,
      effectOutcome: "unknown",
    })
    await act(async () => {
      useDbStore.getState().setSql("SELECT x, y FROM t2")
      await useDbStore.getState().runQuery()
    })
    expect(headerTexts()).toEqual(["x", "y"])
  })
})

describe("DatabasePanel result session controls", () => {
  it("navigates with exact enablement and shows loading and end state", async () => {
    const owner = await openWithStreamingResult()
    const next = deferred<DbResultPage>()
    mockResultPageNext.mockReturnValueOnce(next.promise)
    render(<DatabasePanel />)

    const previousButton = screen.getByRole("button", { name: "Load previous result page" })
    const nextButton = screen.getByRole("button", { name: "Load next result page" })
    const releaseButton = screen.getByRole("button", { name: "Release this result session" })
    expect(screen.getByText("Page 1")).toBeInTheDocument()
    expect(previousButton).toBeDisabled()
    expect(nextButton).toBeEnabled()
    expect(releaseButton).toBeEnabled()

    fireEvent.click(nextButton)
    await waitFor(() => expect(screen.getByText("Loading result page…")).toBeInTheDocument())
    expect(previousButton).toBeDisabled()
    expect(nextButton).toBeDisabled()
    expect(releaseButton).toBeDisabled()

    next.resolve(panelWirePage(owner, {
      pageIndex: 1,
      columns: ["value"],
      rows: [[{ kind: "integer", value: "501" }]],
      hasPrevious: true,
      lifecycle: "complete",
    }))
    await waitFor(() => expect(screen.getByText("Page 2")).toBeInTheDocument())
    expect(previousButton).toBeEnabled()
    expect(nextButton).toBeDisabled()
    expect(releaseButton).toBeDisabled()
    expect(screen.getByText("End of result")).toBeInTheDocument()
    expect(mockResultPageNext).toHaveBeenCalledWith(owner)

    mockResultPagePrevious.mockResolvedValueOnce(panelWirePage(owner, {
      columns: threeCol.columns,
      rows: threeCol.rows,
      hasNext: true,
      lifecycle: "streaming",
    }))
    fireEvent.click(previousButton)
    await waitFor(() => expect(screen.getByText("Page 1")).toBeInTheDocument())
    expect(mockResultPagePrevious).toHaveBeenCalledWith(owner)
    expect(nextButton).toBeEnabled()
    expect(releaseButton).toBeEnabled()
  })

  it("settles Release with final rows and keeps the terminal page sortable", async () => {
    const owner = await openWithStreamingResult()
    const release = deferred<DbResultPage>()
    mockResultSessionRelease.mockReturnValueOnce(release.promise)
    render(<DatabasePanel />)
    mockQueryRun.mockClear()

    fireEvent.click(screen.getByRole("button", { name: "Release this result session" }))
    await waitFor(() => expect(screen.getByText("Loading result page…")).toBeInTheDocument())
    release.resolve(panelWirePage(owner, {
      pageIndex: 0,
      columns: ["value"],
      rows: [
        [{ kind: "integer", value: "2" }],
        [{ kind: "integer", value: "1" }],
      ],
      lifecycle: "released",
      effectOutcome: "committed",
    }))

    await waitFor(() => expect(screen.getByText("Result released")).toBeInTheDocument())
    expect(screen.getByText("Committed")).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Load next result page" })).toBeDisabled()
    expect(screen.getByRole("button", { name: "Release this result session" })).toBeDisabled()
    fireEvent.click(screen.getByRole("button", { name: "Sort by value" }))
    await waitFor(() => expect(bodyCellTexts()).toEqual(["1", "2"]))
    expect(mockQueryRun).not.toHaveBeenCalled()
  })

  it("shows a page error without replacing the current rows", async () => {
    await openWithStreamingResult()
    mockResultPageNext.mockRejectedValueOnce({
      code: "queryFailed",
      message: "page failed",
    })
    render(<DatabasePanel />)

    fireEvent.click(screen.getByRole("button", { name: "Load next result page" }))

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Could not load this result page.",
    )
    expect(screen.getByText("Page 1")).toBeInTheDocument()
    expect(bodyCellTexts()).toEqual(["1", "alice", "30", "2", "bob", "25"])
    expect(screen.getByRole("button", { name: "Load next result page" })).toBeEnabled()
    expect(screen.getByRole("button", { name: "Release this result session" })).toBeEnabled()
  })

  it.each([
    ["complete cached page", false],
    ["result-limit cached page", true],
  ])("enables Next for a %s with hasNext", async (_label, resultLimitReached) => {
    const owner = await openWithCachedLifecycle("complete", resultLimitReached)
    mockResultPageNext.mockResolvedValueOnce(panelWirePage(owner, {
      pageIndex: 1,
      hasPrevious: true,
      lifecycle: "complete",
      resultLimitReached,
    }))
    render(<DatabasePanel />)

    const nextButton = screen.getByRole("button", { name: "Load next result page" })
    expect(nextButton).toBeEnabled()
    expect(screen.getByRole("button", { name: "Release this result session" })).toBeDisabled()
    fireEvent.click(nextButton)

    await waitFor(() => expect(mockResultPageNext).toHaveBeenCalledWith(owner))
    expect(await screen.findByText("Page 2")).toBeInTheDocument()
  })

  it.each(["released", "cancelled", "error"] as const)(
    "disables Next for a %s lifecycle even when hasNext",
    async (lifecycle) => {
      await openWithCachedLifecycle(lifecycle)
      render(<DatabasePanel />)

      expect(screen.getByRole("button", { name: "Load next result page" })).toBeDisabled()
      expect(screen.getByRole("button", { name: "Release this result session" })).toBeDisabled()
      expect(mockResultPageNext).not.toHaveBeenCalled()
    },
  )

  it("enables Cancel for a settled active streaming page and preserves its cached rows", async () => {
    const owner = await openWithStreamingResult()
    const group = useDbStore.getState().queryBuckets["panel-profile"].runGroup!
    expect(group.status).toBe("settled")
    render(<DatabasePanel />)

    const cancelButton = screen.getByRole("button", { name: "Cancel running query" })
    expect(cancelButton).toBeEnabled()
    fireEvent.click(cancelButton)

    await waitFor(() => expect(mockQueryCancel).toHaveBeenCalledWith(group.owner))
    expect(await screen.findByText("Result cancelled")).toBeInTheDocument()
    expect(bodyCellTexts()).toEqual(["1", "alice", "30", "2", "bob", "25"])
    expect(screen.getByRole("button", { name: "Load next result page" })).toBeDisabled()
    expect(screen.getByRole("button", { name: "Release this result session" })).toBeDisabled()
    expect(cancelButton).toBeDisabled()
    const pageState = useDbStore.getState().queryBuckets["panel-profile"]
      .runGroup!.resultPages[resultPageKey(owner)]
    expect(pageState.page).toMatchObject({ lifecycle: "cancelled", hasNext: false })
  })

  it.each([
    ["complete", false],
    ["complete", true],
    ["released", false],
    ["cancelled", false],
    ["error", false],
  ] as const)(
    "disables Cancel for a settled %s page (limit=%s)",
    async (lifecycle, resultLimitReached) => {
      await openWithCachedLifecycle(lifecycle, resultLimitReached)
      render(<DatabasePanel />)

      expect(screen.getByRole("button", { name: "Cancel running query" })).toBeDisabled()
      expect(mockQueryCancel).not.toHaveBeenCalled()
    },
  )
})
