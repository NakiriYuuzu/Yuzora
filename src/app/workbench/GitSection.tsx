import { useEffect, useState } from "react"
import { RefreshCw } from "lucide-react"
import { useTranslation } from "react-i18next"

import { Button } from "@/components/ui/button"
import { cn } from "@/lib/utils"
import { useGitStore, type RemoteCheckMode } from "@/state/gitStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { SettingCard } from "./settingsPrimitives"

/**
 * Git pane — detection state card + remote-check card. Reads the live git
 * environment / remote-check config from useGitStore (T11) and re-detects
 * against the current workspace path. Visual language extends SettingCard;
 * no upstream design for this pane.
 */
export function GitSection() {
  const { t } = useTranslation("workbench")
  const environment = useGitStore((s) => s.environment)
  const remoteCheck = useGitStore((s) => s.remoteCheck)
  const setRemoteCheck = useGitStore((s) => s.setRemoteCheck)
  const detect = useGitStore((s) => s.detect)
  const workspacePath = useWorkspaceStore((s) => s.workspacePath)

  // Free-typing draft for the interval field: onChange no longer rejects
  // intermediate sub-minimum values (e.g. "4" on the way to "45"); clamping and
  // commit happen on blur (T19).
  const [intervalText, setIntervalText] = useState(String(remoteCheck.intervalSec))
  const remoteCheckModes: { id: RemoteCheckMode; label: string }[] = [
    { id: "off", label: t("gitSettings.modeOff") },
    { id: "probe", label: t("gitSettings.modeProbe") },
    { id: "autofetch", label: t("gitSettings.modeAutofetch") },
  ]

  useEffect(() => {
    if (environment?.status === "missing" && environment.reason) {
      console.warn("git executable unavailable in settings:", environment.reason)
    }
  }, [environment])

  const redetect = () => {
    if (workspacePath) void detect(workspacePath)
  }

  function commitInterval() {
    const next = Number(intervalText)
    const clamped = Number.isFinite(next) && next >= 30 ? Math.floor(next) : 30
    setIntervalText(String(clamped))
    if (clamped !== remoteCheck.intervalSec) {
      setRemoteCheck({ ...remoteCheck, intervalSec: clamped })
    }
  }

  const missing = !environment || environment.status === "missing"
  const unsupported =
    environment?.status === "missing" && environment.kind === "unsupportedVersion"
  const minVersion =
    environment?.status === "missing"
      ? (environment.minimumVersion ?? "2.24")
      : "2.24"

  return (
    <div className="flex flex-col gap-[14px]">
      <SettingCard
        label={t("gitSettings.detectionLabel")}
        sub={t("gitSettings.detectionDescription")}
      >
        {missing && (
          <div className="flex flex-col gap-[10px]">
            {unsupported ? (
              <>
                <span className="text-[12.5px] font-semibold leading-[1.45] text-(--ink-1)">
                  {t("gitSettings.unsupportedTitle")}
                </span>
                <span className="text-[12.5px] leading-[1.45] text-(--ink-2)">
                  {t("gitSettings.unsupportedDescription", { version: minVersion })}
                </span>
                <span className="text-[11.5px] leading-[1.45] text-(--ink-3)">
                  {t("gitSettings.unsupportedHint", { version: minVersion })}
                </span>
              </>
            ) : (
              <span className="text-[12.5px] leading-[1.45] text-(--ink-2)">
                {environment?.status === "missing"
                  ? t("gitSettings.unavailable")
                  : t("gitSettings.notDetected")}
              </span>
            )}
            <div>
              <Button
                type="button"
                size="sm"
                onClick={redetect}
                className="flex h-[28px] items-center gap-[6px] rounded-[8px] bg-(--yz-solid) px-[11px] text-[11.5px] font-semibold text-(--ink-0) shadow-(--shadow-xs) transition-colors hover:bg-(--yz-hover)"
              >
                <RefreshCw className="size-[12px]" aria-hidden="true" />
                {t("gitSettings.redetect")}
              </Button>
            </div>
          </div>
        )}

        {environment?.status === "notARepo" && (
          <span className="text-[12.5px] leading-[1.45] text-(--ink-2)">
            {t("gitSettings.notRepository")}
          </span>
        )}

        {environment?.status === "ready" && (
          <div className="flex items-center gap-[8px]">
            <span className="size-[8px] shrink-0 rounded-full bg-(--yz-accent)" aria-hidden="true" />
            <span className="truncate font-mono text-[11.5px] text-(--ink-1)">
              git {environment.version} · {environment.root}
            </span>
          </div>
        )}
      </SettingCard>

      <SettingCard
        label={t("gitSettings.remoteCheckLabel")}
        sub={t("gitSettings.remoteCheckDescription")}
      >
        <div
          role="group"
          aria-label={t("gitSettings.remoteCheckAriaLabel")}
          className="flex gap-[4px] rounded-[10px] bg-(--paper-2) p-[3px]"
        >
          {remoteCheckModes.map((option) => {
            const active = option.id === remoteCheck.mode
            return (
              <button
                key={option.id}
                type="button"
                aria-pressed={active}
                onClick={() => setRemoteCheck({ ...remoteCheck, mode: option.id })}
                className={cn(
                  "flex h-[28px] flex-1 items-center justify-center rounded-[8px] text-[11.5px] transition-all duration-[140ms] ease-(--ease-out)",
                  active
                    ? "bg-(--yz-solid) font-semibold text-(--ink-0) shadow-(--shadow-xs)"
                    : "font-medium text-(--ink-3) hover:text-(--ink-1)"
                )}
              >
                {option.label}
              </button>
            )
          })}
        </div>

        <label className="mt-[12px] flex items-center justify-between gap-[10px]">
          <span className="text-[12px] text-(--ink-2)">{t("gitSettings.intervalLabel")}</span>
          <span className="flex items-center gap-[6px]">
            <input
              type="number"
              min={30}
              value={intervalText}
              onChange={(e) => setIntervalText(e.target.value)}
              onBlur={commitInterval}
              className="h-[28px] w-[76px] rounded-[8px] border border-(--line-1) bg-(--paper-0) px-[9px] text-right font-mono text-[11.5px] text-(--ink-1) tabular-nums outline-none focus:border-(--yz-accent)"
            />
            <span className="text-[11px] text-(--ink-3)">{t("gitSettings.seconds")}</span>
          </span>
        </label>
      </SettingCard>
    </div>
  )
}
