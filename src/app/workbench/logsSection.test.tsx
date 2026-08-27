import { beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks"

import { LogsSection } from "@/app/workbench/LogsSection"
import { groupRowsByRun } from "@/features/logs/runGroups"

const writeText = vi.fn(async (_value: string) => undefined)
const save = vi.fn(async (_options: unknown) => "/tmp/yuzora-logs.zip")
const openPath = vi.fn(async (_path: string) => undefined)
const homeDir = vi.fn(async () => "/Users/tester")
const join = vi.fn(async (...parts: string[]) => parts.join("/"))

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({
  writeText: (value: string) => writeText(value),
}))

vi.mock("@tauri-apps/plugin-dialog", () => ({
  save: (options: unknown) => save(options),
}))

vi.mock("@tauri-apps/plugin-opener", () => ({
  openPath: (path: string) => openPath(path),
}))

vi.mock("@tauri-apps/api/path", () => ({
  homeDir: () => homeDir(),
  join: (...parts: string[]) => join(...parts),
}))

const sensitiveRow = {
  timestamp: "2026-01-02T03:04:05+08:00",
  run_id: "20260102T030405Z-aaaaaaaa",
  level: "warn",
  kind: "debug",
  source: "ssh",
  workspace_path: "/Users/tester/projects/yuzora",
  event: "connect_failed",
  message: "ssh connection to tester@10.0.0.5:22 failed",
  metadata: {
    host: "10.0.0.5",
    user: "tester",
    fingerprint: "SHA256:9pC2Yl0nQyq4Rm7xTvBd1oKfEuHjAsWn5Zg3MtLbXcU",
  },
}

// 對應 Rust 端 redact_line 的輸出形狀（marker + 保留 basename／port）
const redactedLine = JSON.stringify({
  timestamp: "2026-01-02T03:04:05+08:00",
  run_id: "20260102T030405Z-aaaaaaaa",
  level: "warn",
  kind: "debug",
  source: "ssh",
  workspace_path: "<path:1a2b3c4d>/yuzora",
  event: "connect_failed",
  message: "ssh connection to <user:5e6f7a8b>@<host:9c0d1e2f:private>:22 failed",
  metadata: {
    host: "<host:9c0d1e2f:private>",
    user: "<user:5e6f7a8b>",
    fingerprint: "<fp:3a4b5c6d>",
  },
})

const exportSummary = {
  paths: 7,
  hosts: 3,
  usernames: 2,
  fingerprints: 1,
  secrets: 4,
  unparseable_lines: 5,
}

let sanitizeCalls: string[][] = []
let exportCalls: Array<Record<string, unknown>> = []
let queryResult: unknown[] = []
let sanitizeFails = false

function setupIpc() {
  mockIPC((cmd, args) => {
    const payload = (args ?? {}) as Record<string, unknown>
    if (cmd === "log_sources") return []
    if (cmd === "get_log_level") return "info"
    if (cmd === "log_query") return queryResult
    if (cmd === "log_sanitize_lines") {
      sanitizeCalls.push(payload.lines as string[])
      if (sanitizeFails) throw new Error("log sink unavailable")
      return (payload.lines as string[]).map(() => redactedLine)
    }
    if (cmd === "log_export") {
      exportCalls.push(payload)
      return {
        path: "/tmp/yuzora-logs.zip",
        summary: payload.sanitize === true ? exportSummary : null,
      }
    }
    return undefined
  })
}

beforeEach(() => {
  cleanup()
  clearMocks()
  vi.clearAllMocks()
  sanitizeCalls = []
  exportCalls = []
  queryResult = [sensitiveRow]
  sanitizeFails = false
  setupIpc()
})

async function renderWithRows() {
  render(<LogsSection />)
  await screen.findByTestId("log-row-connect_failed")
}

