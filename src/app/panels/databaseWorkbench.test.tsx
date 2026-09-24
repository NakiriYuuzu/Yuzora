import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { DatabaseCatalogPicker } from "./DatabaseCatalogPicker"
import { DatabaseTableActions } from "./DatabaseTableActions"
import { useDbStore } from "@/state/dbStore"
import { dbObjectRefKey } from "@/lib/databaseSql"
import { dbListDatabases } from "@/lib/ipc"
import type { DbTable } from "@/lib/types"

vi.mock("@/lib/ipc", () => ({ dbListDatabases: vi.fn(), dbPostgresTransportChallenge: vi.fn() }))
const initial = useDbStore.getState()
const table: DbTable = { catalog: "app", schema: "public", name: "users", kind: "table" }
beforeEach(() => {
    useDbStore.setState({ ...initial, activeDescriptorId: "profile", saved: [{ id: "profile", configGeneration: 1, targetKey: "test", name: "Test", kind: "postgres", host: "localhost", port: 5432, user: "tester", database: "", transportMode: "verifyFull", credentialState: "stored" }], connections: [{ descriptorId: "profile", connId: "conn", connectionGeneration: "1" as never, kind: "postgres", targetKey: "test", title: "test", name: "Test" }], columnBuckets: { profile: { [dbObjectRefKey(table)]: [{ name: "id", type: "bigint", pk: true, notnull: true }] } } }, true)
    vi.mocked(dbListDatabases).mockResolvedValue(["app", "analytics"])
})
afterEach(() => vi.restoreAllMocks())

it("discovers databases after connecting, then reconnects with the chosen database and stored credential", async () => {
    const update = vi.spyOn(useDbStore.getState(), "updateSaved").mockResolvedValue()
    const open = vi.spyOn(useDbStore.getState(), "openOrReconnectSavedConnection").mockResolvedValue({ outcome: "connected", descriptorId: "profile", connectionId: "conn" } as never)
    render(<DatabaseCatalogPicker descriptorId="profile" />)
    expect(screen.getByRole("combobox")).toHaveTextContent("Select a database")
    fireEvent.keyDown(screen.getByRole("combobox"), { key: "ArrowDown" })
    fireEvent.click(await screen.findByRole("option", { name: "analytics" }))
    await waitFor(() => expect(update).toHaveBeenCalledWith("profile", expect.objectContaining({ database: "analytics", password: "" }), expect.anything()))
    expect(open).toHaveBeenCalledWith("profile")
    expect(dbListDatabases).toHaveBeenCalledWith({ descriptorId: "profile", connectionId: "conn", connectionGeneration: "1" })
})

it("does not reconnect a profile the user switched away from while its update was saving", async () => {
    let finishUpdate!: () => void
    vi.spyOn(useDbStore.getState(), "updateSaved").mockImplementation(() => new Promise<void>(resolve => { finishUpdate = resolve }))
    const open = vi.spyOn(useDbStore.getState(), "openOrReconnectSavedConnection").mockResolvedValue({ outcome: "connected", descriptorId: "profile", connectionId: "conn" } as never)
    render(<DatabaseCatalogPicker descriptorId="profile" />)
    fireEvent.keyDown(screen.getByRole("combobox"), { key: "ArrowDown" })
    fireEvent.click(await screen.findByRole("option", { name: "analytics" }))
    await waitFor(() => expect(finishUpdate).toBeTypeOf("function"))
    act(() => useDbStore.setState({ activeDescriptorId: "other" }))
    await act(async () => { finishUpdate() })
    expect(open).not.toHaveBeenCalled()
})

it("opens the table designer by context menu and applies exactly the SQL shown", async () => {
    const loadColumns = vi.spyOn(useDbStore.getState(), "loadColumns").mockResolvedValue()
    const loadTables = vi.spyOn(useDbStore.getState(), "loadTables").mockResolvedValue()
    const execute = vi.spyOn(useDbStore.getState(), "executeTableStatement").mockResolvedValue()
    render(<DatabaseTableActions descriptorId="profile" table={table}><button>users</button></DatabaseTableActions>)
    fireEvent.contextMenu(screen.getByRole("button", { name: "users" }))
    fireEvent.click(await screen.findByRole("menuitem", { name: "Edit table" }))
    expect(await screen.findByRole("dialog")).toBeVisible()
    expect(loadColumns).toHaveBeenCalledWith("profile", table)
    fireEvent.change(screen.getByRole("textbox", { name: "New name" }), { target: { value: "people" } })
    const sql = 'ALTER TABLE "public"."users" RENAME TO "people"'
    expect(screen.getByText(sql)).toBeVisible()
    expect(execute).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole("button", { name: "Apply change" }))
    await waitFor(() => expect(execute).toHaveBeenCalledWith({ descriptorId: "profile", connectionId: "conn", connectionGeneration: "1" }, sql))
    expect(loadTables).toHaveBeenCalledWith("profile")
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
})

it("keeps the table designer open while cancelling an applying change, then reports the cancellation", async () => {
    vi.spyOn(useDbStore.getState(), "loadColumns").mockResolvedValue()
    let rejectApply!: (reason: unknown) => void
    const execute = vi.spyOn(useDbStore.getState(), "executeTableStatement").mockImplementation(() => new Promise<void>((_, reject) => { rejectApply = reject }))
    const cancel = vi.spyOn(useDbStore.getState(), "cancelTableStatement").mockResolvedValue()
    render(<DatabaseTableActions descriptorId="profile" table={table}><button>users</button></DatabaseTableActions>)
    fireEvent.contextMenu(screen.getByRole("button", { name: "users" }))
    fireEvent.click(await screen.findByRole("menuitem", { name: "Edit table" }))
    fireEvent.change(await screen.findByRole("textbox", { name: "New name" }), { target: { value: "people" } })
    fireEvent.click(screen.getByRole("button", { name: "Apply change" }))
    await waitFor(() => expect(execute).toHaveBeenCalled())
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    expect(cancel).toHaveBeenCalledWith({ descriptorId: "profile", connectionId: "conn", connectionGeneration: "1" })
    expect(screen.getByRole("dialog")).toBeVisible()
    await act(async () => rejectApply(new Error("editCancelled")))
    expect(await screen.findByText("Save was cancelled. Refresh the table to confirm the current data.")).toBeVisible()
    expect(screen.getByRole("dialog")).toBeVisible()
})
