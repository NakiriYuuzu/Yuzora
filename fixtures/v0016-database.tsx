// Production UI and store, deterministic IPC fixture. No external database writes.
import { createRoot } from "react-dom/client"
import { useState } from "react"
import { Button } from "../src/components/ui/button"
import { DatabasePanel } from "../src/app/panels/DatabasePanel"
import { DatabaseNavContent } from "../src/app/workbench/DatabaseNavContent"
import { useDbStore } from "../src/state/dbStore"
import { installDemoRuntime } from "../src/demo/runtime"
import type { DbColumn, DbProfileDescriptor, DbProfileUpdateRequest, DbQueryRunRequest, DbResultPage, DbResultSessionOwner, DbValue } from "../src/lib/types"
import i18n from "../src/lib/i18n"
import "../src/styles.css"
import "../src/editor/editor.css"

installDemoRuntime()
let profile: DbProfileDescriptor = { descriptorId: "demo-postgres" as never, configGeneration: 1, name: "Local development", credentialState: "stored", target: { kind: "postgres", host: "127.0.0.1", port: 5432, user: "developer", database: "", transportMode: "verifyFull" } }
let generation = 0
const columns: DbColumn[] = [
    { name: "id", type: "bigint", pk: true, notnull: true },
    { name: "name", type: "text", pk: false, notnull: true },
    { name: "email", type: "text", pk: false, notnull: true },
    { name: "active", type: "boolean", pk: false, notnull: true },
    { name: "created_at", type: "timestamp", pk: false, notnull: true },
]
const rows: DbValue[][] = Array.from({ length: 1000 }, (_, i) => [{ kind: "integer", value: String(9007199254740993n + BigInt(i)) }, { kind: "text", value: i === 0 ? "Alice Chen" : `User ${i + 1}` }, { kind: "text", value: `user${i + 1}@example.invalid` }, { kind: "boolean", value: i % 3 !== 0 }, { kind: "dateTime", value: "2026-09-22 09:30:00" }])
const events: Array<{ command: string; sql?: string }> = []
const pages = new Map<string, DbResultPage>()
const internals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (command: string, args: Record<string, unknown>) => Promise<unknown> } }).__TAURI_INTERNALS__
const previousInvoke = internals.invoke
internals.invoke = async (command, args) => {
    if (!command.startsWith("db_")) return previousInvoke(command, args)
    events.push({ command })
    if (command === "db_profile_list" || command === "db_profile_import_legacy") return { profiles: [profile], recovery: [] }
    if (command === "db_profile_update") { const request = args.request as DbProfileUpdateRequest; profile = { ...profile, target: request.target, configGeneration: profile.configGeneration + 1 }; return profile }
    if (command === "db_profile_open") return { descriptorId: profile.descriptorId, connectionId: `fixture-${++generation}`, connectionGeneration: String(generation), engine: "postgres" }
    if (command === "db_profile_disconnect") return null
    if (command === "db_list_databases") return ["app_development", "analytics", "staging"]
    if (command === "db_list_tables") return ["users", "projects", "activity_log"].map(name => ({ catalog: "app_development", schema: "public", name, kind: "table" }))
    if (command === "db_table_columns") return columns
    if (command === "db_result_session_release") { const owner = args.owner as DbResultSessionOwner; return { ...pages.get(owner.resultSessionId), lifecycle: "released", hasNext: false, hasPrevious: false } }
    if (command === "db_query_run") {
        const request = args.request as DbQueryRunRequest
        const statements = request.statements.map((statement, index) => {
            events.push({ command, sql: statement.sql })
            const owner = { ...request, statementExecutionId: `${request.queryRunId}:${index}`, resultSessionId: `${request.queryRunId}:${index}:result` } as DbResultSessionOwner
            const select = /^\s*SELECT/i.test(statement.sql)
            const updated = statement.sql.match(/SET "name" = E'((?:''|[^'])*)'/)
            if (updated) rows[0][1] = { kind: "text", value: updated[1].replace(/''/g, "'") }
            const added = statement.sql.match(/ADD "([^"]+)" ([\w()]+) NULL/)
            if (added) { columns.push({ name: added[1], type: added[2], pk: false, notnull: false }); rows.forEach(row => row.push({ kind: "null" })) }
            const page = { owner, pageIndex: 0, columns: columns.map(column => column.name), rows, hasPrevious: false, hasNext: false, lifecycle: "complete", effectOutcome: "none", resultLimitReached: false } satisfies DbResultPage
            pages.set(owner.resultSessionId, page)
            return { statementExecutionId: owner.statementExecutionId, statementIndex: index, sql: statement.sql, effectOutcome: select ? "none" : "committed", result: select ? { kind: "rows", resultSession: { owner, columns: page.columns, initialPage: page }, affectedRows: null } : { kind: "execute", affectedRows: "1" } }
        })
        return { ...request, statements, transactionMayBeOpen: false, connectionTerminated: false }
    }
    throw new Error(`Unhandled fixture IPC: ${command}`)
}
Object.assign(window, { v0016DbEvents: events })
await i18n.changeLanguage("zh-TW")
await useDbStore.getState().initializeProfiles()
await useDbStore.getState().openOrReconnectSavedConnection(profile.descriptorId)

function Workbench() {
    const [sidebar, setSidebar] = useState(true)
    return <main className="flex h-dvh flex-col gap-3 bg-background p-4 text-foreground">
        <header className="flex items-center gap-3"><strong>Database workbench</strong><Button variant="outline" size="sm" onClick={() => document.documentElement.classList.toggle("dark")}>亮／暗</Button><Button variant="outline" size="sm" onClick={() => setSidebar(value => !value)}>側邊欄</Button><span className="text-xs text-muted-foreground">本機驗收資料 · 1,000 rows</span></header>
        <div className="flex min-h-0 flex-1 gap-3">{sidebar && <aside className="w-64 shrink-0 rounded-xl border p-3"><DatabaseNavContent /></aside>}<DatabasePanel /></div>
    </main>
}
const root = createRoot(document.getElementById("root")!)
root.render(<Workbench />)
if (import.meta.hot) import.meta.hot.dispose(() => root.unmount())
