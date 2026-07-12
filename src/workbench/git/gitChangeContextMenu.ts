import type { MouseEvent as ReactMouseEvent } from "react"

import type { GitChangeTarget } from "@/app/workbench/contextMenuModel"
import { useContextMenuStore } from "@/state/contextMenuStore"
import { useGitStore } from "@/state/gitStore"
import { useUiStore } from "@/state/uiStore"

import {
    currentGitChanges,
    type GitChangeKey,
    type GitChangeRow
} from "./gitChangeSelection"

function target(key: GitChangeKey): GitChangeTarget {
    return {
        path: key.path,
        staged: key.staged,
        classification: key.classification,
        stagedStatus: key.stagedStatus,
        unstagedStatus: key.unstagedStatus,
        origPath: key.origPath
    }
}

export function openGitChangeContextMenu(
    event: ReactMouseEvent,
    clicked: GitChangeRow,
    rows: readonly GitChangeRow[]
) {
    event.preventDefault()
    event.stopPropagation()
    const environment = useGitStore.getState().environment
    if (environment?.status !== "ready") return

    const ui = useUiStore.getState()
    ui.ensureGitChangeContextSelection(clicked)
    const selected = currentGitChanges(useUiStore.getState().gitChangeSelection, rows)
    useContextMenuStore.getState().open({
        kind: "gitChange",
        repositoryRoot: environment.root,
        clicked: target(clicked),
        selected: (selected.length > 0 ? selected : [clicked]).map(target)
    }, event.clientX, event.clientY)
}
