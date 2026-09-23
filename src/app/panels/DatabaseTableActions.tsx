import { useState, type ReactNode } from "react"
import { useTranslation } from "react-i18next"
import { ContextMenu, ContextMenuContent, ContextMenuGroup, ContextMenuItem, ContextMenuTrigger } from "@/components/ui/context-menu"
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Field, FieldDescription, FieldGroup, FieldLabel } from "@/components/ui/field"
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Alert, AlertDescription } from "@/components/ui/alert"
import { ScrollArea } from "@/components/ui/scroll-area"
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table"
import { buildTableEdit, NEW_COLUMN_TYPES, type TableEdit } from "@/lib/databaseEditing"
import { dbObjectRefKey } from "@/lib/databaseSql"
import type { DbConnectionIdentity, DbTable } from "@/lib/types"
import { identityOf, queryFor, useDbStore } from "@/state/dbStore"
import { useOverlayPresence } from "@/state/overlayStore"

export function DatabaseTableActions({ descriptorId, table, children }: { descriptorId: string; table: DbTable; children: ReactNode }) {
    const { t } = useTranslation("databaseWorkbench")
    const connection = useDbStore(state => state.connections.find(item => item.descriptorId === descriptorId))
    const columns = useDbStore(state => state.columnBuckets[descriptorId]?.[dbObjectRefKey(table)])
    const running = useDbStore(state => queryFor(state, descriptorId).running)
    const [identity, setIdentity] = useState<DbConnectionIdentity | null>(null)
    const [menuOpen, setMenuOpen] = useState(false)
    useOverlayPresence(menuOpen || identity !== null)
    const [operation, setOperation] = useState<TableEdit["kind"]>("renameTable")
    const [name, setName] = useState("")
    const [column, setColumn] = useState("")
    const [type, setType] = useState("")
    const [busy, setBusy] = useState(false)
    const [error, setError] = useState(false)
    const kind = connection?.kind ?? "sqlite"
    let statement = ""
    try {
        if (name.trim()) statement = buildTableEdit(kind, table, operation === "renameColumn"
            ? { kind: operation, column, name: name.trim() }
            : operation === "addColumn" ? { kind: operation, name: name.trim(), type: type || NEW_COLUMN_TYPES[kind][0] }
            : { kind: operation, name: name.trim() })
    } catch { /* Incomplete draft. */ }

    async function openDesigner() {
        const captured = identityOf(connection)
        if (!captured || running) return
        setIdentity(captured)
        setName(table.name)
        setOperation("renameTable")
        setColumn("")
        setError(false)
        await useDbStore.getState().loadColumns(descriptorId, table)
    }
    async function save() {
        if (!identity || !statement || busy) return
        setBusy(true)
        setError(false)
        try {
            await useDbStore.getState().executeTableStatement(identity, statement)
            await useDbStore.getState().loadTables(descriptorId)
            const nextTable = operation === "renameTable" ? { ...table, name: name.trim() } : table
            await useDbStore.getState().loadColumns(descriptorId, nextTable)
            const state = useDbStore.getState()
            const current = queryFor(state, descriptorId).table
            if (state.activeDescriptorId === descriptorId && current && dbObjectRefKey(current) === dbObjectRefKey(table)) await state.openTableQuery(nextTable)
            setIdentity(null)
        } catch { setError(true) } finally { setBusy(false) }
    }

    return <>
        <ContextMenu onOpenChange={setMenuOpen}><ContextMenuTrigger asChild>{children}</ContextMenuTrigger><ContextMenuContent><ContextMenuGroup>
            <ContextMenuItem disabled={running} onSelect={() => { useDbStore.getState().setActiveDescriptor(descriptorId); void useDbStore.getState().openTableQuery(table) }}>{t("browseData")}</ContextMenuItem>
            <ContextMenuItem disabled={running || table.kind !== "table"} onSelect={() => void openDesigner()}>{t("editTable")}</ContextMenuItem>
        </ContextMenuGroup></ContextMenuContent></ContextMenu>
        {identity && <Dialog open onOpenChange={open => { if (!open && !busy) setIdentity(null) }}>
            <DialogContent className="sm:max-w-2xl" onEscapeKeyDown={event => { if (busy) event.preventDefault() }} onInteractOutside={event => { if (busy) event.preventDefault() }}>
                <DialogHeader><DialogTitle>{t("tableStructure", { table: table.name })}</DialogTitle><DialogDescription>{table.catalog} / {table.schema} / {table.name}</DialogDescription></DialogHeader>
                <ScrollArea className="max-h-52" orientation="both"><Table><TableHeader><TableRow><TableHead>{t("column")}</TableHead><TableHead>{t("type")}</TableHead><TableHead>{t("constraints")}</TableHead></TableRow></TableHeader><TableBody>
                    {(columns ?? []).map(item => <TableRow key={item.name}><TableCell className="font-mono">{item.name}</TableCell><TableCell className="font-mono">{item.type}</TableCell><TableCell>{item.pk ? "PRIMARY KEY" : item.notnull ? "NOT NULL" : "NULL"}</TableCell></TableRow>)}
                </TableBody></Table></ScrollArea>
                <FieldGroup>
                    <Field><FieldLabel>{t("operation")}</FieldLabel><Select value={operation} disabled={busy} onValueChange={value => { setOperation(value as TableEdit["kind"]); setName("") }}><SelectTrigger aria-label={t("operation")}><SelectValue /></SelectTrigger><SelectContent><SelectGroup>{["renameTable", "renameColumn", "addColumn"].map(value => <SelectItem key={value} value={value}>{t(value)}</SelectItem>)}</SelectGroup></SelectContent></Select></Field>
                    {operation === "renameColumn" && <Field><FieldLabel>{t("column")}</FieldLabel><Select value={column} disabled={busy} onValueChange={setColumn}><SelectTrigger aria-label={t("column")}><SelectValue /></SelectTrigger><SelectContent><SelectGroup>{(columns ?? []).map(item => <SelectItem key={item.name} value={item.name}>{item.name}</SelectItem>)}</SelectGroup></SelectContent></Select></Field>}
                    <Field><FieldLabel htmlFor="database-schema-name">{t("newName")}</FieldLabel><Input id="database-schema-name" value={name} disabled={busy} onChange={event => setName(event.target.value)} /></Field>
                    {operation === "addColumn" && <Field><FieldLabel>{t("type")}</FieldLabel><Select value={type || NEW_COLUMN_TYPES[kind][0]} disabled={busy} onValueChange={setType}><SelectTrigger aria-label={t("type")}><SelectValue /></SelectTrigger><SelectContent><SelectGroup>{NEW_COLUMN_TYPES[kind].map(value => <SelectItem key={value} value={value}>{value}</SelectItem>)}</SelectGroup></SelectContent></Select><FieldDescription>{t("newColumnNullable")}</FieldDescription></Field>}
                </FieldGroup>
                {statement && <ScrollArea className="max-h-28 rounded-md border bg-muted p-3" orientation="both"><pre className="font-mono text-xs">{statement}</pre></ScrollArea>}
                {error && <Alert variant="destructive"><AlertDescription>{t("editFailed")}</AlertDescription></Alert>}
                <DialogFooter><Button variant="outline" disabled={busy} onClick={() => setIdentity(null)}>{t("cancel")}</Button><Button disabled={busy || !statement || (operation === "renameTable" && name.trim() === table.name)} onClick={() => void save()}>{busy ? t("saving") : t("applySchema")}</Button></DialogFooter>
            </DialogContent>
        </Dialog>}
    </>
}
