import { useCallback, useMemo, useState } from "react"
import { useTranslation } from "react-i18next"
import { Button } from "@/components/ui/button"
import { cn } from "@/lib/utils"
import { computeGraphLayout, type GraphRow } from "@/workbench/git/graphLayout"
import { CommitGraph } from "../../components/CommitGraph"
import { LoadError } from "../../components/LoadError"
import { Message, Progress } from "../../components/Page"
import { useLoad } from "../../components/useLoad"
import { loadLog } from "../../gitReads"
import { rpcErrorKey } from "../../lib/errors"
import { formatWhen, spaceKey, visibleRefs } from "../../lib/git"
import type { GitLogCommit, GitRef } from "../../lib/protocol"
import { RpcError } from "../../lib/rpc"
import { open, useCompanion } from "../../store"

const ROW_HEIGHT = 56

export function HistoryView({ session, workspaceId, reloadToken }: { session: string; workspaceId: string; reloadToken: number }) {
  const { t } = useTranslation()
  const key = spaceKey(session, workspaceId)
  // Pages already loaded stay: only an empty history, or the refresh button, starts over.
  const load = useCallback(async () => {
    if (!useCompanion.getState().git[key]?.log) await loadLog(session, workspaceId)
  }, [key, session, workspaceId])
  const { error, reload } = useLoad(load, reloadToken)
  const log = useCompanion((state) => state.git[key]?.log)
  const [loadingMore, setLoadingMore] = useState(false)
  const [moreError, setMoreError] = useState<string | null>(null)
  const commits = log?.commits
  const layout = useMemo(() => computeGraphLayout((commits ?? []).map((commit) => ({ hash: commit.hash, parents: commit.parents }))), [commits])

  if (!log) return error ? <LoadError code={error} onRetry={reload} /> : <Progress text={t("home.loading")} />

  const loadMore = async () => {
    setLoadingMore(true)
    setMoreError(null)
    try {
      await loadLog(session, workspaceId, true)
    } catch (failure) {
      if (!(failure instanceof RpcError && failure.code === "disconnected")) setMoreError(failure instanceof RpcError ? failure.code : "unknown")
    } finally {
      setLoadingMore(false)
    }
  }
  return (
    <div className="flex flex-col gap-3">
      {error && <LoadError code={error} onRetry={reload} />}
      {log.commits.length === 0 && <Message>{t("git.noCommits")}</Message>}
      <ul className="overflow-hidden rounded-xl border bg-card">
        {log.commits.map((commit, index) => (
          <li key={commit.hash}>
            <CommitRow commit={commit} row={layout.rows[index]} laneCount={layout.laneCount} onOpen={() => open({ name: "commit", session, workspaceId, hash: commit.hash })} />
          </li>
        ))}
      </ul>
      {moreError && <Message tone="warning">{t(rpcErrorKey(moreError))}</Message>}
      {log.hasMore && (
        <Button variant="outline" className="h-11" disabled={loadingMore} onClick={() => void loadMore()}>
          {loadingMore ? t("git.loadingMore") : t("git.loadMore")}
        </Button>
      )}
    </div>
  )
}

/** One commit: its slice of the lane graph, subject, refs, author and date. Rows touch, so the lanes run unbroken. */
function CommitRow({ commit, row, laneCount, onOpen }: { commit: GitLogCommit; row: GraphRow; laneCount: number; onOpen: () => void }) {
  const { i18n } = useTranslation()
  const refs = visibleRefs(commit.refs)
  return (
    <button type="button" className="flex w-full items-stretch gap-2 pr-3 text-left active:bg-muted" style={{ height: ROW_HEIGHT }} onClick={onOpen}>
      <CommitGraph row={row} laneCount={laneCount} height={ROW_HEIGHT} hasRefs={refs.length > 0} />
      <span className="flex min-w-0 flex-1 flex-col justify-center gap-0.5">
        <span className="truncate text-sm font-medium">{commit.subject}</span>
        <span className="flex items-center gap-1.5 overflow-hidden whitespace-nowrap text-xs text-muted-foreground">
          {refs.slice(0, 2).map((ref) => <RefChip key={`${ref.kind}:${ref.name}`} gitRef={ref} />)}
          {refs.length > 2 && <span className="shrink-0">+{refs.length - 2}</span>}
          <span className="truncate">{commit.authorName} · {formatWhen(commit.timestamp, i18n.language)}</span>
        </span>
      </span>
    </button>
  )
}

const REF_TONE: Record<GitRef["kind"], string> = {
  head: "bg-foreground text-background",
  local: "bg-[color-mix(in_srgb,var(--term-ok)_14%,transparent)] text-[var(--term-ok)]",
  remote: "bg-muted text-muted-foreground",
  tag: "bg-[color-mix(in_srgb,var(--term-amber)_14%,transparent)] text-[var(--term-amber)]",
}

function RefChip({ gitRef }: { gitRef: GitRef }) {
  return <span className={cn("max-w-28 shrink-0 truncate rounded-md px-1.5 py-0.5 font-mono text-[11px] font-medium", REF_TONE[gitRef.kind])}>{gitRef.name}</span>
}
