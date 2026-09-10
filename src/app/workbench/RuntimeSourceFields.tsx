import { useId } from "react"
import { useTranslation } from "react-i18next"
import { Field, FieldDescription, FieldGroup, FieldLabel } from "@/components/ui/field"
import { Input } from "@/components/ui/input"
import { Select, SelectContent, SelectGroup, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Badge } from "@/components/ui/badge"
import { ScrollArea } from "@/components/ui/scroll-area"
import type { HerdrRuntimeSelection, RuntimeBinaryCheck } from "@/lib/herdrTypes"

export function RuntimeSourceFields({ value, onChange, disabled, local = false }: {
  value: HerdrRuntimeSelection
  onChange: (selection: HerdrRuntimeSelection) => void
  disabled?: boolean
  local?: boolean
}) {
  const { t } = useTranslation("runtimeSettings")
  const id = useId()
  return <FieldGroup>
    <Field data-disabled={disabled}>
      <FieldLabel htmlFor={`${id}-source`}>{t("source")}</FieldLabel>
      <Select value={value.source} disabled={disabled} onValueChange={source => {
        if (source === "default" || source === "global" || source === "custom") onChange({ source, ...(source === "custom" ? { customPath: value.customPath ?? "" } : {}) })
      }}>
        <SelectTrigger id={`${id}-source`} className="w-full"><SelectValue /></SelectTrigger>
        <SelectContent><SelectGroup>
          <SelectItem value="default">{t("managed")}</SelectItem>
          <SelectItem value="global">{t("installed")}</SelectItem>
          <SelectItem value="custom">{t("custom")}</SelectItem>
        </SelectGroup></SelectContent>
      </Select>
      <FieldDescription>{t(value.source === "default" ? "managedHint" : value.source === "global" ? "installedHint" : local ? "customLocalHint" : "customRemoteHint")}</FieldDescription>
    </Field>
    {value.source === "custom" && <Field data-disabled={disabled}>
      <FieldLabel htmlFor={`${id}-path`}>{t("customPath")}</FieldLabel>
      <Input id={`${id}-path`} value={value.customPath ?? ""} disabled={disabled} onChange={event => onChange({ source: "custom", customPath: event.target.value })} />
    </Field>}
  </FieldGroup>
}

export function RuntimeCheckView({ check }: { check: RuntimeBinaryCheck }) {
  const { t } = useTranslation("runtimeSettings")
  return <div className="flex min-w-0 flex-col gap-3">
    <Badge variant={check.canApply ? "secondary" : "destructive"}>{t(check.canApply ? "compatible" : "incompatible")}</Badge>
    <dl className="grid min-w-0 grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-2 text-sm">
      <dt>{t("client")}</dt><dd>{check.clientVersion} · protocol {check.clientProtocol}</dd>
      <dt>{t("binary")}</dt><dd className="break-all"><code>{check.binary}</code></dd>
      {check.reportedBinary && check.reportedBinary !== check.binary && <><dt>{t("reportedBinary")}</dt><dd className="break-all"><code>{check.reportedBinary}</code></dd></>}
      <dt>{t("schema")}</dt><dd>{check.schemaProtocol}</dd>
    </dl>
    {!!check.missingMethods.length && <Alert variant="destructive"><AlertTitle>{t("missingMethods")}</AlertTitle><AlertDescription>{check.missingMethods.join(", ")}</AlertDescription></Alert>}
    <ScrollArea className="max-h-56 min-w-0" viewportClassName="[&>div]:!block"><div className="flex flex-col gap-3">
      {check.sessions.map(session => <div key={session.name} className="min-w-0 text-sm">
        <p className="font-medium">{session.name} · {t(!session.running ? "notRunning" : session.compatible ? "compatible" : "incompatible")}</p>
        {session.running && <p>{t("server")}: {session.serverVersion ?? "—"} · protocol {session.serverProtocol ?? "—"}</p>}
        {session.socket && <p className="break-all"><code>{session.socket}</code></p>}
      </div>)}
    </div></ScrollArea>
    {!check.canApply && <Alert variant="destructive"><AlertTitle>{t("blocked")}</AlertTitle><AlertDescription>{t("preserveSessions")}</AlertDescription></Alert>}
  </div>
}
