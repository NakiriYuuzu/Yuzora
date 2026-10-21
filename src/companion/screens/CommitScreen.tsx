import { useCallback } from "react"
import { useTranslation } from "react-i18next"
import { Button } from "@/components/ui/button"
import { BackButton } from "../components/BackButton"
import { ConnectionPill } from "../components/ConnectionPill"
import { GitFileRow } from "../components/GitFileRow"
import { LoadError } from "../components/LoadError"
import { Message, Page, Progress } from "../components/Page"
import { useLoad } from "../components/useLoad"
import { loadCommit } from "../gitReads"
import { formatWhen, spaceKey } from "../lib/git"
import { open, useCompanion } from "../store"

/** One commit: message, author, parents and the files it changed; a file opens its diff. */
export function CommitScreen({ session, workspaceId, hash }: { session: string; workspaceId: string; hash: string }) {
  const { t, i18n } = useTranslation()
  const load = useCallback(() => loadCommit(session, workspaceId, hash), [session, workspaceId, hash])
  const { error, reload } = useLoad(load)
  const commit = useCompanion((state) => state.git[spaceKey(session, workspaceId)]?.commits[hash])

  return (
    <Page title={hash.slice(0, 7)} leading={<BackButton />} trailing={<ConnectionPill />}>
      <div className="flex flex-col gap-3 px-3 py-3">
        {error && <LoadError code={error} onRetry={reload} />}
        {!commit && !error && <Progress text={t("home.loading")} />}
        {commit && (
          <>
            <div className="flex flex-col gap-2 px-1">
              <h2 className="text-base font-semibold [overflow-wrap:anywhere]">{commit.subject}</h2>
              {commit.body && <p className="whitespace-pre-wrap text-sm text-muted-foreground [overflow-wrap:anywhere]">{commit.body}</p>}
              <p className="text-xs text-muted-foreground">{commit.authorName} · {formatWhen(commit.timestamp, i18n.language, true)}</p>
              <p className="break-all font-mono text-xs text-muted-foreground">{commit.hash}</p>
            </div>
            {commit.parents.length > 0 && (
              <div className="flex flex-wrap items-center gap-2">
                <span className="px-1 text-xs text-muted-foreground">{t("git.commit.parents")}</span>
                {commit.parents.map((parent) => (
                  <Button key={parent} variant="outline" className="h-11 font-mono" onClick={() => open({ name: "commit", session, workspaceId, hash: parent })}>{parent.slice(0, 7)}</Button>
                ))}
              </div>
            )}
            <p className="px-1 text-sm">
              {t("git.commit.files", { count: commit.files.length })}
              <span className="ml-2 font-mono text-xs"><span className="text-[var(--term-ok)]">+{commit.totalAdditions}</span> <span className="text-destructive">-{commit.totalDeletions}</span></span>
            </p>
            {commit.truncated && <Message tone="warning">{t("git.listTruncated")}</Message>}
            {commit.files.length > 0 && (
              <ul className="divide-y overflow-hidden rounded-xl border bg-card">
                {commit.files.map((file) => (
                  <li key={`${file.status}:${file.path}`}>
                    <GitFileRow
                      status={file.status}
                      path={file.path}
                      oldPath={file.oldPath}
                      trailing={file.binary
                        ? <span className="shrink-0 text-xs text-muted-foreground">{t("git.commit.binary")}</span>
                        : <span className="shrink-0 font-mono text-xs"><span className="text-[var(--term-ok)]">+{file.additions}</span> <span className="text-destructive">-{file.deletions}</span></span>}
                      onOpen={() => open({ name: "diff", session, workspaceId, source: { kind: "commit", hash: commit.hash, path: file.path, origPath: file.oldPath } })}
                    />
                  </li>
                ))}
              </ul>
            )}
          </>
        )}
      </div>
    </Page>
  )
}
