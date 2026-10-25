import { useCallback, type ReactNode } from "react"
import { useTranslation } from "react-i18next"
import { ArrowDown, ArrowUp, GitBranch } from "lucide-react"
import { GitFileRow, NestedRepoRow } from "../../components/GitFileRow"
import { LoadError } from "../../components/LoadError"
import { Message, Progress } from "../../components/Page"
import { useLoad } from "../../components/useLoad"
import { loadStatus } from "../../gitReads"
import { spaceKey } from "../../lib/git"
import type { DiffSource } from "../../lib/protocol"
import { open, useCompanion } from "../../store"

export function ChangesView({ session, workspaceId, reloadToken }: { session: string; workspaceId: string; reloadToken: number }) {
  const { t } = useTranslation()
  const load = useCallback(() => loadStatus(session, workspaceId), [session, workspaceId])
  const { error, reload } = useLoad(load, reloadToken)
  const loaded = useCompanion((state) => state.git[spaceKey(session, workspaceId)]?.status)
  if (!loaded) return error ? <LoadError code={error} onRetry={reload} /> : <Progress text={t("home.loading")} />

  const { repo, status, truncated } = loaded
  const show = (source: DiffSource) => () => open({ name: "diff", session, workspaceId, source })
  const clean = !status.conflicted.length && !status.staged.length && !status.unstaged.length && !status.untracked.length
  return (
    <div className="flex flex-col gap-4">
      {error && <LoadError code={error} onRetry={reload} />}
      <div className="flex flex-col gap-1 px-1">
        <p className="truncate text-base font-semibold">{repo.name}</p>
        <p className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-muted-foreground">
          <span className="inline-flex items-center gap-1">
            <GitBranch className="size-4" aria-hidden />
            {status.detached ? t("git.detached", { oid: (status.headOid ?? "").slice(0, 7) }) : status.branch ?? t("git.noBranch")}
          </span>
          {status.ahead > 0 && <span className="inline-flex items-center gap-0.5"><ArrowUp className="size-3.5" aria-hidden />{status.ahead}</span>}
          {status.behind > 0 && <span className="inline-flex items-center gap-0.5"><ArrowDown className="size-3.5" aria-hidden />{status.behind}</span>}
        </p>
        {status.inProgress && <p className="text-sm text-[var(--term-amber)]">{t(`git.inProgress.${status.inProgress}`, { defaultValue: status.inProgress })}</p>}
      </div>
      {truncated && <Message tone="warning">{t("git.listTruncated")}</Message>}
      {clean && <Message>{t("git.clean")}</Message>}
      <Group title={t("git.group.conflicted")}>
        {status.conflicted.map((file) => (
          <GitFileRow key={file.path} status={file.status} path={file.path} onOpen={show({ kind: "worktree", path: file.path, untracked: false, conflicted: true })} />
        ))}
      </Group>
      <Group title={t("git.group.staged")}>
        {status.staged.map((file) => (
          <GitFileRow key={file.path} status={file.status} path={file.path} oldPath={file.origPath} onOpen={show({ kind: "staged", path: file.path, origPath: file.origPath })} />
        ))}
      </Group>
      <Group title={t("git.group.unstaged")}>
        {status.unstaged.map((file) => (
          <GitFileRow key={file.path} status={file.status} path={file.path} onOpen={show({ kind: "worktree", path: file.path, untracked: false })} />
        ))}
      </Group>
      <Group title={t("git.group.untracked")}>
        {status.untracked.map((path) =>
          // git lists a nested repository (or a linked worktree) as its folder; the host can't diff that.
          path.endsWith("/")
            ? <NestedRepoRow key={path} path={path} />
            : <GitFileRow key={path} status="?" path={path} onOpen={show({ kind: "worktree", path, untracked: true })} />,
        )}
      </Group>
    </div>
  )
}

function Group({ title, children }: { title: string; children: ReactNode[] }) {
  if (children.length === 0) return null
  return (
    <section className="overflow-hidden rounded-xl border bg-card">
      <h3 className="border-b px-3 py-2 text-sm font-semibold">
        {title}
        <span className="ml-2 text-xs font-normal text-muted-foreground">{children.length}</span>
      </h3>
      <ul className="divide-y">
        {children.map((row, index) => <li key={index}>{row}</li>)}
      </ul>
    </section>
  )
}
