import { create } from "zustand"

import type { DeleteProgress } from "@/lib/types"

export interface DeleteTask {
    name: string
    progress: DeleteProgress | null
    /** Only local deletes report progress and accept a cancel. */
    local: boolean
    cancelling: boolean
}

/** Running deletes by operation id, for their progress toasts. */
export const useDeleteTasks = create<Record<string, DeleteTask>>(() => ({}))

export function patchDeleteTask(id: string, change: Partial<DeleteTask>) {
    useDeleteTasks.setState((tasks) => (tasks[id] ? { [id]: { ...tasks[id], ...change } } : {}))
}

export function removeDeleteTask(id: string) {
    useDeleteTasks.setState((tasks) => {
        const next = { ...tasks }
        delete next[id]
        return next
    }, true)
}
