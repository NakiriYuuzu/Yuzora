import { useCallback, useMemo } from "react"
import { useTranslation } from "react-i18next"
import { BackButton } from "../components/BackButton"
import { ConnectionPill } from "../components/ConnectionPill"
import { DiffText } from "../components/DiffText"
import { LoadError } from "../components/LoadError"
import { Message, Page, Progress } from "../components/Page"
import { useLoad } from "../components/useLoad"
import { loadDiff, loadStatus } from "../gitReads"
import { parseUnifiedDiff } from "../lib/diff"
import { splitPath } from "../lib/git"
import type { DiffSource, GitDiff } from "../lib/protocol"

/** What a Conflicted row's blank diff means is for git.status to say, so it is read with the diff and the two arrive together. */
type Loaded = { diff: GitDiff; stillConflicted: boolean }

// The one emptiness test: whitespace-only text is no output, so it is never parsed into a header-less file.
const isBlank = (text: string) => text.trim() === ""

/** One file's changes as a unified diff, fetched fresh each time. */
export function DiffScreen({ session, workspaceId, source }: { session: string; workspaceId: string; source: DiffSource }) {
  const { t } = useTranslation()
  const conflicted = source.kind === "worktree" && source.conflicted === true
  const load = useCallback(async (): Promise<Loaded> => {
    const diff = await loadDiff(session, workspaceId, source)
    if (!conflicted || diff.binary || !isBlank(diff.text)) return { diff, stillConflicted: false }
    // Git printed nothing: either the conflict is gone, or the path never reached git as it is spelled (a lossy name in the list), and only the list itself can tell. This also refreshes the Git tab's cached list.
    const { status } = await loadStatus(session, workspaceId)
    return { diff, stillConflicted: status.conflicted.some((file) => file.path === source.path) }
  }, [session, workspaceId, source, conflicted])
  // A Conflicted row's verdict is only as good as the read behind it, so no earlier result outlives a reload or a retry.
  const { data: loaded, error, reload } = useLoad(load, 0, !conflicted)
  const data = loaded?.diff ?? null
  const blank = loaded !== null && isBlank(loaded.diff.text)
  const parsed = useMemo(() => (data && !blank ? parseUnifiedDiff(data.text) : []), [data, blank])
  // A path git calls unmerged has nothing to show but that fact.
  const files = useMemo(() => parsed.filter((file) => !file.unmerged), [parsed])
  const unmerged = files.length !== parsed.length
  const hunkless = files.every((file) => file.hunks.length === 0)
  // Opened from the Conflicted group, yet git printed no hunk: `--cc` hides the hunks that match a parent, so a file taken whole from one side (or deleted) before `git add` shows a bare combined header. A plain `diff --git` file (say, only a mode change after the conflict was resolved and staged) proves nothing and is shown as any other diff.
  const bareCombined = conflicted && !unmerged && files.length > 0 && hunkless && files.every((file) => file.combined)
  // Blank text for a Conflicted row is settled by the status read made with it; when that read fails, the load error stands alone.
  const blankConflict = conflicted && blank && !loaded.diff.binary
  const resolved = blankConflict && !error && !loaded.stillConflicted
  const matchesSide = bareCombined || (blankConflict && !error && loaded.stillConflicted)
  // With the load in error, a Conflicted row's notice (from the diff text or the status read) is not trusted: the error stands alone, and matchesSide still hides the empty diff.
  const verdict = !(conflicted && error)
  const binary = data !== null && (data.binary || (files.length > 0 && files.every((file) => file.binary)))
  const label =
    source.kind === "commit" ? t("git.diff.source.commit", { hash: source.hash.slice(0, 7) })
    : t(`git.diff.source.${source.kind === "worktree" && source.untracked ? "untracked" : source.kind}`)

  return (
    <Page title={splitPath(source.path).name} leading={<BackButton />} trailing={<ConnectionPill />}>
      <div className="flex flex-col gap-3 px-3 py-3">
        <div className="flex flex-col gap-0.5 px-1">
          <p className="break-all font-mono text-xs">{source.path}</p>
          <p className="text-xs text-muted-foreground">{label}</p>
        </div>
        {error && <LoadError code={error} onRetry={reload} />}
        {!data && !error && <Progress text={t("home.loading")} />}
        {data?.truncated && <Message tone="warning">{t("git.diff.truncated")}</Message>}
        {data && binary && <Message>{t("git.diff.binary")}</Message>}
        {data && unmerged && verdict && <Message tone="warning">{t("git.diff.unmerged")}</Message>}
        {data && matchesSide && verdict && <Message tone="warning">{t("git.diff.unmergedMatched")}</Message>}
        {data && resolved && <Message>{t("git.diff.resolved")}</Message>}
        {data && !binary && !unmerged && !blankConflict && files.length === 0 && <Message>{t("git.diff.empty")}</Message>}
        {data && !binary && !matchesSide && files.length > 0 && <DiffText files={files} />}
        {data && !binary && !matchesSide && files.length > 0 && hunkless && <Message>{t("git.diff.noContentChange")}</Message>}
      </div>
    </Page>
  )
}
