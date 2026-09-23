import { createContext, useContext } from "react"
import type { DbValue } from "@/lib/types"

export interface EditingContext {
    edit: (columns: string[], row: DbValue[], name: string) => void
    editable: (name: string, value: DbValue) => boolean
}
export const CellEditingContext = createContext<EditingContext | null>(null)
export const useDatabaseCellEditing = () => useContext(CellEditingContext)
