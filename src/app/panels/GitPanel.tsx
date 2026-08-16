import { useState } from "react"
import { useTranslation } from "react-i18next"

import { Button } from "@/components/ui/button"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { GitGuidedSetup } from "@/app/workbench/GitNavContent"
import { EmptyState } from "@/app/workbench/EmptyState"
import { contextMenuHandler } from "@/state/contextMenuStore"
import { commitLikeFrom, useDiffModalStore } from "@/state/diffModalStore"
import { changedPathSet, useGitStore } from "@/state/gitStore"
import { useGitLogStore } from "@/state/gitLogStore"
import { useUiStore, type GitPanelTab } from "@/state/uiStore"
import { gitFetch, gitPull, gitPush } from "@/lib/ipc"
import type { CommitFileChange } from "@/lib/types"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { BranchPopover } from "@/workbench/git/BranchPopover"
import { ConflictBanner } from "@/workbench/git/ConflictBanner"
import { ConsoleTab } from "@/workbench/git/ConsoleTab"
import { LocalChangesTab } from "@/workbench/git/LocalChangesTab"
import { LogTab } from "@/workbench/git/LogTab"
import { FolderGit2, MoreHorizontal } from "lucide-react"

/**
 * Git mode main region. Only the ready environment mounts Log/Local/Console.
 * Tab selection is store-backed so FileTree/ConflictBanner can land on Local.
 */
export function GitPanel() {
  const { t } = useTranslation("menus")
  const environment = useGitStore((s) => s.environment)
  const status = useGitStore((s) => s.status)
  const lastError = useGitStore((s) => s.lastError)
  const detect = useGitStore((s) => s.detect)
  const workspacePath = useWorkspaceStore((s) => s.workspacePath)

  return (
    <div
      onContextMenu={contextMenuHandler({
        kind: "git",
        repositoryRoot: environment?.status === "ready" ? environment.root : null,
      })}
      className="yz-modein @container/git-panel flex min-h-0 flex-1 flex-col overflow-hidden rounded-(--r-lg) border border-(--line-1) bg-(--paper-0) shadow-(--shadow-lg)"
    >
      {environment?.status === "missing" ? (
        <GitGuidedSetup
          reason={environment.reason}
          kind={environment.kind}
          minimumVersion={environment.minimumVersion}
        />
      ) : environment?.status === "ready" && status ? (
        <GitPanelTabs />
      ) : environment?.status === "notARepo" ? (
        <div className="flex h-full items-center justify-center p-[16px]">
          <EmptyState
            icon={FolderGit2}
            title={t("gitPanel.notARepoTitle")}
            description={t("gitPanel.notARepoDescription")}
          />
        </div>
      ) : (
        <div className="flex h-full flex-col items-center justify-center gap-[10px] p-[16px] text-center">
          <EmptyState
            icon={FolderGit2}
            title={lastError ? t("gitPanel.errorTitle") : t("gitPanel.detectingTitle")}
            description={
              lastError
                ? t("gitPanel.errorDescription", { message: lastError })
                : t("gitPanel.detectingDescription")
            }
          />
          {workspacePath && lastError && (
            <Button
              type="button"
              onClick={() => void detect(workspacePath)}
              className="rounded-[8px] bg-(--yz-solid) px-[11px] py-[5px] text-[11.5px] font-semibold text-(--ink-0) shadow-(--shadow-xs) hover:bg-(--yz-hover)"
            >
              {t("gitPanel.retry")}
            </Button>
          )}
        </div>
      )}
    </div>
  )
}

