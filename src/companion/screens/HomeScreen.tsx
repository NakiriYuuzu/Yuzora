import { useMemo } from "react"
import { useTranslation } from "react-i18next"
import { ChevronRight, LogOut, RefreshCw } from "lucide-react"
import { Button } from "@yuzora/shared/components/ui/button"
import { logout, refresh } from "../client"
import { AgentList } from "../components/AgentList"
import { ConnectionPill } from "../components/ConnectionPill"
import { Disclosure } from "../components/Disclosure"
import { Message, Page, Progress } from "../components/Page"
import { PushControl } from "../components/PushControl"
import { otherSpaces, projectSpaces } from "../lib/agents"
import { rpcErrorKey } from "../lib/errors"
import type { Snapshot } from "../lib/protocol"
import { open, useCompanion } from "../store"

export function HomeScreen() {
  const { t } = useTranslation()
  const sessions = useCompanion((state) => state.sessions)
  const snapshots = useCompanion((state) => state.snapshots)
  const errors = useCompanion((state) => state.sessionErrors)
  const socket = useCompanion((state) => state.socket)
  const push = useCompanion((state) => state.hello?.push ?? null)

  return (
    <Page
      title={t("app.title")}
      trailing={
        <>
          <ConnectionPill />
          {push && <PushControl vapidPublicKey={push.vapidPublicKey} />}
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
  const others = useMemo(() => otherSpaces(snapshot), [snapshot])
  return (
    <>
      {spaces.length === 0 && <Message>{t("home.noAgents")}</Message>}
      {spaces.map((space) => (
        <div key={space.workspaceId ?? ""} className="overflow-hidden rounded-xl border bg-card">
          <h3 className="border-b text-sm font-semibold">
            {space.workspaceId === null ? (
              <span className="block px-3 py-2">{space.label ?? t("home.unnamedSpace")}</span>
            ) : (
              <SpaceLink session={space.session} workspaceId={space.workspaceId} label={space.label} />
            )}
          </h3>
          <AgentList agents={space.agents} />
        </div>
      ))}
      {others.length > 0 && (
        <Disclosure title={t("home.otherSpaces")} count={others.length}>
          <ul className="divide-y">
            {others.map((space) => (
              <li key={space.workspaceId}>
                <SpaceLink session={space.session} workspaceId={space.workspaceId} label={space.label} />
              </li>
            ))}
          </ul>
        </Disclosure>
      )}
    </>
  )
}

/** A Space's name as a button into its own screen. */
function SpaceLink({ session, workspaceId, label }: { session: string; workspaceId: string; label: string | null }) {
  const { t } = useTranslation()
  return (
    <button
      type="button"
      className="flex min-h-11 w-full items-center gap-2 px-3 py-2 text-left active:bg-muted"
      onClick={() => open({ name: "space", session, workspaceId })}
    >
      <span className="min-w-0 flex-1 truncate">{label ?? t("home.unnamedSpace")}</span>
      <ChevronRight className="size-4 shrink-0 text-muted-foreground" aria-hidden />
    </button>
  )
}
