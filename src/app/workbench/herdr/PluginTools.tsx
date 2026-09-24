import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { openUrl } from "@tauri-apps/plugin-opener"
import { Button } from "@/components/ui/button"
import { FieldGroup } from "@/components/ui/field"
import { Card, CardHeader, CardTitle, CardDescription, CardContent, CardFooter } from "@/components/ui/card"
import { herdrFeature, type HerdrPlugin } from "@/lib/herdrFeatures"
import { requestAppConfirmation } from "@/state/appDialogStore"
import { useHerdrNativeStore } from "@/state/herdrNativeStore"
import { TextField } from "./controls"
import type { HerdrOperation } from "./useHerdrOperation"

export function PluginTools({ sessionName, workspaceId, paneId, operation, can }: { sessionName: string; workspaceId: string; paneId: string; operation: HerdrOperation; can: (method: string) => boolean }) {
  const { t } = useTranslation("herdrTools")
  const [source, setSource] = useState("")
  const [revision, setRevision] = useState("")
  const [items, setItems] = useState<HerdrPlugin[]>([])
  const [error, setError] = useState<string | null>(null)
  const [refresh, setRefresh] = useState(0)
  const available = can("plugin.list")
  useEffect(() => {
    let active = true
    if (available) void herdrFeature(sessionName, { method: "plugin.list", params: {} }).then(result => {
      if (active) { setItems(Array.isArray(result.plugins) ? result.plugins as HerdrPlugin[] : []); setError(null) }
    }).catch(cause => { if (active) setError(String(cause)) })
    return () => { active = false }
  }, [sessionName, available, refresh])
  return <FieldGroup>
    <Card><CardHeader><CardTitle>{t("installPlugin")}</CardTitle><CardDescription>{t("pluginHint")}</CardDescription></CardHeader><CardContent><FieldGroup>
      <TextField label={t("pluginSource")} value={source} onChange={setSource} placeholder="owner/repository/subdirectory" />
      <TextField label={t("pluginRevision")} value={revision} onChange={setRevision} />
    </FieldGroup></CardContent><CardFooter className="flex flex-wrap gap-2">
      <Button disabled={operation.busy || !available || !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+(?:\/[^\s]+)*$/.test(source)} onClick={async () => {
        if (await requestAppConfirmation({ title: t("installPlugin"), description: t("pluginWarning", { source }) }) && await operation.run({ method: "plugin.install", params: { source, revision: revision || undefined } })) setRefresh(value => value + 1)
      }}>{t("install")}</Button>
      <Button variant="outline" onClick={() => void openUrl("https://herdr.dev/plugins/").catch(cause => setError(String(cause)))}>{t("marketplace")}</Button>
    </CardFooter></Card>
    {error && <p role="alert" className="text-destructive">{error}</p>}
    {!available && <p>{t("unavailable")}</p>}
    {items.map(plugin => <Card key={plugin.plugin_id} size="sm"><CardHeader><CardTitle>{plugin.name} · {plugin.version}</CardTitle><CardDescription>{plugin.description}</CardDescription></CardHeader><CardContent className="flex flex-wrap gap-2">
      {plugin.actions?.map(action => <Button key={action.id} variant="outline" disabled={operation.busy || !plugin.enabled || !can("plugin.action.invoke")} onClick={() => useHerdrNativeStore.getState().open({ sessionName, paneId: paneId || undefined, request: { method: "plugin.action.invoke", params: {
        plugin_id: plugin.plugin_id, action_id: action.id,
        // Context is optional; without a Space HERDR fills it from the active workspace, tab and pane.
        ...(workspaceId ? { context: { workspace_id: workspaceId, focused_pane_id: paneId || undefined } } : {})
      } } })}>{action.title}</Button>)}
      {plugin.panes?.map(pane => {
        const placement = pane.placement ?? "overlay"
        // Popup/overlay use the active client pane and need no workspace; other placements have distinct targets.
        const target = placement === "tab" ? { workspace_id: workspaceId }
          : placement === "split" || placement === "zoomed" ? { target_pane_id: paneId } : {}
        return <Button key={pane.id} variant="outline" disabled={operation.busy || !plugin.enabled || Object.values(target).includes("") || !can("plugin.pane.open")} onClick={() => {
          useHerdrNativeStore.getState().open({ sessionName, paneId: paneId || undefined, request: { method: "plugin.pane.open", params: { plugin_id: plugin.plugin_id, entrypoint: pane.id, placement, ...target, focus: true } } })
        }}>{pane.title}</Button>
      })}
    </CardContent><CardFooter className="flex flex-wrap gap-2">
      <Button variant="outline" disabled={operation.busy || !can(plugin.enabled ? "plugin.disable" : "plugin.enable")} onClick={async () => { if (await operation.run({ method: plugin.enabled ? "plugin.disable" : "plugin.enable", params: { plugin_id: plugin.plugin_id } })) setRefresh(value => value + 1) }}>{t(plugin.enabled ? "disable" : "enable")}</Button>
      <Button variant="outline" disabled={operation.busy || !can("plugin.log.list")} onClick={() => void operation.run({ method: "plugin.log.list", params: { plugin_id: plugin.plugin_id } })}>{t("logs")}</Button>
      <Button variant="destructive" disabled={operation.busy || !available} onClick={async () => {
        if (await requestAppConfirmation({ title: t("uninstallPlugin"), description: t("pluginRemoveWarning", { name: plugin.name }), destructive: true }) && await operation.run({ method: "plugin.uninstall", params: { plugin_id: plugin.plugin_id } })) setRefresh(value => value + 1)
      }}>{t("uninstall")}</Button>
    </CardFooter></Card>)}
  </FieldGroup>
}
