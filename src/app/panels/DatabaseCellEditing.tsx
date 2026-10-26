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
import { useUiStore } from "@/state/uiStore"
import { useOverlayPresence } from "@/state/overlayStore"
import { CellEditingContext, type EditingContext } from "./databaseCellEditingContext"

function hasLiveOwner(identity: DbConnectionIdentity, state = useDbStore.getState()): boolean {
    return state.connections.some(connection => connection.descriptorId === identity.descriptorId
        && connection.connId === identity.connectionId
        && connection.connectionGeneration === identity.connectionGeneration)
}

export function DatabaseCellEditing({ children, identity, profileName, kind, table, metadata, disabled }: {
    children: ReactNode; identity: DbConnectionIdentity | null; profileName: string; kind: DbKind; table: DbTable | null; metadata: DbColumn[]; disabled: boolean
}) {
    const { t } = useTranslation("databaseWorkbench")
    const [editing, setEditing] = useState<{ identity: DbConnectionIdentity; profileName: string; table: DbTable; kind: DbKind; metadata: DbColumn[]; columns: string[]; row: DbValue[]; column: DbColumn } | null>(null)
    const [text, setText] = useState("")
    const [isNull, setIsNull] = useState(false)
    const [busy, setBusy] = useState(false)
    const [error, setError] = useState<string | null>(null)
    const visible = useUiStore(state => state.mode === "database")
    const liveOwner = useDbStore(state => editing !== null && hasLiveOwner(editing.identity, state))
    useOverlayPresence(editing !== null && visible)
    const context = useMemo<EditingContext | null>(() => {
        if (editing || !identity || !table || table.kind !== "table" || disabled || !metadata.some(column => column.pk)) return null
        const byName = new Map(metadata.map(column => [column.name, column]))
        return {
            editable: (name, value) => {
                const column = byName.get(name)
                return column !== undefined && column.generated !== true && !isBinaryDbColumn(column) && value.kind !== "binary" && value.kind !== "json"
            },
            edit: (columns, row, name) => {
                const original = row[columns.indexOf(name)]
                const column = byName.get(name)
                if (!original || !column) return
                try { buildCellUpdate(kind, table, metadata, columns, row, name, original) } catch { return }
                setEditing({ identity, profileName, table, kind, metadata, columns, row, column })
                setText(formatDbValue(original) ?? "")
                setIsNull(original.kind === "null")
                setError(null)
            },
        }
    }, [identity, profileName, kind, table, metadata, disabled, editing])

    async function save() {
        if (!editing || busy) return
        if (!hasLiveOwner(editing.identity)) {
            setError(current => current === "editUncertain" ? current : "staleConnection")
            return
        }
        setBusy(true)
        setError(null)
        try {
            const original = editing.row[editing.columns.indexOf(editing.column.name)]
            const value = parseEditedDbValue(editing.kind, original, text, isNull, editing.column)
            const statement = buildCellUpdate(editing.kind, editing.table, editing.metadata, editing.columns, editing.row, editing.column.name, value)
            await useDbStore.getState().executeTableStatement(editing.identity, statement, "1")
            setEditing(null)
            const state = useDbStore.getState()
            const current = queryFor(state, editing.identity.descriptorId).table
            if (hasLiveOwner(editing.identity, state) && state.activeDescriptorId === editing.identity.descriptorId && current && dbObjectRefKey(current) === dbObjectRefKey(editing.table)) await state.openTableQuery(editing.table)
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

    const message = error ?? (editing && !liveOwner ? "staleConnection" : null)
    return <CellEditingContext value={context}>
        {children}
        <Dialog open={editing !== null && visible} onOpenChange={open => { if (!open && !busy && visible) setEditing(null) }}>
            <DialogContent
                onInteractOutside={event => { if (busy) event.preventDefault() }}
                onEscapeKeyDown={event => { if (busy) event.preventDefault() }}
                // Select-all outside a text field (e.g. the NULL checkbox) would highlight the whole page behind the dialog.
                onKeyDown={event => {
                    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "a" && !(event.target instanceof HTMLElement && event.target.matches("input:not([type=checkbox]), textarea, [contenteditable]"))) event.preventDefault()
                }}
            >
                <DialogHeader><DialogTitle>{t("editCell", { column: editing?.column.name })}</DialogTitle><DialogDescription>{t("editCellDescription", { table: editing?.table.name, profile: editing?.profileName })}</DialogDescription></DialogHeader>
                <FieldGroup>
                    <Field><FieldLabel htmlFor="database-cell-value">{t("value")}</FieldLabel><Textarea id="database-cell-value" value={text} onChange={event => setText(event.target.value)} disabled={busy || isNull} readOnly={!liveOwner} className="min-h-32 font-mono" autoFocus /></Field>
                    <Field orientation="horizontal"><Checkbox id="database-cell-null" checked={isNull} onCheckedChange={checked => setIsNull(checked === true)} disabled={busy || !liveOwner || editing?.column.notnull || editing?.column.pk} /><FieldLabel htmlFor="database-cell-null">NULL</FieldLabel></Field>
                </FieldGroup>
                {message && <Alert variant="destructive"><AlertDescription>{t(message)}</AlertDescription></Alert>}
                <DialogFooter><Button variant="outline" onClick={cancel}>{t("cancel")}</Button><Button disabled={busy || !liveOwner} onClick={() => void save()}>{busy ? t("saving") : t("save")}</Button></DialogFooter>
            </DialogContent>
        </Dialog>
    </CellEditingContext>
}
