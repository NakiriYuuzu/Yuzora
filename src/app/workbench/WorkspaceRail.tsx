import { FolderPlus, PanelLeft, Plus, Settings } from "lucide-react"
import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { open } from "@tauri-apps/plugin-dialog"

import { cn } from "@/lib/utils"
import {
  HoverCard,
  HoverCardContent,
  HoverCardTrigger
} from "@/components/ui/hover-card"
import { ScrollArea } from "@/components/ui/scroll-area"
import { resolveProjectPresentation } from "@/app/workbench/projectPresentation"
import { resolveSpaceTabCount } from "@/lib/workbenchTabReorder"
import { canonicalPathKey, workspacePathBasename } from "@/lib/paths"
import { contextMenuHandler } from "@/state/contextMenuStore"
import { useHerdrStore } from "@/state/herdrStore"
import { useRecentWorkspacesStore } from "@/state/recentWorkspaces"
import { useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

interface WorkspaceRailProps {
  navCollapsed: boolean
  onToggleNav: () => void
  onOpenSettings: () => void
  terminalOpen: boolean
  onToggleTerminalDrawer: () => void
}

const RAIL_BUTTON_CLASS =
  "flex h-[32px] w-[38px] items-center justify-center rounded-[10px] transition-all duration-[160ms] ease-(--ease-out) hover:bg-(--yz-hover) hover:text-(--yz-accent-ink)"
const RAIL_IDLE_CLASS = "text-(--ink-3)"
const RAIL_ACTIVE_CLASS = "bg-(--yz-hover) text-(--yz-accent-ink)"

/**
 * Activity rail — global controls stay; middle list is the selected named
 * session's Herdr Spaces (not Recent folders). "+" creates/focuses a Herdr
 * workspace via public `workspace.create` for the running session.
 */
export function WorkspaceRail({
  navCollapsed,
  onToggleNav,
  onOpenSettings,
  terminalOpen,
  onToggleTerminalDrawer
}: WorkspaceRailProps) {
  const { t } = useTranslation("workbench")
  const removedNotice = useUiStore((s) => s.recentWorkspaceRemovedNotice)
  const clearRemovedNotice = useUiStore((s) => s.clearRecentWorkspaceRemovedNotice)
  const workspacePath = useWorkspaceStore((s) => s.workspacePath)
  const presentations = useRecentWorkspacesStore((s) => s.presentations)
  const spaces = useHerdrStore((s) => s.snapshot?.spaces)
  const selectedSpaceId = useHerdrStore((s) => s.selectedSpaceId)
  const selectedSessionName = useHerdrStore((s) => s.selectedSessionName)
  const connectionState = useHerdrStore((s) => s.connectionState)
  const canMutate = useHerdrStore((s) => s.canMutateSelectedSession())
  const createSpaceFromFolder = useHerdrStore((s) => s.createSpaceFromFolder)
  const activateSpace = useHerdrStore((s) => s.activateSpace)
  const agents = useHerdrStore((s) => s.snapshot?.agents)
  const tabs = useHerdrStore((s) => s.snapshot?.tabs)

  const spaceList = spaces ?? []

  const agentCounts = useMemo(() => {
    const map = new Map<string, { total: number; running: number }>()
    for (const agent of agents ?? []) {
      const current = map.get(agent.workspaceId) ?? { total: 0, running: 0 }
      current.total += 1
      if (agent.status === "working" || agent.status === "blocked") {
        current.running += 1
      }
      map.set(agent.workspaceId, current)
    }
    return map
  }, [agents])

  const [notice, setNotice] = useState<{ message: string; danger: boolean } | null>(null)
  const [creating, setCreating] = useState(false)
  const noticeTimer = useRef<number | null>(null)

  useEffect(
    () => () => {
      if (noticeTimer.current !== null) clearTimeout(noticeTimer.current)
    },
    []
  )

  const showNotice = useCallback((message: string, danger = true) => {
    setNotice({ message, danger })
    if (noticeTimer.current !== null) clearTimeout(noticeTimer.current)
    noticeTimer.current = window.setTimeout(() => setNotice(null), 4000)
  }, [])

  useEffect(() => {
    if (!removedNotice) return
    const timer = window.setTimeout(clearRemovedNotice, 4000)
    return () => clearTimeout(timer)
  }, [clearRemovedNotice, removedNotice])

  const visibleNotice = removedNotice
    ? {
        message: t("rail.removedFromRecent", { name: removedNotice.name }),
        danger: false
      }
    : notice

  const mutationsDisabled = !canMutate || connectionState === "stopped" || creating

  async function handleActivateSpace(spaceId: string, path?: string | null) {
    if (!selectedSessionName || !canMutate) return
    const result = await activateSpace({
      sessionName: selectedSessionName,
      workspaceId: spaceId,
      path
    })
    if (!result.ok && result.error && !result.cancelled) {
      showNotice(result.error)
    }
  }

  async function handleNewSpace() {
    if (mutationsDisabled || !selectedSessionName) return
    setCreating(true)
    try {
      const selected = await open({ directory: true, multiple: false })
      if (typeof selected !== "string") return
      const label = workspacePathBasename(selected)
      // Single guarded transaction: unsaved preflight → workspace.create →
      // preapproved local switch → commit selection. Cancel never creates.
      const result = await createSpaceFromFolder(selected, label)
      if (!result.ok) {
        if (result.cancelled) return
        const reason =
          result.error ??
          useHerdrStore.getState().errorMessage ??
          t("rail.newSpaceFailedUnknown")
        showNotice(t("rail.newSpaceFailed", { error: reason }))
      }
    } catch (e) {
      showNotice(t("rail.newSpaceFailed", { error: String(e) }))
    } finally {
      setCreating(false)
    }
  }

  return (
    <nav
      aria-label={t("rail.ariaLabel")}
      onContextMenu={contextMenuHandler({ kind: "rail" })}
      className="flex w-[68px] shrink-0 flex-col items-center gap-[5px] pt-[13px] pb-[11px]"
    >
      <button
        type="button"
        aria-label={t("rail.toggleSidebar")}
        aria-pressed={!navCollapsed}
        title={t("rail.toggleSidebar")}
        onClick={onToggleNav}
        className={cn(
          "flex h-[32px] w-[38px] items-center justify-center rounded-[10px] transition-all duration-[160ms] ease-(--ease-out) hover:bg-(--yz-hover) hover:text-(--yz-accent-ink)",
          navCollapsed ? "text-(--ink-3)" : "bg-(--yz-hover) text-(--yz-accent-ink)"
        )}
      >
        <PanelLeft className="size-[17px]" aria-hidden="true" />
      </button>

      <button
        type="button"
        aria-label={t("rail.toggleTerminal")}
        aria-pressed={terminalOpen}
        title={t("rail.toggleTerminalTitle")}
        onClick={onToggleTerminalDrawer}
        className={cn(RAIL_BUTTON_CLASS, terminalOpen ? RAIL_ACTIVE_CLASS : RAIL_IDLE_CLASS)}
      >
        <svg
          width="16"
          height="16"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.8"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
        >
          <path d="m5 8 4 4-4 4M13 16h6" />
        </svg>
      </button>

      <div aria-hidden="true" className="my-[4px] h-px w-[24px] bg-(--line-1)" />
      <div className="text-[9px] font-medium uppercase tracking-[0.12em] text-(--ink-3)">
        {t("rail.spaces")}
      </div>
      <ScrollArea
        data-testid="rail-spaces-scroll"
        className={cn(
          "min-h-0 w-full flex-1",
          // Compact B: 68px rail − 58px tile = 5px side margins when centered.
          // Keep this rail's vertical scrollbar ≤5px so it sits in the right
          // blank margin instead of shifting content flush-left with pr-10.
          "[&_[data-slot=scroll-area-scrollbar][data-orientation=vertical]]:w-[5px]",
          "[&_[data-slot=scroll-area-scrollbar][data-orientation=vertical]]:p-0",
          "[&_[data-slot=scroll-area-scrollbar][data-orientation=vertical]]:border-l-0"
        )}
        contentClassName="flex flex-col items-center gap-[5px] pt-[3px]"
      >
        {spaceList.map((space) => {
          const active =
            space.id === selectedSpaceId ||
            Boolean(
              !selectedSpaceId &&
                space.path &&
                workspacePath &&
                canonicalPathKey(space.path) === canonicalPathKey(workspacePath)
            )
          const counts = agentCounts.get(space.id)
          // Trim only decides blankness; never rewrite a valid operational path
          // before presentation lookup or resolveProjectPresentation.
          const rawPath = typeof space.path === "string" ? space.path : ""
          const hasPath = rawPath.trim().length > 0
          const fallbackName =
            (typeof space.label === "string" ? space.label.trim() : "") ||
            (typeof space.id === "string" ? space.id.trim() : "") ||
            "S"
          const identity = hasPath
            ? resolveProjectPresentation(
                rawPath,
                presentations[canonicalPathKey(rawPath)]
              )
            : {
                name: fallbackName,
                glyph: fallbackName.charAt(0).toUpperCase() || "S",
                color: null
              }
          // Accessible open name prefers trimmed label, then id, never blank.
          const openName = fallbackName
          const statusLabel = space.status ?? "unknown"
          const provenanceKind =
            space.isLinkedWorktree === true
              ? "linked"
              : space.isLinkedWorktree === false
                ? "source"
                : null
          const branchLabel = space.isDetached
            ? t("rail.spaceDetached")
            : space.branch
              ? space.branch
              : null
          const resolvedTabCount = resolveSpaceTabCount(space, tabs)
          const openAriaLabel = (() => {
            const baseLabel = (() => {
              if (provenanceKind === "linked") {
                return t("rail.openLinkedWorktreeSpace", {
                  name: openName,
                  branch: branchLabel ?? t("rail.spaceNoBranch"),
                  repo: space.repoName ?? space.repoRoot ?? ""
                })
              }
              if (provenanceKind === "source") {
                return t("rail.openSourceCheckoutSpace", {
                  name: openName,
                  branch: branchLabel ?? t("rail.spaceNoBranch"),
                  repo: space.repoName ?? space.repoRoot ?? ""
                })
              }
              return t("rail.openSpace", { name: openName })
            })()
            return `${baseLabel}, ${t("rail.tabCount", { count: resolvedTabCount })}`
          })()
          return (
            <HoverCard key={space.id} openDelay={250} closeDelay={100}>
              <HoverCardTrigger asChild>
                <button
                  type="button"
                  data-testid={`rail-space-${space.id}`}
                  aria-label={openAriaLabel}
                  aria-pressed={active}
                  aria-disabled={!canMutate}
                  data-worktree-kind={provenanceKind ?? undefined}
                  data-worktree-branch={space.branch ?? undefined}
                  data-worktree-detached={space.isDetached ? "true" : undefined}
                  onClick={() => void handleActivateSpace(space.id, space.path)}
                  onContextMenu={contextMenuHandler({
                    kind: "herdrSpace",
                    sessionName: selectedSessionName ?? "",
                    workspaceId: space.id,
                    label: space.label,
                    path: space.path ?? null
                  })}
                  className={cn(
                    "relative flex w-[58px] min-h-[51px] shrink-0 flex-col items-center justify-center gap-[3px] rounded-[10px] px-0 py-[4px] transition-all duration-[160ms] ease-(--ease-out) focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-(--yz-accent)/50",
                    active ? "bg-(--yz-hover)" : "hover:bg-(--yz-hover)/70",
                    !canMutate && "cursor-not-allowed opacity-50"
                  )}
                >
                  {active && (
                    <span
                      aria-hidden="true"
                      className="absolute left-0 top-1/2 h-[20px] w-[3px] -translate-y-1/2 rounded-full bg-(--yz-accent)"
                    />
                  )}
                  <span className="relative">
                    <span
                      aria-hidden="true"
                      data-testid={`rail-space-glyph-${space.id}`}
                      className={cn(
                        "flex size-[34px] shrink-0 items-center justify-center rounded-[10px] text-[13px] font-semibold shadow-(--shadow-xs)",
                        !identity.color &&
                          "border border-(--line-1) bg-(--yz-field) text-(--ink-2)",
                        active && "ring-2 ring-(--yz-accent)/45"
                      )}
                      style={
                        identity.color
                          ? {
                              background: identity.color.background,
                              color: identity.color.foreground
                            }
                          : undefined
                      }
                    >
                      {identity.glyph}
                    </span>
                    {counts && counts.total > 0 && (
                      <span
                        aria-label={t("rail.agentCount", {
                          total: counts.total,
                          running: counts.running
                        })}
                        className={cn(
                          "absolute -right-[3px] -top-[3px] flex h-[15px] min-w-[15px] items-center justify-center rounded-full px-[3px] text-[9px] font-semibold leading-none text-white",
                          counts.running > 0 ? "bg-(--yz-accent)" : "bg-(--ink-4)"
                        )}
                      >
                        {counts.running > 0 ? `${counts.running}/${counts.total}` : counts.total}
                      </span>
                    )}
                    {resolvedTabCount > 0 && (
                      <span
                        data-testid={`rail-space-tab-count-${space.id}`}
                        aria-label={t("rail.tabCount", { count: resolvedTabCount })}
                        className="absolute -bottom-[3px] -right-[3px] flex h-[15px] min-w-[15px] items-center justify-center rounded-full bg-(--ink-2) px-[3px] text-[9px] font-semibold leading-none text-white"
                      >
                        {resolvedTabCount}
                      </span>
                    )}
                  </span>
                  <span
                    data-testid={`rail-space-label-${space.id}`}
                    className={cn(
                      "w-[54px] truncate text-center text-[8.5px] font-semibold leading-none",
                      active ? "text-(--ink-0)" : "text-(--ink-3)"
                    )}
                  >
                    {identity.name}
                  </span>
                </button>
              </HoverCardTrigger>
              <HoverCardContent
                side="right"
                align="start"
                className="w-[260px]"
                data-testid={`rail-space-card-${space.id}`}
              >
                <div className="flex flex-col gap-1.5">
                  <div
                    className="break-words text-[13px] font-semibold text-(--ink-0)"
                    data-testid={`rail-space-card-label-${space.id}`}
                  >
                    {space.label}
                  </div>
                  <div
                    className="break-all font-mono text-[11px] text-(--ink-3)"
                    data-testid={`rail-space-card-path-${space.id}`}
                  >
                    {space.path ?? t("rail.spaceNoPath")}
                  </div>
                  <div className="text-[11px] text-(--ink-3)">
                    {t("rail.spaceSession", {
                      name: selectedSessionName ?? "—"
                    })}
                  </div>
                  {provenanceKind && (
                    <div
                      className="text-[11px] text-(--ink-3)"
                      data-testid={`rail-space-card-provenance-${space.id}`}
                    >
                      {provenanceKind === "linked"
                        ? t("rail.spaceLinkedWorktree")
                        : t("rail.spaceSourceCheckout")}
                      {space.repoName
                        ? ` · ${t("rail.spaceRepo", { name: space.repoName })}`
                        : space.repoRoot
                          ? ` · ${t("rail.spaceRepo", { name: space.repoRoot })}`
                          : ""}
                      {space.isDetached
                        ? ` · ${t("rail.spaceDetached")}`
                        : space.branch
                          ? ` · ${t("rail.spaceBranch", { branch: space.branch })}`
                          : ""}
                    </div>
                  )}
                  <div className="text-[11px] text-(--ink-3)">
                    {t("rail.spaceAgents", {
                      total: counts?.total ?? 0,
                      running: counts?.running ?? 0,
                      status: statusLabel
                    })}
                  </div>
                  <div className="text-[11px] text-(--ink-3)">
                    {t("rail.spaceTabs", { count: resolvedTabCount })}
                  </div>
                </div>
              </HoverCardContent>
            </HoverCard>
          )
        })}
      </ScrollArea>

      <button
        type="button"
        data-testid="rail-new-space"
        aria-label={t("rail.newSpace")}
        title={
          mutationsDisabled
            ? t("rail.newSpaceUnavailable")
            : t("rail.newSpace")
        }
        onClick={() => void handleNewSpace()}
        disabled={mutationsDisabled}
        className="flex size-[38px] items-center justify-center rounded-[11px] border-[1.5px] border-dashed border-(--line-2) text-(--ink-3) transition-all duration-[180ms] ease-(--ease-spring) hover:border-(--yz-accent)/60 hover:bg-(--yz-hover) hover:text-(--yz-accent-ink) disabled:pointer-events-none disabled:opacity-50"
      >
        {creating ? (
          <Plus className="size-[16px] animate-pulse" aria-hidden="true" />
        ) : (
          <FolderPlus className="size-[16px]" aria-hidden="true" />
        )}
      </button>

      <button
        type="button"
        aria-label={t("rail.settings")}
        title={t("rail.settings")}
        onClick={onOpenSettings}
        className="flex size-[32px] items-center justify-center rounded-[10px] text-(--ink-3) transition-colors duration-150 hover:bg-(--yz-hover) hover:text-(--yz-accent-ink)"
      >
        <Settings className="size-[17px]" aria-hidden="true" />
      </button>

      {visibleNotice && (
        <div
          role="status"
          className="fixed bottom-[16px] left-[84px] z-50 max-w-[280px] rounded-[10px] border border-(--line-1) px-[12px] py-[8px] text-[11px] shadow-[var(--shadow-xl)]"
          style={
            visibleNotice.danger
              ? { background: "var(--danger-soft)", color: "var(--status-d)" }
              : { background: "var(--frost-light)", color: "var(--ink-1)" }
          }
        >
          {visibleNotice.message}
        </div>
      )}
    </nav>
  )
}