function GitPanelTabs() {
  const { t } = useTranslation("menus")
  const status = useGitStore((s) => s.status)
  const repositoryRoot = useGitStore((s) =>
    s.environment?.status === "ready" ? s.environment.root : null
  )
  const openCommit = useDiffModalStore((s) => s.openCommit)
  const gitPanelTab = useUiStore((s) => s.gitPanelTab)
  const setGitPanelTab = useUiStore((s) => s.setGitPanelTab)

  const totalCount = new Set([
    ...(status?.staged.map((entry) => entry.path) ?? []),
    ...changedPathSet(status)
  ]).size

  function openCommitDiff(hash: string, file?: CommitFileChange) {
    const s = useGitLogStore.getState()
    if (s.selectedHash !== hash) return
    const commit = s.commits.find((c) => c.hash === hash)
    const detail = s.detail
    if (!repositoryRoot || !commit || !detail) return
    const index = file ? detail.files.findIndex((f) => f.path === file.path) : 0
    openCommit(repositoryRoot, commitLikeFrom(commit, detail), index < 0 ? 0 : index)
  }

  return (
    <>
      <ConflictBanner />
      <Tabs
        value={gitPanelTab}
        onValueChange={(value) => setGitPanelTab(value as GitPanelTab)}
        className="min-h-0 flex-1 gap-0"
      >
        <div
          data-testid="git-panel-toolbar"
          className="flex h-[43px] min-w-0 shrink-0 items-center gap-[4px] overflow-hidden border-b border-(--line-1) px-[10px]"
        >
          <TabsList variant="line" aria-label={t("gitPanel.viewsAriaLabel")} className="min-w-0 flex-1">
            <TabsTrigger value="log" className="min-w-0 flex-1">{t("gitPanel.tabLog")}</TabsTrigger>
            <TabsTrigger value="local" className="min-w-0 flex-1">
              <span data-testid="git-panel-local-label" className="min-w-0 flex-1 truncate">{t("gitPanel.tabLocal")}</span>
              {totalCount > 0 && (
                <span
                  data-testid="git-panel-local-count"
                  className="shrink-0 whitespace-nowrap rounded-(--r-pill) bg-(--amber-soft) px-[6px] py-[1px] font-mono text-[9.5px] font-semibold"
                  style={{ color: "#9a6512" }}
                >
                  {totalCount}
                </span>
              )}
            </TabsTrigger>
            <TabsTrigger value="console" className="min-w-0 flex-1">{t("gitPanel.tabConsole")}</TabsTrigger>
          </TabsList>
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

function GitTabActions() {
  const { t } = useTranslation("menus")
  const environment = useGitStore((s) => s.environment)
  const status = useGitStore((s) => s.status)
  const busy = useGitStore((s) => s.busy)
  const runOp = useGitStore((s) => s.runOp)
  const snapshotStale = useGitStore((s) => s.snapshotStale)

  const [branchOpen, setBranchOpen] = useState(false)

  if (environment?.status !== "ready") return null

  const branchName = status?.detached
    ? status.headOid.slice(0, 7)
    : (status?.branch ?? "main")

  const branchPill = (
    <Button
      type="button"
      variant="outline"
      size="sm"
      aria-label={t("gitPanel.branchesAriaLabel")}
      title={branchName}
      aria-busy={busy != null || undefined}
      aria-expanded={branchOpen}
      aria-haspopup="dialog"
      className="flex h-[28px] min-w-0 max-w-[72px] shrink cursor-pointer items-center gap-[7px] rounded-(--r-pill) border border-(--line-1) bg-(--yz-solid) pr-[11px] pl-[9px] text-[11.5px] text-(--ink-1) shadow-(--shadow-xs) transition-colors hover:bg-(--paper-1) disabled:cursor-not-allowed disabled:opacity-50 @min-[720px]/git-panel:max-w-[180px]"
      onClick={() => setBranchOpen((v) => !v)}
    >
      <span
        aria-hidden="true"
        className="size-[8px] shrink-0 rounded-full"
        style={{ background: "#3b6fe0" }}
      />
      <span className="min-w-0 truncate font-mono font-medium">{branchName}</span>
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
        className={`shrink-0 transition-transform ${branchOpen ? "rotate-180" : ""}`}
      >
        <path d="m6 9 6 6 6-6" />
      </svg>
    </Button>
  )

  return (
    <div className="flex min-w-0 items-center gap-[2px]">
      <BranchPopover open={branchOpen} onOpenChange={setBranchOpen} trigger={branchPill} />
      <div className="hidden items-center gap-[2px] @min-[720px]/git-panel:flex">
        <GitActionButton
          label={t("branchPopover.fetch", { ns: "menus" })}
          busy={busy}
          stale={snapshotStale}
          onClick={() => runOp("fetch", () => gitFetch(environment.root, false))}
        >
          <path d="M3 12a9 9 0 1 0 3-6.7L3 8" />
          <path d="M3 3v5h5" />
        </GitActionButton>
        <GitActionButton
          label={t("branchPopover.pull", { ns: "menus" })}
          busy={busy}
          stale={snapshotStale}
          onClick={() => runOp("pull", () => gitPull(environment.root))}
        >
          <path d="M12 3v12M7 10l5 5 5-5M5 21h14" />
        </GitActionButton>
        <GitActionButton
          label={t("branchPopover.push", { ns: "menus" })}
          busy={busy}
          stale={snapshotStale}
          onClick={() => runOp("push", () => gitPush(environment.root))}
        >
          <path d="M12 21V9M7 14l5-5 5 5M5 3h14" />
        </GitActionButton>
      </div>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            aria-label={t("gitPanel.moreActionsAriaLabel")}
            disabled={busy != null || snapshotStale}
            className="flex size-[30px] shrink-0 items-center justify-center rounded-[9px] text-(--ink-3) transition-all duration-150 hover:bg-(--paper-2) hover:text-(--ink-1) disabled:opacity-50 @min-[720px]/git-panel:hidden"
          >
            <MoreHorizontal className="size-[15px]" aria-hidden="true" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" className="min-w-[140px]">
          <DropdownMenuItem
            disabled={busy != null || snapshotStale}
            onSelect={() => void runOp("fetch", () => gitFetch(environment.root, false))}
          >
            {t("branchPopover.fetch", { ns: "menus" })}
          </DropdownMenuItem>
          <DropdownMenuItem
            disabled={busy != null || snapshotStale}
            onSelect={() => void runOp("pull", () => gitPull(environment.root))}
          >
            {t("branchPopover.pull", { ns: "menus" })}
          </DropdownMenuItem>
          <DropdownMenuItem
            disabled={busy != null || snapshotStale}
            onSelect={() => void runOp("push", () => gitPush(environment.root))}
          >
            {t("branchPopover.push", { ns: "menus" })}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  )
}

function GitActionButton({
  label,
  busy,
  stale,
  onClick,
  children
}: {
  label: string
  busy: string | null
  stale: boolean
  onClick: () => void
  children: React.ReactNode
}) {
  return (
    <Button
      type="button"
      variant="ghost"
      size="icon-sm"
      title={label}
      aria-label={label}
      disabled={busy != null || stale}
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
    </Button>
  )
}