describe("LogsSection sanitize", () => {
  // AC 7：Copy 與 Export 的 sanitize 語意一致
  it("routes Copy through log_sanitize_lines while the sanitize checkbox is on", async () => {
    await renderWithRows()

    fireEvent.click(screen.getByRole("button", { name: "Copy" }))

    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1))
    expect(sanitizeCalls).toEqual([[JSON.stringify(sensitiveRow)]])
    const copied = writeText.mock.calls[0][0]
    for (const secret of ["10.0.0.5", "tester", "SHA256:", "/Users/"]) {
      expect(copied).not.toContain(secret)
    }
    expect(copied).toContain("<host:9c0d1e2f:private>")
    // 與 raw 模式一致：仍是可讀的 JSON array
    expect(JSON.parse(copied)).toEqual([JSON.parse(redactedLine)])
  })

  it("copies only the 50 rows shown on the active results page", async () => {
    queryResult = Array.from({ length: 100 }, (_, index) => ({
      ...sensitiveRow,
      event: `event_${index}`,
      message: `message ${index}`,
      metadata: { index },
    }))
    render(<LogsSection />)

    expect(await screen.findByText("Page 1 of 2")).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Next results page" }))
    expect(await screen.findByText("Page 2 of 2")).toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "Copy" }))

    await waitFor(() => expect(sanitizeCalls).toHaveLength(1))
    expect(sanitizeCalls[0]).toHaveLength(50)
    const copiedEvents = sanitizeCalls[0].map((line) => JSON.parse(line).event)
    expect(copiedEvents).toEqual(
      Array.from({ length: 50 }, (_, index) => `event_${index + 50}`),
    )
    expect(screen.getByRole("status")).toHaveTextContent("Copied 50 rows (sanitized)")

    fireEvent.click(screen.getByRole("checkbox", { name: "Sanitize" }))
    fireEvent.click(screen.getByRole("button", { name: "Copy" }))

    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(2))
    const rawEvents = (JSON.parse(writeText.mock.calls[1][0]) as Array<{ event: string }>).map(
      (row) => row.event,
    )
    expect(rawEvents).toEqual(
      Array.from({ length: 50 }, (_, index) => `event_${index + 50}`),
    )
    expect(screen.getByRole("status")).toHaveTextContent("Copied 50 rows (raw, not sanitized)")
  })

  // AC 7：sanitize 失敗時 fail-closed——絕不 fallback 成 raw
  it("never falls back to raw rows when log_sanitize_lines rejects", async () => {
    sanitizeFails = true
    await renderWithRows()

    fireEvent.click(screen.getByRole("button", { name: "Copy" }))

    const alert = await screen.findByRole("alert")
    expect(alert.textContent).toContain("Copy failed")
    expect(sanitizeCalls).toEqual([[JSON.stringify(sensitiveRow)]])
    expect(writeText).not.toHaveBeenCalled()
    expect(screen.queryByRole("status")).not.toBeInTheDocument()
  })

  // AC 7：raw mode 是明確的選擇，Copy 不得偷偷 sanitize，也不得偷偷外洩
  it("copies raw rows without calling log_sanitize_lines when sanitize is off", async () => {
    await renderWithRows()

    fireEvent.click(screen.getByRole("checkbox", { name: "Sanitize" }))
    fireEvent.click(screen.getByRole("button", { name: "Copy" }))

    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1))
    expect(sanitizeCalls).toEqual([])
    expect(writeText.mock.calls[0][0]).toContain("10.0.0.5")
    expect(screen.getByRole("status").textContent).toContain("raw")
  })

  // AC 7：raw mode 的風險必須在 UI 上明講
  it("shows a raw-mode warning note only while sanitize is off", async () => {
    await renderWithRows()

    expect(screen.queryByTestId("logs-raw-mode-warning")).not.toBeInTheDocument()

    fireEvent.click(screen.getByRole("checkbox", { name: "Sanitize" }))

    const warning = screen.getByTestId("logs-raw-mode-warning")
    expect(warning).toHaveAttribute("role", "note")
    expect(warning.textContent).toContain("Raw mode")
    expect(warning.textContent).toContain("SSH fingerprints")
    expect(screen.queryByTestId("logs-sanitize-preview")).not.toBeInTheDocument()
  })

  // AC 6：sanitize preview 先講清楚會保留與移除哪些資料類型
  it("previews the kept and removed data classes before any export", async () => {
    await renderWithRows()

    const preview = screen.getByTestId("logs-sanitize-preview")
    expect(preview).toHaveAttribute("role", "note")
    expect(preview.textContent).toContain("Kept: file names, host class, ports")
    expect(preview.textContent).toContain("Removed: full paths, hosts and IPs")
    expect(screen.queryByTestId("logs-sanitize-counts")).not.toBeInTheDocument()
  })

  // AC 6：匯出後補上各類型的實際計數
  it("reports the sanitize summary counts returned by log_export", async () => {
    await renderWithRows()

    fireEvent.click(screen.getByRole("button", { name: "Export bundle" }))

    const counts = await screen.findByTestId("logs-sanitize-counts")
    expect(exportCalls).toEqual([{ dest: "/tmp/yuzora-logs.zip", sanitize: true }])
    expect(counts.textContent).toContain("paths 7")
    expect(counts.textContent).toContain("hosts 3")
    expect(counts.textContent).toContain("accounts 2")
    expect(counts.textContent).toContain("fingerprints 1")
    expect(counts.textContent).toContain("credentials 4")
    expect(counts.textContent).toContain("unreadable lines 5")
    expect(screen.getByRole("status").textContent).toContain("/tmp/yuzora-logs.zip")
  })

  it("drops the stale summary when sanitize is toggled after an export", async () => {
    await renderWithRows()

    fireEvent.click(screen.getByRole("button", { name: "Export bundle" }))
    await screen.findByTestId("logs-sanitize-counts")

    fireEvent.click(screen.getByRole("checkbox", { name: "Sanitize" }))
    fireEvent.click(screen.getByRole("checkbox", { name: "Sanitize" }))

    expect(screen.getByTestId("logs-sanitize-preview")).toBeInTheDocument()
    expect(screen.queryByTestId("logs-sanitize-counts")).not.toBeInTheDocument()
  })
})

