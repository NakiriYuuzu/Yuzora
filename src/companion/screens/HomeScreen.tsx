import { useMemo } from "react"
import { useTranslation } from "react-i18next"
import { ChevronRight, LogOut, RefreshCw } from "lucide-react"
import { Button } from "@/components/ui/button"
import { logout, refresh } from "../client"
import { ConnectionPill } from "../components/ConnectionPill"
import { Message, Page, Progress } from "../components/Page"
import { StatusBadge } from "../components/StatusBadge"
import { projectSpaces } from "../lib/agents"
import { rpcErrorKey } from "../lib/errors"
import type { Snapshot } from "../lib/protocol"
import { navigate, useCompanion } from "../store"

export function HomeScreen() {
  const { t } = useTranslation()
  const sessions = useCompanion((state) => state.sessions)
  const snapshots = useCompanion((state) => state.snapshots)
  const errors = useCompanion((state) => state.sessionErrors)
  const socket = useCompanion((state) => state.socket)

  return (
    <Page
      title={t("app.title")}
      trailing={
        <>
          <ConnectionPill />
          <Button variant="ghost" size="icon" className="size-11" aria-label={t("home.refresh")} disabled={socket !== "ready"} onClick={() => void refresh()}>
            <RefreshCw aria-hidden />
          </Button>
          <Button variant="ghost" size="icon" className="size-11" aria-label={t("home.logout")} onClick={() => void logout()}>
            <LogOut aria-hidden />
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-6 px-3 py-4">
        {sessions === null && <Progress text={t("home.loading")} />}
        {sessions?.length === 0 && <Message>{t("home.noSessions")}</Message>}
        {sessions?.map((session) => (
          <section key={session.name} aria-label={session.name} className="flex flex-col gap-2">
            <h2 className="px-1 text-sm font-medium text-muted-foreground">
              {session.name}
              {session.default && <span className="ml-2 text-xs">· {t("home.default")}</span>}
            </h2>
            {errors[session.name] && <Message tone="warning">{t(rpcErrorKey(errors[session.name]))}</Message>}
            {snapshots[session.name] ? <SessionAgents snapshot={snapshots[session.name]} /> : !errors[session.name] && <Progress text={t("home.loading")} />}
          </section>
        ))}
      </div>
    </Page>
  )
}

function SessionAgents({ snapshot }: { snapshot: Snapshot }) {
  const { t } = useTranslation()
  const spaces = useMemo(() => projectSpaces(snapshot), [snapshot])
  if (spaces.length === 0) return <Message>{t("home.noAgents")}</Message>
  return spaces.map((space) => (
    <div key={space.workspaceId ?? ""} className="overflow-hidden rounded-xl border bg-card">
      <h3 className="border-b px-3 py-2 text-sm font-semibold">{space.label ?? t("home.unnamedSpace")}</h3>
      <ul>
        {space.agents.map((agent) => (
          <li key={agent.paneId} className="border-b last:border-b-0">
            <button
              type="button"
              className="flex min-h-14 w-full items-center gap-3 px-3 py-2 text-left active:bg-muted"
              onClick={() => navigate({ name: "pane", session: agent.session, pane: agent.paneId })}
            >
              <span className="min-w-0 flex-1">
                <span className="block truncate text-base font-medium">{agent.name}</span>
                <span className="block truncate text-xs text-muted-foreground">{[agent.kind, agent.tabLabel, agent.hint ?? agent.paneLabel].filter(Boolean).join(" · ")}</span>
              </span>
              <StatusBadge status={agent.status} />
              <ChevronRight className="size-4 shrink-0 text-muted-foreground" aria-hidden />
            </button>
          </li>
        ))}
      </ul>
    </div>
  ))
}
