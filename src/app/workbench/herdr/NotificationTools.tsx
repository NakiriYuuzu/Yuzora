import { useId, useState } from "react"
import { useTranslation } from "react-i18next"
import { isPermissionGranted, requestPermission } from "@tauri-apps/plugin-notification"
import { Field, FieldDescription, FieldGroup, FieldLabel } from "@/components/ui/field"
import { Switch } from "@/components/ui/switch"
import { useHerdrNotificationStore, type HerdrNotificationSettings } from "@/state/herdrNotificationStore"
import { playHerdrSound } from "@/lib/herdrNotifications"

export function NotificationTools() {
  const { t } = useTranslation("herdrTools")
  const settings = useHerdrNotificationStore()
  const [error, setError] = useState<string | null>(null)
  const prefix = useId()
  const options: (keyof HerdrNotificationSettings)[] = ["toast", "system", "sound", "done", "blocked"]
  return <FieldGroup>
    {options.map(key => <Field key={key} orientation="horizontal"><div className="flex flex-1 flex-col gap-1"><FieldLabel htmlFor={`${prefix}-${key}`}>{t(`notification.${key}`)}</FieldLabel><FieldDescription>{t(`notification.${key}Hint`)}</FieldDescription></div><Switch id={`${prefix}-${key}`} checked={settings[key]} onCheckedChange={async enabled => {
      setError(null)
      try {
        if (key === "system" && enabled && !await isPermissionGranted() && await requestPermission() !== "granted") throw new Error(t("notification.permissionDenied"))
        if (key === "sound" && enabled) await playHerdrSound("done")
        settings.update({ [key]: enabled })
      } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    }} /></Field>)}
    {error && <p role="alert" className="text-destructive">{error}</p>}
  </FieldGroup>
}