// --- issue #40：run 分組（AC 第 1、3 條） ------------------------------------

const runRows = [
  {
    timestamp: "2026-01-02T05:00:00+08:00",
    run_id: "20260102T050000Z-bbbbbbbb",
    level: "info",
    kind: "debug",
    source: "acp",
    workspace_path: null,
    event: "acp_spawn",
    message: "spawned agent-1",
    metadata: { id: "agent-1" },
  },
  {
    timestamp: "2026-01-02T03:00:00+08:00",
    run_id: "20260102T030000Z-aaaaaaaa",
    level: "info",
    kind: "debug",
    source: "acp",
    workspace_path: null,
    event: "acp_spawn",
    message: "spawned agent-1",
    metadata: { id: "agent-1" },
  },
  {
    timestamp: "2026-01-02T02:00:00+08:00",
    run_id: "20260102T030000Z-aaaaaaaa",
    level: "info",
    kind: "app_lifecycle",
    source: "app",
    workspace_path: null,
    event: "app_start",
    message: "app run started",
    metadata: {},
  },
]

const collidingRows = [
  {
    ...runRows[0],
    timestamp: "2026-01-02T06:00:00+08:00",
    run_id: "20260102T060000Z-aaaaaaaa",
    event: "shared_event",
    message: "same visible identity",
    metadata: { owner: "run-a" },
  },
  {
    ...runRows[0],
    timestamp: "2026-01-02T06:00:00+08:00",
    run_id: "20260102T060000Z-bbbbbbbb",
    event: "shared_event",
    message: "same visible identity",
    metadata: { owner: "run-b" },
  },
]

describe("groupRowsByRun", () => {
  it("依 run_id 分組並統計筆數與起訖時間", () => {
    const groups = groupRowsByRun(runRows as never)
    expect(groups.map((group) => group.runId)).toEqual([
      "20260102T050000Z-bbbbbbbb",
      "20260102T030000Z-aaaaaaaa",
    ])
    expect(groups[1].count).toBe(2)
    expect(groups[1].startedAt).toBe("2026-01-02T02:00:00+08:00")
    expect(groups[1].endedAt).toBe("2026-01-02T03:00:00+08:00")
  })

  it("沒有 run_id 的歷史 record 落在單獨一組，不與任何 run 混在一起", () => {
    const groups = groupRowsByRun([
      { ...runRows[0], run_id: null },
      runRows[1],
    ] as never)
    expect(groups).toHaveLength(2)
    expect(groups.map((group) => group.runId)).toContain("—")
  })

  it("空清單回空分組", () => {
    expect(groupRowsByRun([])).toEqual([])
  })
})

