import { useMemo, useState, type ReactNode } from "react"
import { useTranslation } from "react-i18next"
import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog"
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field"
import { Textarea } from "@/components/ui/textarea"
import { Alert, AlertDescription } from "@/components/ui/alert"
import { buildCellUpdate, isBinaryDbColumn, parseEditedDbValue } from "@/lib/databaseEditing"
import { dbObjectRefKey } from "@/lib/databaseSql"
import { formatDbValue, type DbColumn, type DbConnectionIdentity, type DbKind, type DbTable, type DbValue } from "@/lib/types"
import { queryFor, useDbStore } from "@/state/dbStore"
import { useOverlayPresence } from "@/state/overlayStore"
import { CellEditingContext, type EditingContext } from "./databaseCellEditingContext"

export function DatabaseCellEditing({ children, identity, kind, table, metadata, disabled }: {
    children: ReactNode; identity: DbConnectionIdentity | null; kind: DbKind; table: DbTable | null; metadata: DbColumn[]; disabled: boolean
}) {
    const { t } = useTranslation("databaseWorkbench")
    const [editing, setEditing] = useState<{ identity: DbConnectionIdentity; table: DbTable; kind: DbKind; metadata: DbColumn[]; columns: string[]; row: DbValue[]; column: DbColumn } | null>(null)
    const [text, setText] = useState("")
    const [isNull, setIsNull] = useState(false)
    const [busy, setBusy] = useState(false)
    const [error, setError] = useState<string | null>(null)
    useOverlayPresence(editing !== null)
    const context = useMemo<EditingContext | null>(() => {
        if (!identity || !table || table.kind !== "table" || disabled || !metadata.some(column => column.pk)) return null
        const byName = new Map(metadata.map(column => [column.name, column]))
        return {
            editable: (name, value) => {
                const column = byName.get(name)
                return column !== undefined && !isBinaryDbColumn(column) && value.kind !== "binary" && value.kind !== "json"
            },
            edit: (columns, row, name) => {
                const original = row[columns.indexOf(name)]
                const column = byName.get(name)
                if (!original || !column) return
                try { buildCellUpdate(kind, table, metadata, columns, row, name, original) } catch { return }
                setEditing({ identity, table, kind, metadata, columns, row, column })
                setText(formatDbValue(original) ?? "")
                setIsNull(original.kind === "null")
                setError(null)
            },
        }
    }, [identity, kind, table, metadata, disabled])

    async function save() {
        if (!editing || busy) return
        setBusy(true)
        setError(null)
        try {
            const original = editing.row[editing.columns.indexOf(editing.column.name)]
            const value = parseEditedDbValue(original, text, isNull, editing.column)
            const statement = buildCellUpdate(editing.kind, editing.table, editing.metadata, editing.columns, editing.row, editing.column.name, value)
            await useDbStore.getState().executeTableStatement(editing.identity, statement, "1")
            setEditing(null)
            const state = useDbStore.getState()
            const current = queryFor(state, editing.identity.descriptorId).table
            if (state.activeDescriptorId === editing.identity.descriptorId && current && dbObjectRefKey(current) === dbObjectRefKey(editing.table)) await state.openTableQuery(editing.table)
        } catch (failure) {
            const code = failure instanceof Error ? failure.message : "editFailed"
            setError(["invalidValue", "notNullable", "editConflict", "staleConnection", "connectionBusy", "editUncertain", "editCancelled"].includes(code) ? code : "editFailed")
        } finally { setBusy(false) }
    }

    // While saving, Cancel stops the pending write; the dialog stays open to
    // report whether it was cancelled or had already completed.
    function cancel() {
        if (!busy) setEditing(null)
        else if (editing) void useDbStore.getState().cancelTableStatement(editing.identity)
    }

    return <CellEditingContext value={context}>
        {children}
        <Dialog open={editing !== null} onOpenChange={open => { if (!open && !busy) setEditing(null) }}>
            <DialogContent onInteractOutside={event => { if (busy) event.preventDefault() }} onEscapeKeyDown={event => { if (busy) event.preventDefault() }}>
                <DialogHeader><DialogTitle>{t("editCell", { column: editing?.column.name })}</DialogTitle><DialogDescription>{t("editCellDescription", { table: editing?.table.name })}</DialogDescription></DialogHeader>
                <FieldGroup>
                    <Field><FieldLabel htmlFor="database-cell-value">{t("value")}</FieldLabel><Textarea id="database-cell-value" value={text} onChange={event => setText(event.target.value)} disabled={busy || isNull} className="min-h-32 font-mono" autoFocus /></Field>
                    <Field orientation="horizontal"><Checkbox id="database-cell-null" checked={isNull} onCheckedChange={checked => setIsNull(checked === true)} disabled={busy || editing?.column.notnull || editing?.column.pk} /><FieldLabel htmlFor="database-cell-null">NULL</FieldLabel></Field>
                </FieldGroup>
                {error && <Alert variant="destructive"><AlertDescription>{t(error)}</AlertDescription></Alert>}
                <DialogFooter><Button variant="outline" onClick={cancel}>{t("cancel")}</Button><Button disabled={busy} onClick={() => void save()}>{busy ? t("saving") : t("save")}</Button></DialogFooter>
            </DialogContent>
        </Dialog>
    </CellEditingContext>
}
