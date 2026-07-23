import { create } from "zustand"

export const WORKBENCH_LAYOUT_STORAGE_KEY = "yuzora.workbench.layout.v1"
export const WORKBENCH_LAYOUT_VERSION = 1

type TerminalRatioScope = "global" | "workspace"

export interface WorkbenchLayout {
  version: number
  markdownEditorRatio: number
  terminalRatioScope: TerminalRatioScope
  terminalGlobalRatio: number
  terminalWorkspaceRatios: Record<string, number>
}

interface WorkbenchLayoutStore extends WorkbenchLayout {
  setMarkdownEditorRatio: (ratio: number) => void
  effectiveTerminalRatio: (workspacePath: string | null) => number
  setTerminalRatio: (workspacePath: string | null, ratio: number) => void
  setTerminalRatioScope: (scope: TerminalRatioScope, workspacePath: string | null) => void
}

export const workbenchLayoutInitialState: WorkbenchLayout = {
  version: WORKBENCH_LAYOUT_VERSION,
  markdownEditorRatio: 0.5,
  terminalRatioScope: "global",
  terminalGlobalRatio: 0.3,
  terminalWorkspaceRatios: {},
}

function isRatio(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 && value <= 1
}

function isTerminalRatioScope(value: unknown): value is TerminalRatioScope {
  return value === "global" || value === "workspace"
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

function validWorkspaceRatios(value: unknown): Record<string, number> {
  if (!isRecord(value)) return {}

  const entries: Array<[string, number]> = []
  for (const [workspacePath, ratio] of Object.entries(value)) {
    if (workspacePath.length > 0 && isRatio(ratio)) entries.push([workspacePath, ratio])
  }
  return Object.fromEntries(entries)
}

function copyLayout(layout: WorkbenchLayout): WorkbenchLayout {
  return {
    ...layout,
    terminalWorkspaceRatios: { ...layout.terminalWorkspaceRatios },
  }
}

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
      terminalRatioScope: isTerminalRatioScope(parsed.terminalRatioScope)
        ? parsed.terminalRatioScope
        : fallback.terminalRatioScope,
      terminalGlobalRatio: isRatio(parsed.terminalGlobalRatio)
        ? parsed.terminalGlobalRatio
        : fallback.terminalGlobalRatio,
      terminalWorkspaceRatios: validWorkspaceRatios(parsed.terminalWorkspaceRatios),
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
      terminalRatioScope: isTerminalRatioScope(layout.terminalRatioScope)
        ? layout.terminalRatioScope
        : workbenchLayoutInitialState.terminalRatioScope,
      terminalGlobalRatio: isRatio(layout.terminalGlobalRatio)
        ? layout.terminalGlobalRatio
        : workbenchLayoutInitialState.terminalGlobalRatio,
      terminalWorkspaceRatios: validWorkspaceRatios(layout.terminalWorkspaceRatios),
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
    terminalRatioScope: state.terminalRatioScope,
    terminalGlobalRatio: state.terminalGlobalRatio,
    terminalWorkspaceRatios: state.terminalWorkspaceRatios,
  }
}

function resolveTerminalRatio(layout: WorkbenchLayout, workspacePath: string | null): number {
  if (layout.terminalRatioScope !== "workspace" || !workspacePath) {
    return layout.terminalGlobalRatio
  }

  return layout.terminalWorkspaceRatios[workspacePath] ?? layout.terminalGlobalRatio
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

    effectiveTerminalRatio: (workspacePath) => resolveTerminalRatio(layoutFromStore(get()), workspacePath),

    setTerminalRatio: (workspacePath, ratio) => {
      if (!isRatio(ratio)) return

      const current = layoutFromStore(get())
      if (current.terminalRatioScope === "workspace" && workspacePath) {
        if (current.terminalWorkspaceRatios[workspacePath] === ratio) return
        commit({
          ...current,
          terminalWorkspaceRatios: { ...current.terminalWorkspaceRatios, [workspacePath]: ratio },
        })
        return
      }

      if (current.terminalGlobalRatio === ratio) return
      commit({ ...current, terminalGlobalRatio: ratio })
    },

    setTerminalRatioScope: (terminalRatioScope, workspacePath) => {
      const current = layoutFromStore(get())
      if (current.terminalRatioScope === terminalRatioScope) return

      // Without a workspace there is no canonical map key to seed. The effective
      // value is already global, so changing only the scope keeps later workspaces
      // inheriting the global ratio.
      if (!workspacePath) {
        commit({ ...current, terminalRatioScope })
        return
      }

      const effectiveRatio = resolveTerminalRatio(current, workspacePath)
      if (terminalRatioScope === "workspace") {
        commit({
          ...current,
          terminalRatioScope,
          terminalWorkspaceRatios: {
            ...current.terminalWorkspaceRatios,
            [workspacePath]: effectiveRatio,
          },
        })
        return
      }

      commit({
        ...current,
        terminalRatioScope,
        terminalGlobalRatio: effectiveRatio,
      })
    },
  }
})
