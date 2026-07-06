import { open } from "@tauri-apps/plugin-dialog"
import { FolderOpen, Search } from "lucide-react"
import { useEffect, useRef, useState } from "react"

import { DashedActionButton } from "@/app/workbench/DashedActionButton"
import { EmptyState } from "@/app/workbench/EmptyState"
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from "@/components/ui/input-group"
import { clearAll } from "@/editor/documentRegistry"
import { logUserAction } from "@/features/logs/userAction"
import { openWorkspace, searchWorkspace, startWatch } from "@/lib/ipc"
import type { SearchEvent } from "@/lib/types"
import { contextMenuHandler } from "@/state/contextMenuStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { FileTree } from "@/workbench/FileTree"
import { SearchResults } from "@/workbench/search/SearchResults"

/**
 * Files mode nav content — empty state with an "Open workspace" action when
 * no workspace is open (§8 gap 8 baseline, matching the SQL console empty
 * state); once a workspace is set, renders a project-wide search box above the
 * file tree. A non-empty query streams results (250ms debounce) and swaps the
 * tree for the grouped result list; clearing or pressing Esc returns to it.
 */
export function FilesNavContent() {
  const workspacePath = useWorkspaceStore((state) => state.workspacePath)
  const setWorkspace = useWorkspaceStore((state) => state.setWorkspace)
  const [query, setQuery] = useState("")
  const [caseSensitive, setCaseSensitive] = useState(false)
  const [events, setEvents] = useState<SearchEvent[]>([])

  const trimmed = query.trim()
  const hadQuery = useRef(false)

  useEffect(() => {
    if (!workspacePath || trimmed === "") {
      setEvents([])
      // Leaving/clearing search: fire an empty search so the Rust generation
      // advances and any still-running query sees a stale generation and stops
      // (m6, front-end cancellation — no new command). Only when a query was
      // actually active, so first mount doesn't emit a spurious search.
      if (workspacePath && hadQuery.current) {
        void searchWorkspace(workspacePath, "", caseSensitive, () => {})
      }
      hadQuery.current = false
      return
    }
    hadQuery.current = true
    // Clear old results immediately so a previous query's hits don't linger
    // through the debounce when switching queries (T18).
    setEvents([])
    let cancelled = false
    const timer = setTimeout(() => {
      void searchWorkspace(workspacePath, trimmed, caseSensitive, (e) => {
        if (!cancelled) setEvents((prev) => [...prev, e])
      })
    }, 250)
    return () => {
      cancelled = true
      clearTimeout(timer)
    }
  }, [workspacePath, trimmed, caseSensitive])

  async function pickWorkspace() {
    const selected = await open({ directory: true, multiple: false })
    if (typeof selected === "string") {
      const canonical = await openWorkspace(selected)
      // Drop cached document content from the previous workspace so it can't
      // leak into the new one.
      clearAll()
      setWorkspace(canonical)
      void startWatch(canonical)
      void logUserAction("open_workspace", `open workspace ${canonical}`)
    }
  }

  if (!workspacePath) {
    return (
      <div onContextMenu={contextMenuHandler("explorer")} className="flex h-full flex-col gap-[10px]">
        <div className="flex flex-1 items-center justify-center">
          <EmptyState
            icon={FolderOpen}
            title="No files yet"
            description="Open a project to browse its files"
          />
        </div>
        <DashedActionButton label="Open workspace" onClick={pickWorkspace} />
      </div>
    )
  }

  return (
    <div onContextMenu={contextMenuHandler("explorer")} className="flex h-full flex-col">
      <div className="px-[8px] pt-[6px] pb-[4px]">
        <InputGroup className="h-[28px]">
          <InputGroupAddon>
            <Search className="size-[14px] text-(--ink-3)" aria-hidden="true" />
          </InputGroupAddon>
          <InputGroupInput
            placeholder="Search in workspace"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") setQuery("")
            }}
            className="text-[12.5px]"
          />
          <InputGroupAddon align="inline-end">
            <InputGroupButton
              type="button"
              aria-label="Match case"
              aria-pressed={caseSensitive}
              onClick={() => setCaseSensitive((v) => !v)}
              className={caseSensitive ? "bg-(--yz-active) text-(--ink-0)" : "text-(--ink-3)"}
            >
              Aa
            </InputGroupButton>
          </InputGroupAddon>
        </InputGroup>
      </div>
      {trimmed !== "" ? (
        <div className="flex-1 overflow-y-auto">
          <SearchResults events={events} query={trimmed} />
        </div>
      ) : (
        <div className="flex-1 overflow-y-auto py-[4px]">
          <FileTree />
        </div>
      )}
    </div>
  )
}
