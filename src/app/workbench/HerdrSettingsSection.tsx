import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"

import { Button } from "@/components/ui/button"
import { herdrBinarySourceGet, herdrBinarySourceSet } from "@/lib/herdrIpc"
import type { HerdrBinarySource, HerdrBinarySourceInfo } from "@/lib/herdrTypes"

export function HerdrSettingsSection() {
  const { t } = useTranslation("workbench")
  const [info, setInfo] = useState<HerdrBinarySourceInfo | null>(null)
  const [pending, setPending] = useState<HerdrBinarySource | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [restartRequired, setRestartRequired] = useState(false)

  useEffect(() => {
    let active = true
    void herdrBinarySourceGet()
      .then((next) => {
        if (!active) return
        setInfo(next)
        setRestartRequired(next.restartRequired)
      })
      .catch((err) => {
        if (!active) return
        setError(err instanceof Error ? err.message : String(err))
      })
    return () => {
      active = false
    }
  }, [])

  const onSelect = async (source: HerdrBinarySource) => {
    setPending(source)
    setError(null)
    try {
      const result = await herdrBinarySourceSet(source)
      const next = await herdrBinarySourceGet()
      setInfo(next)
      setRestartRequired(result.restartRequired || next.restartRequired)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setPending(null)
    }
  }

  const configured = info?.configured ?? "global"
  const active = info?.active ?? info?.resolved ?? info?.configured ?? null
  const configuredAvailable = info
    ? (info.configuredAvailable ?? (info.configured === active ? info.available : false))
    : false

  return (
    <div className="flex flex-col gap-[14px]">
      <div className="rounded-[12px] border border-(--line-1) bg-(--paper-0) px-[14px] py-[12px]">
        <div className="text-[13px] font-semibold text-(--ink-1)">
          {t("herdrSettings.binarySource")}
        </div>
        <p className="mt-[4px] text-[11.5px] text-(--ink-3)">
          {t("herdrSettings.binarySourceSub")}
        </p>
        <div
          role="group"
          aria-label={t("herdrSettings.binarySource")}
          className="mt-[12px] flex flex-wrap gap-[8px]"
        >
          <Button
            type="button"
            size="sm"
            variant={configured === "global" ? "default" : "outline"}
            aria-pressed={configured === "global"}
            disabled={pending !== null}
            onClick={() => void onSelect("global")}
          >
            {t("herdrSettings.global")}
          </Button>
          <Button
            type="button"
            size="sm"
            variant={configured === "default" ? "default" : "outline"}
            aria-pressed={configured === "default"}
            disabled={pending !== null}
            onClick={() => void onSelect("default")}
          >
            {t("herdrSettings.default")}
          </Button>
        </div>
        {restartRequired && (
          <p role="status" className="mt-[10px] text-[11.5px] text-(--ink-2)">
            {t("herdrSettings.restartRequired")}
          </p>
        )}
      </div>

      <div className="rounded-[12px] border border-(--line-1) bg-(--paper-0) px-[14px] py-[12px] text-[12px]">
        <div className="font-semibold text-(--ink-1)">{t("herdrSettings.diagnostics")}</div>
        <dl className="mt-[8px] grid grid-cols-[120px_1fr] gap-x-[10px] gap-y-[6px] text-(--ink-2)">
          <dt>{t("herdrSettings.active")}</dt>
          <dd>{active ?? "—"}</dd>
          <dt>{t("herdrSettings.path")}</dt>
          <dd className="break-all font-mono text-[11px]">{info?.path ?? "—"}</dd>
          <dt>{t("herdrSettings.version")}</dt>
          <dd>{info?.version ?? "—"}</dd>
          <dt>{t("herdrSettings.protocol")}</dt>
          <dd>{info?.protocol ?? "—"}</dd>
          <dt>{t("herdrSettings.availability")}</dt>
          <dd>
            {info
              ? info.available
                ? t("herdrSettings.available")
                : t("herdrSettings.unavailable")
              : "—"}
          </dd>
          <dt>{t("herdrSettings.reason")}</dt>
          <dd>{info?.reason ?? "—"}</dd>
          <dt>{t("herdrSettings.configuredTarget")}</dt>
          <dd>{info?.configured ?? "—"}</dd>
          <dt>{t("herdrSettings.targetPath")}</dt>
          <dd className="break-all font-mono text-[11px]">
            {info?.configuredPath ?? "—"}
          </dd>
          <dt>{t("herdrSettings.version")}</dt>
          <dd>{info?.configuredVersion ?? "—"}</dd>
          <dt>{t("herdrSettings.protocol")}</dt>
          <dd>{info?.configuredProtocol ?? "—"}</dd>
          <dt>{t("herdrSettings.targetAvailability")}</dt>
          <dd>
            {info
              ? configuredAvailable
                ? t("herdrSettings.available")
                : t("herdrSettings.unavailable")
              : "—"}
          </dd>
          <dt>{t("herdrSettings.reason")}</dt>
          <dd>{info?.configuredReason ?? "—"}</dd>
        </dl>
        {!configuredAvailable && info?.configured === "default" && (
          <p className="mt-[10px] text-[11.5px] text-(--ink-3)">
            {t("herdrSettings.defaultUnavailable")}
          </p>
        )}
        {info?.configurationError && (
          <p role="alert" className="mt-[10px] text-[11.5px] text-destructive">
            {t("herdrSettings.configurationError", {
              error: info.configurationError
            })}
          </p>
        )}
        {error && (
          <p role="alert" className="mt-[10px] text-[11.5px] text-destructive">
            {error}
          </p>
        )}
      </div>
    </div>
  )
}
