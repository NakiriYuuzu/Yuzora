import { useTranslation } from "react-i18next"
import { writeText } from "@tauri-apps/plugin-clipboard-manager"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Button } from "@/components/ui/button"
import { describeHerdrError } from "@/lib/herdrErrors"

/** Localized HERDR error with the raw backend text folded into a scrollable diagnostics block. */
export function DescribedError({ error, title, action }: { error: unknown; title?: string; action?: React.ReactNode }) {
  const { t } = useTranslation("herdrErrors")
  const described = describeHerdrError(error, t)
  const showDiagnostics = described.code !== null && described.detail !== null || described.code === null && described.raw.length > 160
  return <Alert variant="destructive">
    {title && <AlertTitle>{title}</AlertTitle>}
    <AlertDescription className="min-w-0 break-words">
      <p>{described.code ? described.message : described.raw.length > 160 ? described.raw.slice(0, 160) + "…" : described.raw}</p>
      {showDiagnostics && <details className="mt-2 text-xs">
        <summary className="cursor-pointer">{t("ui.diagnostics")}</summary>
        <pre className="mt-1 max-h-40 overflow-auto whitespace-pre-wrap break-all">{described.raw}</pre>
        <Button type="button" variant="outline" size="sm" className="mt-2" onClick={() => { void writeText(described.raw).catch(() => undefined) }}>{t("ui.copyDiagnostics")}</Button>
      </details>}
      {action}
    </AlertDescription>
  </Alert>
}
