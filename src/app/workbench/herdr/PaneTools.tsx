import { useState } from "react"
import { useTranslation } from "react-i18next"
import { ArrowRight, Columns2, FolderPlus, PanelTop, Rows2, SquarePlus } from "lucide-react"
import { Button } from "@/components/ui/button"
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group"
import type { HerdrSnapshot } from "@/lib/herdrTypes"
import type { PaneMoveDestination } from "@/lib/herdrFeatures"
import { ChoiceCards, PaneChoices, TextField, ToolStep } from "./controls"
import type { HerdrOperation } from "./useHerdrOperation"

type Destination = "tab" | "new_tab" | "new_workspace"

export function PaneTools({ snapshot, paneId, workspaceId, operation, can }: {
  snapshot: HerdrSnapshot | null; paneId: string; workspaceId: string; operation: HerdrOperation; can: (method: string) => boolean
}) {
  const { t } = useTranslation("herdrTools")
  const [source, setSource] = useState(paneId)
  const [destination, setDestination] = useState<Destination>("new_tab")
  const [targetTab, setTargetTab] = useState("")
  const [targetWorkspace, setTargetWorkspace] = useState(workspaceId)
  const [direction, setDirection] = useState<"right" | "down">("right")
  const [label, setLabel] = useState("")
  const sourcePane = snapshot?.terminals.find(pane => pane.paneId === source)
  const tabs = snapshot?.tabs ?? []
  const validTarget = destination === "new_workspace" || (destination === "tab"
    ? tabs.some(tab => tab.id === targetTab)
    : snapshot?.spaces.some(space => space.id === targetWorkspace))
  const spaceLabel = (id?: string | null) => snapshot?.spaces.find(space => space.id === id)?.label ?? id ?? ""
  const targetLabel = destination === "tab" ? snapshot?.tabs.find(tab => tab.id === targetTab)?.label ?? "—"
    : destination === "new_tab" ? t("summaryNewTab", { space: spaceLabel(targetWorkspace) || "—" })
      : t("summaryNewSpace", { name: label || t("destination.new_workspace") })
  return <div className="flex min-w-0 flex-col gap-5">
    <ToolStep index={1} title={t("moveSource")}>
      <PaneChoices snapshot={snapshot} value={source} onChange={setSource} disabled={operation.busy} />
    </ToolStep>
    <ToolStep index={2} title={t("moveDestination")}>
      <ChoiceCards label={t("moveDestination")} value={destination} onChange={value => setDestination(value as Destination)} disabled={operation.busy} columns={3} options={[
        { value: "tab", icon: PanelTop, title: t("destination.tab"), description: t("destinationHint.tab") },
        { value: "new_tab", icon: SquarePlus, title: t("destination.new_tab"), description: t("destinationHint.new_tab") },
        { value: "new_workspace", icon: FolderPlus, title: t("destination.new_workspace"), description: t("destinationHint.new_workspace") },
      ]} />
    </ToolStep>
    <ToolStep index={3} title={t(`placement.${destination}`)}>
      <div className="flex min-w-0 flex-col gap-3">
        {destination === "tab" && <>
          {tabs.length ? <ChoiceCards label={t("tab")} value={targetTab} onChange={setTargetTab} disabled={operation.busy} columns={2} options={tabs.map(tab => ({
            value: tab.id, title: tab.label || tab.id, description: t("tabIn", { space: spaceLabel(tab.workspaceId), panes: tab.paneCount }),
          }))} /> : <p className="text-xs text-muted-foreground">{t("noTabs")}</p>}
          <div className="flex flex-wrap items-center gap-3">
            <span className="text-xs text-muted-foreground">{t("splitDirection")}</span>
            <ToggleGroup type="single" value={direction} onValueChange={value => { if (value) setDirection(value as "right" | "down") }} aria-label={t("splitDirection")} className="gap-1 rounded-lg bg-muted p-[3px]" disabled={operation.busy}>
              <ToggleGroupItem value="right" className="h-7 gap-1.5 px-2.5 text-xs data-[state=on]:shadow-sm"><Columns2 className="size-3.5" />{t("right")}</ToggleGroupItem>
              <ToggleGroupItem value="down" className="h-7 gap-1.5 px-2.5 text-xs data-[state=on]:shadow-sm"><Rows2 className="size-3.5" />{t("down")}</ToggleGroupItem>
            </ToggleGroup>
          </div>
        </>}
        {destination === "new_tab" && <ToggleGroup type="single" value={targetWorkspace} onValueChange={value => { if (value) setTargetWorkspace(value) }} aria-label={t("workspace")} className="flex-wrap justify-start gap-1.5" disabled={operation.busy}>
          {snapshot?.spaces.map(space => <ToggleGroupItem key={space.id} value={space.id} className="h-7 border px-2.5 text-xs data-[state=on]:border-primary/40 data-[state=on]:bg-primary/5">{space.label}</ToggleGroupItem>)}
        </ToggleGroup>}
        {destination !== "tab" && <TextField label={t("nameOptional")} value={label} onChange={setLabel} disabled={operation.busy} />}
      </div>
    </ToolStep>
    <div className="flex min-w-0 flex-wrap items-center gap-3 rounded-lg border bg-muted/40 px-3 py-2.5">
      <p className="flex min-w-0 flex-1 items-center gap-2 text-xs text-muted-foreground" aria-live="polite">
        <span className="min-w-0 truncate font-medium text-foreground">{sourcePane?.title ?? sourcePane?.paneId ?? "—"}</span>
        <ArrowRight className="size-3.5 shrink-0" aria-hidden="true" />
        <span className="min-w-0 truncate font-medium text-foreground">{targetLabel}</span>
      </p>
      <Button disabled={operation.busy || !can("pane.move") || !sourcePane || !validTarget} onClick={async () => {
        const target: PaneMoveDestination = destination === "tab" ? { type: "tab", tab_id: targetTab, split: direction }
          : destination === "new_tab" ? { type: "new_tab", workspace_id: targetWorkspace, label: label || undefined }
            : { type: "new_workspace", label: label || undefined }
        const result = await operation.run({ method: "pane.move", params: { pane_id: source, destination: target, focus: false } })
        const moved = result?.move_result as { pane?: { pane_id?: string } } | undefined
        if (moved?.pane?.pane_id) setSource(moved.pane.pane_id)
      }}>{t("move")}</Button>
    </div>
  </div>
}
