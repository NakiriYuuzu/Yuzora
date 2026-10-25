import type { ReactNode } from "react"
import { useTranslation } from "react-i18next"
import { ChevronRight } from "lucide-react"
import { cn } from "@/lib/utils"
import { splitPath } from "../lib/git"

// Same colors as the desktop's Git file list.
const TONE = {
  add: "text-[var(--term-ok)] bg-[color-mix(in_srgb,var(--term-ok)_14%,transparent)]",
  remove: "text-destructive bg-[color-mix(in_srgb,var(--destructive)_14%,transparent)]",
  change: "text-[var(--term-amber)] bg-[color-mix(in_srgb,var(--term-amber)_14%,transparent)]",
  move: "text-[var(--yz-run)] bg-[color-mix(in_srgb,var(--yz-run)_14%,transparent)]",
  other: "text-muted-foreground bg-muted",
}

/** A conflict is two letters (UU, AA, DU…); everything else is one. */
function tone(status: string): string {
  if (status.length > 1) return TONE.remove
  if (status === "A") return TONE.add
  if (status === "D") return TONE.remove
  if (status === "M" || status === "T") return TONE.change
  if (status === "R" || status === "C") return TONE.move
  return TONE.other
}

/** One changed file: its status letter, name and folder; a tap opens its diff. */
export function GitFileRow({ status, path, oldPath, trailing, onOpen }: {
  status: string
  path: string
  oldPath?: string | null
  trailing?: ReactNode
  onOpen: () => void
}) {
  const { dir, name } = splitPath(path)
  return (
    <button type="button" className="flex min-h-12 w-full items-center gap-3 px-3 py-2 text-left active:bg-muted" onClick={onOpen}>
      <span aria-hidden className={cn("inline-flex h-6 min-w-6 shrink-0 items-center justify-center rounded-md px-1 font-mono text-xs font-semibold", tone(status))}>{status}</span>
      <span className="min-w-0 flex-1">
        <span className="block truncate text-sm font-medium">{name}</span>
        {(oldPath || dir) && <span className="block truncate font-mono text-xs text-muted-foreground">{oldPath ? `← ${oldPath}` : dir}</span>}
      </span>
      {trailing}
      <ChevronRight className="size-4 shrink-0 text-muted-foreground" aria-hidden />
    </button>
  )
}

/** An untracked folder git lists whole because it holds another repository (`vendor/lib/`): there is no diff to open. */
export function NestedRepoRow({ path }: { path: string }) {
  const { t } = useTranslation()
  const { dir, name } = splitPath(path.replace(/\/+$/, ""))
  return (
    <div className="flex min-h-12 w-full items-center gap-3 px-3 py-2">
      <span aria-hidden className={cn("inline-flex h-6 min-w-6 shrink-0 items-center justify-center rounded-md px-1 font-mono text-xs font-semibold", tone("?"))}>?</span>
      <span className="min-w-0 flex-1">
        <span className="block truncate text-sm font-medium">{name}</span>
        {dir && <span className="block truncate font-mono text-xs text-muted-foreground">{dir}</span>}
      </span>
      <span className="shrink-0 rounded-md bg-muted px-1.5 py-0.5 text-xs font-medium text-muted-foreground">{t("git.nestedRepo")}</span>
    </div>
  )
}
