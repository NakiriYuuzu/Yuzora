import { useTranslation } from "react-i18next"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { spaceKey } from "../../lib/git"
import { setSpaceView, useCompanion, type GitView } from "../../store"
import { BranchesView } from "./BranchesView"
import { ChangesView } from "./ChangesView"
import { HistoryView } from "./HistoryView"

const VIEWS: GitView[] = ["changes", "history", "branches"]

/** The Git tab of a Space: changes, history, branches. Reading only, there is nothing to change from here. */
export function GitTab({ session, workspaceId, reloadToken }: { session: string; workspaceId: string; reloadToken: number }) {
  const { t } = useTranslation()
  const key = spaceKey(session, workspaceId)
  const view = useCompanion((state) => state.spaceView[key]?.view ?? "changes")
  const props = { session, workspaceId, reloadToken }
  return (
    <Tabs value={view} onValueChange={(next) => setSpaceView(key, { tab: "git", view: next as GitView })} className="gap-3">
      <TabsList variant="line" className="w-full group-data-horizontal/tabs:h-[50px]">
        {VIEWS.map((name) => (
          <TabsTrigger key={name} value={name} className="h-11 text-sm">{t(`git.views.${name}`)}</TabsTrigger>
        ))}
      </TabsList>
      <TabsContent value="changes"><ChangesView {...props} /></TabsContent>
      <TabsContent value="history"><HistoryView {...props} /></TabsContent>
      <TabsContent value="branches"><BranchesView {...props} /></TabsContent>
    </Tabs>
  )
}
