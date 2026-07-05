import { useState } from "react"

import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { GitGuidedSetup } from "@/app/workbench/GitNavContent"
import { contextMenuHandler } from "@/state/contextMenuStore"
import { commitLikeFrom, useDiffModalStore } from "@/state/diffModalStore"
import { changedPathSet, useGitStore } from "@/state/gitStore"
import { useGitLogStore } from "@/state/gitLogStore"
import { gitFetch, gitPull, gitPush } from "@/lib/ipc"
import type { CommitFileChange } from "@/lib/types"
import { BranchPopover } from "@/workbench/git/BranchPopover"
import { ConflictBanner } from "@/workbench/git/ConflictBanner"
import { ConsoleTab } from "@/workbench/git/ConsoleTab"
import { LocalChangesTab } from "@/workbench/git/LocalChangesTab"
import { LogTab } from "@/workbench/git/LogTab"

/**
 * Git mode main region — design reference §2. Log, Local changes and Console
 * tabs are all live now. defaultValue is "log" so the panel opens on the commit
 * history (design §2 default). When the git executable is missing the whole
 * region becomes a guided setup instead of the tabbed view.
 */
export function GitPanel() {
  const environment = useGitStore((s) => s.environment)

  return (
    <div
      onContextMenu={contextMenuHandler("git")}
      className="yz-modein flex min-h-0 flex-1 flex-col overflow-hidden rounded-(--r-lg) border border-(--line-1) bg-(--paper-0) shadow-(--shadow-lg)"
    >
      {environment?.status === "missing" ? (
        <GitGuidedSetup reason={environment.reason} />
      ) : (
        <GitPanelTabs />
      )}
    </div>
  )
}

function GitPanelTabs() {
  const status = useGitStore((s) => s.status)
  const changedCount = changedPathSet(status).size
  const openCommit = useDiffModalStore((s) => s.openCommit)

  // Resolve the selected commit + its loaded detail from the log store, then
  // open the Diff modal on it. onOpenFile lands on the clicked file; onCompare
  // opens the whole commit (activeIndex 0). Both require detail to be loaded —
  // the wiring passes undefined otherwise so the surfaces stay inert.
  function openCommitDiff(hash: string, file?: CommitFileChange) {
    const s = useGitLogStore.getState()
    // Guard against the stale-flash window: if the selection has moved on since
    // the row/Compare was rendered, s.detail belongs to a different commit than
    // `hash`. Bail rather than open the modal with mismatched hash + files.
    if (s.selectedHash !== hash) return
    const commit = s.commits.find((c) => c.hash === hash)
    const detail = s.detail
    if (!commit || !detail) return
    const index = file ? detail.files.findIndex((f) => f.path === file.path) : 0
    openCommit(commitLikeFrom(commit, detail), index < 0 ? 0 : index)
  }

  return (
    <>
      <ConflictBanner />
      <Tabs defaultValue="log" className="min-h-0 flex-1 gap-0">
        <div className="flex h-[43px] shrink-0 items-center gap-[4px] border-b border-(--line-1) px-[10px]">
          <TabsList variant="line" aria-label="Git views">
            <TabsTrigger value="log">
              {/* §2 L726 git-graph icon, stroke #3b6fe0 */}
              <svg
                width="14"
                height="14"
                viewBox="0 0 24 24"
                fill="none"
                stroke="#3b6fe0"
                strokeWidth="1.9"
                strokeLinecap="round"
                strokeLinejoin="round"
                aria-hidden="true"
              >
                <circle cx="6" cy="6" r="2.4" />
                <circle cx="6" cy="18" r="2.4" />
                <circle cx="18" cy="8" r="2.4" />
                <path d="M6 8.4v7.2M18 10.4a6 6 0 0 1-6 6H8.4" />
              </svg>
              Log
            </TabsTrigger>
            <TabsTrigger value="local">
              Local changes
              {/* §2 L730 amber changed-count pill (hidden at 0) */}
              {changedCount > 0 && (
                <span
                  className="rounded-(--r-pill) bg-(--amber-soft) px-[6px] py-[1px] font-mono text-[9.5px] font-semibold"
                  style={{ color: "#9a6512" }}
                >
                  {changedCount}
                </span>
              )}
            </TabsTrigger>
            <TabsTrigger value="console">Console</TabsTrigger>
          </TabsList>
          <div className="flex-1" />
          <GitTabActions />
        </div>

        <TabsContent value="log" className="flex min-h-0 flex-1 overflow-hidden">
          <LogTab
            onOpenFile={(hash, file) => openCommitDiff(hash, file)}
            onCompare={(hash) => openCommitDiff(hash)}
          />
        </TabsContent>

        <TabsContent value="local" className="flex min-h-0 flex-1 overflow-hidden">
          <LocalChangesTab />
        </TabsContent>

        <TabsContent value="console" className="flex min-h-0 flex-1 overflow-hidden">
          <ConsoleTab />
        </TabsContent>
      </Tabs>
    </>
  )
}

