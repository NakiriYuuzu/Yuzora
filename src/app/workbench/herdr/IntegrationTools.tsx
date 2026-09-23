import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { Button } from "@/components/ui/button"
import { FieldGroup } from "@/components/ui/field"
import { Card, CardHeader, CardTitle, CardDescription, CardFooter } from "@/components/ui/card"
import { herdrFeature, type HerdrIntegration } from "@/lib/herdrFeatures"
import { requestAppConfirmation } from "@/state/appDialogStore"
import type { HerdrOperation } from "./useHerdrOperation"

export function IntegrationTools({ sessionName, operation, can }: { sessionName: string; operation: HerdrOperation; can: (method: string) => boolean }) {
  const { t } = useTranslation("herdrTools")
  const [items, setItems] = useState<HerdrIntegration[]>([])
  const [error, setError] = useState<string | null>(null)
  const [revision, setRevision] = useState(0)
  const available = can("integration.list")
  useEffect(() => {
    let active = true
    if (available) void herdrFeature(sessionName, { method: "integration.list", params: {} }).then(result => {
      if (active) { setItems(Array.isArray(result.integrations) ? result.integrations as HerdrIntegration[] : []); setError(null) }
    }).catch(cause => { if (active) setError(String(cause)) })
    return () => { active = false }
  }, [sessionName, available, revision])
  return <FieldGroup>
    <p>{t("integrationHint")}</p>
    {error && <p role="alert" className="text-destructive">{error}</p>}
    {!available && <p>{t("unavailable")}</p>}
    {items.map(item => <Card key={item.target} size="sm"><CardHeader><CardTitle>{item.label}</CardTitle><CardDescription>{t(`integrationState.${item.state}`)} · {item.command}</CardDescription></CardHeader><CardFooter className="flex flex-wrap gap-2">
      <Button disabled={operation.busy || !can("integration.install")} onClick={async () => {
        if (await requestAppConfirmation({ title: t("installIntegration"), description: t("integrationWarning", { name: item.label }) }) && await operation.run({ method: "integration.install", params: { target: item.target } })) setRevision(value => value + 1)
      }}>{t(item.state === "not_installed" ? "install" : "update")}</Button>
      <Button variant="outline" disabled={operation.busy || item.state === "not_installed" || !can("integration.uninstall")} onClick={async () => {
        if (await requestAppConfirmation({ title: t("uninstallIntegration"), description: t("integrationRemoveWarning", { name: item.label }), destructive: true }) && await operation.run({ method: "integration.uninstall", params: { target: item.target } })) setRevision(value => value + 1)
      }}>{t("uninstall")}</Button>
    </CardFooter></Card>)}
  </FieldGroup>
}
