import { create } from "zustand"

export const WORKBENCH_LAYOUT_STORAGE_KEY = "yuzora.workbench.layout.v1"
export const WORKBENCH_LAYOUT_VERSION = 1


export interface WorkbenchLayout {
  version: number
  markdownEditorRatio: number
}

interface WorkbenchLayoutStore extends WorkbenchLayout {
  setMarkdownEditorRatio: (ratio: number) => void
}

export const workbenchLayoutInitialState: WorkbenchLayout = {
  version: WORKBENCH_LAYOUT_VERSION,
  markdownEditorRatio: 0.5,
}

function isRatio(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 && value <= 1
}


function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}


function copyLayout(layout: WorkbenchLayout): WorkbenchLayout { return { ...layout } }

/** Load each persisted field independently so one stale preference cannot discard the rest. */
export function loadWorkbenchLayout(): WorkbenchLayout {
  const fallback = copyLayout(workbenchLayoutInitialState)

  try {
    const raw = localStorage.getItem(WORKBENCH_LAYOUT_STORAGE_KEY)
    if (!raw) return fallback

    const parsed: unknown = JSON.parse(raw)
    if (!isRecord(parsed)) return fallback

    return {
      // Unknown versions are upgraded to the current schema while valid fields retain
      // their independent fallbacks. Future migrations can branch here by version.
      version: parsed.version === WORKBENCH_LAYOUT_VERSION
        ? WORKBENCH_LAYOUT_VERSION
        : fallback.version,
      markdownEditorRatio: isRatio(parsed.markdownEditorRatio)
        ? parsed.markdownEditorRatio
        : fallback.markdownEditorRatio,
    }
  } catch {
    return fallback
  }
}

/** The sole localStorage write path. It whitelists the persisted schema and never throws. */
function persistWorkbenchLayout(layout: WorkbenchLayout): void {
  try {
    const guarded: WorkbenchLayout = {
      version: WORKBENCH_LAYOUT_VERSION,
      markdownEditorRatio: isRatio(layout.markdownEditorRatio)
        ? layout.markdownEditorRatio
        : workbenchLayoutInitialState.markdownEditorRatio,
    }

    localStorage.setItem(WORKBENCH_LAYOUT_STORAGE_KEY, JSON.stringify(guarded))
  } catch {
    // Private mode or quota errors leave the reactive in-memory state authoritative.
  }
}

function layoutFromStore(state: WorkbenchLayoutStore): WorkbenchLayout {
  return {
    version: state.version,
    markdownEditorRatio: state.markdownEditorRatio,
  }
}


export const useWorkbenchLayoutStore = create<WorkbenchLayoutStore>()((set, get) => {
  const commit = (next: WorkbenchLayout) => {
    set(next)
    persistWorkbenchLayout(next)
  }

  return {
    ...loadWorkbenchLayout(),

    setMarkdownEditorRatio: (markdownEditorRatio) => {
      if (!isRatio(markdownEditorRatio)) return

      const current = layoutFromStore(get())
      if (current.markdownEditorRatio === markdownEditorRatio) return
      commit({ ...current, markdownEditorRatio })
    },

  }
})
