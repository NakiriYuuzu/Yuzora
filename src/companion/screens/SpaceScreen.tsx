import { useMemo, useState } from "react"
import { useTranslation } from "react-i18next"
import { RefreshCw } from "lucide-react"
import { Button } from "@yuzora/shared/components/ui/button"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@yuzora/shared/components/ui/tabs"
import { AgentList } from "../components/AgentList"
import { BackButton } from "../components/BackButton"
import { ConnectionPill } from "../components/ConnectionPill"
import { Message, Page, Progress } from "../components/Page"
import { dropLog } from "../gitReads"
import { projectSpaces } from "../lib/agents"
import { spaceKey } from "../lib/git"
import { setSpaceView, useCompanion, type SpaceTab } from "../store"
import { GitTab } from "./git/GitTab"

/** One Space: its agents and, when the desktop shares the repository, its Git. */
export function SpaceScreen({ session, workspaceId }: { session: string; workspaceId: string }) {
  const { t } = useTranslation()
  const key = spaceKey(session, workspaceId)
  const snapshot = useCompanion((state) => state.snapshots[session])
  const saved = useCompanion((state) => state.spaceView[key]?.tab)
  const ready = useCompanion((state) => state.socket === "ready")
  const [reloadToken, setReloadToken] = useState(0)
  const space = snapshot?.workspaces.find((item) => item.id === workspaceId)
  const agents = useMemo(() => (snapshot ? projectSpaces(snapshot).find((group) => group.workspaceId === workspaceId)?.agents ?? [] : []), [snapshot, workspaceId])
  // A Space without agents opens on its Git.
  const preferred = saved ?? (agents.length === 0 ? "git" : "agents")
  const tab: SpaceTab = preferred === "git" && space?.git ? "git" : "agents"

  return (
    <Page
      title={space?.label ?? t("home.unnamedSpace")}
      leading={<BackButton />}
      trailing={
        <>
          <ConnectionPill />
          {tab === "git" && (
            <Button
              variant="ghost"
              size="icon"
              className="size-11"
              aria-label={t("git.refresh")}
              disabled={!ready}
              onClick={() => {
                dropLog(session, workspaceId)
                setReloadToken((value) => value + 1)
              }}
            >
              <RefreshCw aria-hidden />
            </Button>
          )}
        </>
      }
    >
      <div className="flex flex-col gap-3 px-3 py-3">
        {!snapshot && <Progress text={t("home.loading")} />}
        {snapshot && !space && <Message tone="warning">{t("space.gone")}</Message>}
        {space && (
          <Tabs value={tab} onValueChange={(next) => setSpaceView(key, { tab: next as SpaceTab })} className="gap-3">
            <TabsList className="w-full group-data-horizontal/tabs:h-[50px]">
              <TabsTrigger value="agents" className="h-11 text-sm">{t("space.tabs.agents")}</TabsTrigger>
              {space.git && <TabsTrigger value="git" className="h-11 text-sm">{t("space.tabs.git")}</TabsTrigger>}
            </TabsList>
            <TabsContent value="agents">
              {agents.length === 0 ? <Message>{t("space.noAgents")}</Message> : <div className="overflow-hidden rounded-xl border bg-card"><AgentList agents={agents} /></div>}
            </TabsContent>
            <TabsContent value="git">
              <GitTab key={space.gitGeneration} session={session} workspaceId={workspaceId} reloadToken={reloadToken} />
            </TabsContent>
          </Tabs>
        )}
      </div>
    </Page>
  )
}
