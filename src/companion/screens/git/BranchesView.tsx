import { useCallback } from "react"
import { useTranslation } from "react-i18next"
import { ArrowDown, ArrowUp, Check } from "lucide-react"
import { Disclosure } from "../../components/Disclosure"
import { LoadError } from "../../components/LoadError"
import { Message, Progress } from "../../components/Page"
import { useLoad } from "../../components/useLoad"
import { loadBranches } from "../../gitReads"
import { formatWhen, spaceKey } from "../../lib/git"
import type { GitBranch } from "../../lib/protocol"
import { useCompanion } from "../../store"

export function BranchesView({ session, workspaceId, reloadToken }: { session: string; workspaceId: string; reloadToken: number }) {
  const { t, i18n } = useTranslation()
  const load = useCallback(() => loadBranches(session, workspaceId), [session, workspaceId])
  const { error, reload } = useLoad(load, reloadToken)
  const branches = useCompanion((state) => state.git[spaceKey(session, workspaceId)]?.branches)
  if (!branches) return error ? <LoadError code={error} onRetry={reload} /> : <Progress text={t("home.loading")} />

  // The current branch leads; the rest keep git's order.
  const local = [...branches.local].sort((left, right) => Number(right.isCurrent) - Number(left.isCurrent))
  return (
    <div className="flex flex-col gap-3">
      {error && <LoadError code={error} onRetry={reload} />}
      {branches.truncated && <Message tone="warning">{t("git.listTruncated")}</Message>}
      <section className="overflow-hidden rounded-xl border bg-card">
        <h3 className="border-b px-3 py-2 text-sm font-semibold">
          {t("git.branches.local")}
          <span className="ml-2 text-xs font-normal text-muted-foreground">{local.length}</span>
        </h3>
        {local.length === 0 && <p className="px-3 py-3 text-sm text-muted-foreground">{t("git.branches.none")}</p>}
        <ul className="divide-y">
          {local.map((branch) => <BranchRow key={branch.name} branch={branch} />)}
        </ul>
      </section>
      {branches.remote.length > 0 && (
        <Disclosure title={t("git.branches.remote")} count={branches.remote.length}>
          <ul className="divide-y">
            {branches.remote.map((name) => <li key={name} className="flex min-h-11 items-center px-3 py-2 font-mono text-sm"><span className="truncate">{name}</span></li>)}
          </ul>
        </Disclosure>
      )}
      {branches.tags.length > 0 && (
        <Disclosure title={t("git.branches.tags")} count={branches.tags.length}>
          <ul className="divide-y">
            {branches.tags.map((tag) => {
              const when = Date.parse(tag.date)
              return (
                <li key={tag.name} className="flex min-h-11 items-center gap-3 px-3 py-2">
                  <span className="min-w-0 flex-1 truncate font-mono text-sm">{tag.name}</span>
                  {!Number.isNaN(when) && <span className="shrink-0 text-xs text-muted-foreground">{formatWhen(when / 1000, i18n.language)}</span>}
                </li>
              )
            })}
          </ul>
        </Disclosure>
      )}
    </div>
  )
}

function BranchRow({ branch }: { branch: GitBranch }) {
  const { t } = useTranslation()
  return (
    <li className="flex min-h-12 items-center gap-3 px-3 py-2">
      <span className="flex size-4 shrink-0 items-center justify-center">{branch.isCurrent && <Check className="size-4 text-[var(--term-ok)]" aria-label={t("git.branches.current")} />}</span>
      <span className="min-w-0 flex-1">
        <span className={`block truncate font-mono text-sm ${branch.isCurrent ? "font-semibold" : ""}`}>{branch.name}</span>
        <span className="block truncate text-xs text-muted-foreground">
          {branch.gone ? t("git.branches.gone", { upstream: branch.upstream ?? "" }) : branch.upstream ?? t("git.branches.noUpstream")}
        </span>
      </span>
      {branch.ahead > 0 && <span className="inline-flex shrink-0 items-center gap-0.5 text-xs text-muted-foreground"><ArrowUp className="size-3.5" aria-hidden />{branch.ahead}</span>}
      {branch.behind > 0 && <span className="inline-flex shrink-0 items-center gap-0.5 text-xs text-muted-foreground"><ArrowDown className="size-3.5" aria-hidden />{branch.behind}</span>}
    </li>
  )
}