describe("LogsSection run grouping", () => {
  it("顯示每個 run 的分組按鈕與筆數", async () => {
    queryResult = runRows
    render(<LogsSection />)
    await screen.findByTestId("logs-run-groups")

    expect(
      screen.getByTestId("logs-run-group-20260102T030000Z-aaaaaaaa").textContent
    ).toContain("run aaaaaaaa · 2")
    expect(
      screen.getByTestId("logs-run-group-20260102T050000Z-bbbbbbbb").textContent
    ).toContain("run bbbbbbbb · 1")
  })

  it("點選某個 run 只留下該 run 的 rows——restart 後重用的 agent-1 因此分得出來", async () => {
    queryResult = runRows
    render(<LogsSection />)
    await screen.findByTestId("logs-run-groups")

    // 兩次 run 都有一筆 `agent-1` 的 acp_spawn。
    expect(screen.getAllByTestId("log-row-acp_spawn")).toHaveLength(2)

    fireEvent.click(screen.getByTestId("logs-run-group-20260102T030000Z-aaaaaaaa"))

    await waitFor(() =>
      expect(screen.getAllByTestId("log-row-acp_spawn")).toHaveLength(1)
    )
    expect(screen.getByTestId("log-row-app_start")).toBeInTheDocument()
  })

  it("再次點選同一個 run 會取消篩選", async () => {
    queryResult = runRows
    render(<LogsSection />)
    await screen.findByTestId("logs-run-groups")

    const button = screen.getByTestId("logs-run-group-20260102T030000Z-aaaaaaaa")
    fireEvent.click(button)
    await waitFor(() => expect(button.getAttribute("aria-pressed")).toBe("true"))
    fireEvent.click(button)

    await waitFor(() =>
      expect(screen.getAllByTestId("log-row-acp_spawn")).toHaveLength(2)
    )
  })

  it("切換 run filter 時不把同位置 row 的 expanded state 帶到另一個 run", async () => {
    queryResult = collidingRows
    render(<LogsSection />)
    await screen.findByTestId("logs-run-groups")

    fireEvent.click(screen.getAllByRole("button", { name: "Expand metadata shared_event" })[0])
    expect(await screen.findByText(/"owner": "run-a"/)).toBeInTheDocument()

    fireEvent.click(screen.getByTestId("logs-run-group-20260102T060000Z-bbbbbbbb"))

    await waitFor(() =>
      expect(screen.getByTestId("log-row-run-shared_event").textContent).toBe("bbbbbbbb")
    )
    expect(
      screen.getByRole("button", { name: "Expand metadata shared_event" }),
    ).toBeInTheDocument()
    expect(screen.queryByText(/"owner": "run-b"/)).not.toBeInTheDocument()
  })

  it("新 query 結果落地時不把舊結果的 expanded state 帶到同位置 row", async () => {
    queryResult = [collidingRows[0]]
    render(<LogsSection />)

    fireEvent.click(await screen.findByRole("button", { name: "Expand metadata shared_event" }))
    expect(await screen.findByText(/"owner": "run-a"/)).toBeInTheDocument()

    queryResult = [
      {
        ...collidingRows[0],
        message: "replacement result",
        metadata: { owner: "replacement" },
      },
    ]
    fireEvent.click(screen.getByRole("button", { name: "audit" }))

    expect(await screen.findByText("replacement result")).toBeInTheDocument()
    expect(
      screen.getByRole("button", { name: "Expand metadata shared_event" }),
    ).toBeInTheDocument()
    expect(screen.queryByText(/"owner": "replacement"/)).not.toBeInTheDocument()
  })

  it("每一列顯示所屬 run 的短代號，歷史 record 顯示 —", async () => {
    queryResult = [runRows[0], { ...runRows[2], run_id: null }]
    render(<LogsSection />)
    await screen.findByTestId("log-row-acp_spawn")

    expect(screen.getByTestId("log-row-run-acp_spawn").textContent).toBe("bbbbbbbb")
    expect(screen.getByTestId("log-row-run-app_start").textContent).toBe("—")
  })

  it("Copy 只複製套用 run 篩選後看得到的 rows", async () => {
    queryResult = runRows
    render(<LogsSection />)
    await screen.findByTestId("logs-run-groups")

    fireEvent.click(screen.getByTestId("logs-run-group-20260102T050000Z-bbbbbbbb"))
    await waitFor(() =>
      expect(screen.getAllByTestId("log-row-acp_spawn")).toHaveLength(1)
    )

    fireEvent.click(screen.getByRole("button", { name: "Copy" }))
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1))

    expect(sanitizeCalls[0]).toHaveLength(1)
    expect(sanitizeCalls[0][0]).toContain("20260102T050000Z-bbbbbbbb")
  })

  it("篩選條件變更後 run 篩選會被清掉，避免留下空清單卻看不出原因", async () => {
    queryResult = runRows
    render(<LogsSection />)
    await screen.findByTestId("logs-run-groups")

    fireEvent.click(screen.getByTestId("logs-run-group-20260102T050000Z-bbbbbbbb"))
    await waitFor(() =>
      expect(screen.getAllByTestId("log-row-acp_spawn")).toHaveLength(1)
    )

    // 換一批結果：舊的 run id 已經不存在於新結果中。若 runFilter 沒被清掉，
    // 新結果會被舊的 run id 篩成空清單。
    queryResult = [runRows[1], runRows[2]]
    fireEvent.click(screen.getByRole("button", { name: "audit" }))

    // 等 debounce 過後的新結果落地。
    await screen.findByTestId("log-row-app_start")
    expect(screen.getAllByTestId("log-row-acp_spawn")).toHaveLength(1)
    expect(
      screen.getByTestId("logs-run-group-20260102T030000Z-aaaaaaaa").getAttribute("aria-pressed")
    ).toBe("false")
  })

  it("kind 篩選含 app_lifecycle，與 Rust 的 VALID_KINDS 一致", async () => {
    queryResult = runRows
    render(<LogsSection />)
    await screen.findByTestId("logs-run-groups")

    expect(screen.getByRole("button", { name: "app_lifecycle" })).toBeInTheDocument()
  })
})
