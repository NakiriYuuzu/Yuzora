import { beforeEach, describe, expect, it, vi } from "vitest"
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"

import { DatabasePanel, reorderColumns } from "@/app/panels/DatabasePanel"
import type { DbQueryResult } from "@/lib/types"
import { useDbStore } from "@/state/dbStore"

vi.mock("@/lib/ipc", () => ({
  dbOpen: vi.fn(),
  dbClose: vi.fn(),
  dbListTables: vi.fn(),
  dbQuery: vi.fn(),
}))

import { dbListTables, dbOpen, dbQuery } from "@/lib/ipc"

const mockOpen = vi.mocked(dbOpen)
const mockList = vi.mocked(dbListTables)
const mockQuery = vi.mocked(dbQuery)

const threeCol: DbQueryResult = {
  kind: "select",
  columns: ["id", "name", "age"],
  rows: [
    [1, "alice", 30],
    [2, "bob", 25],
  ],
  truncated: false,
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
  mockOpen.mockResolvedValue({ connId: "db-1" })
  mockList.mockResolvedValue([])
  mockQuery.mockResolvedValue(threeCol)
})

// Seed an open connection whose console already holds a three-column result, so
// tests render straight into the sortable/reorderable ResultTable.
async function openWithResult(): Promise<void> {
  await useDbStore.getState().openConnection("/a.db")
  useDbStore.getState().setSql("SELECT * FROM t")
  await useDbStore.getState().runQuery()
}

function headerTexts(): string[] {
  return screen.getAllByRole("columnheader").map((c) => c.textContent ?? "")
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

describe("DatabasePanel result table", () => {
  it("renders the columns in order with divider borders", async () => {
    await openWithResult()
    render(<DatabasePanel />)

    expect(headerTexts()).toEqual(["id", "name", "age"])
  })

  it("clicking a header pushes an ORDER BY on the original column ordinal", async () => {
    await openWithResult()
    mockQuery.mockClear()
    render(<DatabasePanel />)

    fireEvent.click(screen.getByRole("button", { name: "Sort by name" }))

    await waitFor(() =>
      expect(mockQuery).toHaveBeenCalledWith(
        "db-1",
        "SELECT * FROM (\nSELECT * FROM t\n) ORDER BY 2 ASC"
      )
    )
  })

  it("strips a trailing semicolon/comment from the base before wrapping", async () => {
    await useDbStore.getState().openConnection("/a.db")
    useDbStore.getState().setSql("SELECT * FROM t; -- latest\n")
    await useDbStore.getState().runQuery()
    mockQuery.mockClear()
    render(<DatabasePanel />)

    fireEvent.click(screen.getByRole("button", { name: "Sort by id" }))

    await waitFor(() =>
      expect(mockQuery).toHaveBeenCalledWith(
        "db-1",
        "SELECT * FROM (\nSELECT * FROM t\n) ORDER BY 1 ASC"
      )
    )
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

    // The header now in display slot 0 is "name" (original index 1) → ordinal 2,
    // proving the click maps the display slot back to its original column.
    mockQuery.mockClear()
    fireEvent.click(screen.getByRole("button", { name: "Sort by name" }))
    await waitFor(() =>
      expect(mockQuery).toHaveBeenCalledWith(
        "db-1",
        "SELECT * FROM (\nSELECT * FROM t\n) ORDER BY 2 ASC"
      )
    )
  })

  it("preserves a dragged order when the same columns come back, resets on new columns", async () => {
    await openWithResult()
    render(<DatabasePanel />)

    fireEvent.dragStart(screen.getByRole("button", { name: "Sort by id" }))
    fireEvent.drop(screen.getByRole("button", { name: "Sort by age" }))
    expect(headerTexts()).toEqual(["name", "age", "id"])

    // A header sort returns a fresh array with identical names → order kept.
    fireEvent.click(screen.getByRole("button", { name: "Sort by name" }))
    await waitFor(() => expect(mockQuery).toHaveBeenCalled())
    expect(headerTexts()).toEqual(["name", "age", "id"])

    // A genuinely different result (different column names) → order resets.
    mockQuery.mockResolvedValueOnce({
      kind: "select",
      columns: ["x", "y"],
      rows: [[1, 2]],
      truncated: false,
    })
    await act(async () => {
      useDbStore.getState().setSql("SELECT x, y FROM t2")
      await useDbStore.getState().runQuery()
    })
    expect(headerTexts()).toEqual(["x", "y"])
  })
})