/**
 * §2 L734-740 tab-strip right cluster: branch pill (opens BranchPopover) +
 * Fetch / Pull / Push icon buttons. Only rendered when the repo is ready; the
 * three ops route through the same gitStore.runOp path the popover uses and are
 * disabled together while any op is busy.
 */
function GitTabActions() {
  const environment = useGitStore((s) => s.environment)
  const status = useGitStore((s) => s.status)
  const busy = useGitStore((s) => s.busy)
  const runOp = useGitStore((s) => s.runOp)

  const [branchOpen, setBranchOpen] = useState(false)

  if (environment?.status !== "ready") return null

  const branchName = status?.detached
    ? status.headOid.slice(0, 7)
    : (status?.branch ?? "main")

  // §2 L734 branch pill — h28, rounded pill, solid track + line border, dot +
  // mono name + chevron; opens the shared BranchPopover.
  const branchPill = (
    <button
      type="button"
      aria-label="Branches"
      className="flex h-[28px] cursor-pointer items-center gap-[7px] rounded-(--r-pill) border border-(--line-1) bg-(--yz-solid) pr-[11px] pl-[9px] text-[11.5px] text-(--ink-1) shadow-(--shadow-xs) transition-colors hover:bg-(--paper-1)"
      onClick={() => setBranchOpen((v) => !v)}
    >
      <span
        aria-hidden="true"
        className="size-[8px] shrink-0 rounded-full"
        style={{ background: "#3b6fe0" }}
      />
      <span className="font-mono font-medium">{branchName}</span>
      <svg
        width="12"
        height="12"
        viewBox="0 0 24 24"
        fill="none"
        stroke="var(--ink-3)"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
      >
        <path d="m6 9 6 6 6-6" />
      </svg>
    </button>
  )

  return (
    <>
      <BranchPopover open={branchOpen} onOpenChange={setBranchOpen} trigger={branchPill} />
      <GitActionButton
        label="Fetch"
        busy={busy}
        onClick={() => runOp("fetch", () => gitFetch(false))}
      >
        <path d="M3 12a9 9 0 1 0 3-6.7L3 8" />
        <path d="M3 3v5h5" />
      </GitActionButton>
      <GitActionButton label="Pull" busy={busy} onClick={() => runOp("pull", () => gitPull())}>
        <path d="M12 3v12M7 10l5 5 5-5M5 21h14" />
      </GitActionButton>
      <GitActionButton label="Push" busy={busy} onClick={() => runOp("push", () => gitPush())}>
        <path d="M12 21V9M7 14l5-5 5 5M5 3h14" />
      </GitActionButton>
    </>
  )
}

// §2 L738-740 30×30 icon button — r9, ink-3, hover paper-2/ink-1, disabled while
// any op is busy (dimmed). svg is 15px, stroke-width 1.9 (design).
function GitActionButton({
  label,
  busy,
  onClick,
  children
}: {
  label: string
  busy: string | null
  onClick: () => void
  children: React.ReactNode
}) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      disabled={busy != null}
      onClick={onClick}
      className="flex size-[30px] shrink-0 items-center justify-center rounded-[9px] text-(--ink-3) transition-all duration-150 hover:bg-(--paper-2) hover:text-(--ink-1) disabled:opacity-50 disabled:hover:bg-transparent disabled:hover:text-(--ink-3)"
    >
      <svg
        width="15"
        height="15"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.9"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
      >
        {children}
      </svg>
    </button>
  )
}
