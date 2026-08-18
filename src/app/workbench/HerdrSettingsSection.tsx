import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"

import { Button } from "@/components/ui/button"
import { ScrollArea } from "@/components/ui/scroll-area"
import {
  herdrBinarySourceGet,
  herdrBinarySourceSet,
  herdrWslDistributions
} from "@/lib/herdrIpc"
import { sameHerdrRuntimeTarget } from "@/lib/herdrRuntime"
import type {
  HerdrBinarySource,
  HerdrBinarySourceInfo,
  HerdrRuntimeTarget,
  HerdrWslDistribution
} from "@/lib/herdrTypes"
import { workspacePathForDisplay } from "@/lib/paths"
import { useHerdrStore } from "@/state/herdrStore"

const MANAGED_UNAVAILABLE_PREFIX = "Yuzora-managed Herdr binary is unavailable at "
const OVERRIDE_NOT_EXECUTABLE_PREFIX = "herdr binary override is not executable: "

function formatHerdrDiagnosticReason(reason: string): string {
  for (const prefix of [MANAGED_UNAVAILABLE_PREFIX, OVERRIDE_NOT_EXECUTABLE_PREFIX]) {
    if (reason.startsWith(prefix)) {
      return `${prefix}${workspacePathForDisplay(reason.slice(prefix.length))}`
    }
  }
  return reason
}

/**
 * Native binary preference and Runtime Environment are intentionally separate:
 * selecting a WSL distro never changes the host-native managed binary source.
 */
export function HerdrSettingsSection() {
  const { t } = useTranslation("workbench")
  const [info, setInfo] = useState<HerdrBinarySourceInfo | null>(null)
  const [pending, setPending] = useState<HerdrBinarySource | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [restartRequired, setRestartRequired] = useState(false)
  const [distros, setDistros] = useState<HerdrWslDistribution[]>([])
  const [wslError, setWslError] = useState<string | null>(null)
  const [pendingRuntime, setPendingRuntime] = useState<string | null>(null)
  const selectedRuntimeTarget = useHerdrStore((state) => state.selectedRuntimeTarget)
  const runtimeCapabilities = useHerdrStore((state) => state.capabilities)
  const connectionState = useHerdrStore((state) => state.connectionState)
  const runtimeError = useHerdrStore((state) => state.errorMessage)
  const selectRuntimeTarget = useHerdrStore((state) => state.selectRuntimeTarget)

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
    // Listing is the only WSL background action. `wsl.exe --list --quiet`
    // does not launch any distro; connection only happens after a button click.
    void herdrWslDistributions()
      .then((next) => {
        if (!active) return
        setDistros(next)
      })
      .catch((err) => {
        if (!active) return
        setWslError(err instanceof Error ? err.message : String(err))
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

  const onSelectRuntime = async (target: HerdrRuntimeTarget) => {
    const targetKey = target.kind === "native" ? "native" : `wsl:${target.distro}`
    setPendingRuntime(targetKey)
    setWslError(null)
    try {
      await selectRuntimeTarget(target)
    } catch (err) {
      setWslError(err instanceof Error ? err.message : String(err))
    } finally {
      setPendingRuntime(null)
    }
  }

  const configured = info?.configured ?? "global"
  const active = info?.resolved ?? info?.active ?? info?.configured ?? null
  const selectedRuntimeLabel = selectedRuntimeTarget.kind === "native"
    ? t("herdrSettings.nativeRuntime")
    : t("herdrSettings.wslRuntime", { distro: selectedRuntimeTarget.distro })
  const configuredAvailable = info
    ? (info.configuredAvailable ?? (info.configured === active ? info.available : false))
    : false

  return (
    <div className="flex flex-col gap-[14px]">
      <div className="rounded-[12px] border border-(--line-1) bg-(--paper-0) px-[14px] py-[12px]">
        <div className="text-[13px] font-semibold text-(--ink-1)">
          {t("herdrSettings.runtimeEnvironments")}
        </div>
        <p className="mt-[4px] text-[11.5px] text-(--ink-3)">
          {t("herdrSettings.runtimeEnvironmentsSub")}
        </p>
        <div className="mt-[12px] flex flex-wrap gap-[8px]" role="group" aria-label={t("herdrSettings.runtimeEnvironments")}>
          <Button
            type="button"
            size="sm"
            variant={selectedRuntimeTarget.kind === "native" ? "default" : "outline"}
            aria-pressed={selectedRuntimeTarget.kind === "native"}
            disabled={pendingRuntime !== null}
            onClick={() => void onSelectRuntime({ kind: "native" })}
          >
            {t("herdrSettings.nativeRuntime")}
          </Button>
        </div>
        {distros.length > 0 && (
          <ScrollArea className="mt-[10px] max-h-44 rounded-[8px] border border-(--line-1)">
            <div className="flex flex-col gap-[6px] p-[8px]" role="list" aria-label={t("herdrSettings.wslDistributions")}>
              {distros.map(({ distro }) => {
                const target = { kind: "wsl", distro } as const
                const selected = sameHerdrRuntimeTarget(selectedRuntimeTarget, target)
                return (
                  <div key={distro} className="flex items-center justify-between gap-[8px]" role="listitem">
                    <span className="min-w-0 truncate font-mono text-[11.5px] text-(--ink-2)">{distro}</span>
                    <Button
                      type="button"
                      size="sm"
                      variant={selected ? "default" : "outline"}
                      aria-pressed={selected}
                      disabled={pendingRuntime !== null}
                      onClick={() => void onSelectRuntime(target)}
                    >
                      {selected ? t("herdrSettings.selectedRuntime") : t("herdrSettings.connectRuntime")}
                    </Button>
                  </div>
                )
              })}
            </div>
          </ScrollArea>
        )}
        {!wslError && distros.length === 0 && (
          <p className="mt-[10px] text-[11.5px] text-(--ink-3)">{t("herdrSettings.noWslDistributions")}</p>
        )}
        <p className="mt-[10px] text-[11.5px] text-(--ink-3)" role="status">
          {t("herdrSettings.selectedRuntimeLabel", {
            runtime: selectedRuntimeLabel
          })}
        </p>
        {selectedRuntimeTarget.kind === "wsl" && (
          <>
            <p className="mt-[6px] text-[11.5px] text-(--ink-3)">
              {t("herdrSettings.wslInstallGuidance", { distro: selectedRuntimeTarget.distro })}
            </p>
            <dl className="mt-[8px] grid grid-cols-[120px_1fr] gap-x-[10px] gap-y-[4px] text-[11.5px] text-(--ink-3)" data-testid="herdr-wsl-transport-diagnostics">
              <dt>{t("herdrSettings.transport")}</dt>
              <dd>{runtimeCapabilities?.transport?.mode ?? t("herdrSettings.cliFallback")}</dd>
              <dt>{t("herdrSettings.transportState")}</dt>
              <dd>{runtimeCapabilities?.transport?.state ?? connectionState}</dd>
              <dt>{t("herdrSettings.proxyGeneration")}</dt>
              <dd>{runtimeCapabilities?.transport?.generation ?? "—"}</dd>
              <dt>{t("herdrSettings.ownedChildren")}</dt>
              <dd>{runtimeCapabilities?.transport?.activeChildren ?? 0}</dd>
              <dt>{t("herdrSettings.coldStart")}</dt>
              <dd>{runtimeCapabilities?.transport?.coldStartMs == null ? "—" : `${runtimeCapabilities.transport.coldStartMs} ms`}</dd>
              <dt>{t("herdrSettings.requestLatency")}</dt>
              <dd>{runtimeCapabilities?.transport?.lastRequestMs == null ? "—" : `${runtimeCapabilities.transport.lastRequestMs} ms / max ${runtimeCapabilities.transport.maxRequestMs} ms`}</dd>
              <dt>{t("herdrSettings.eventDispatchLatency")}</dt>
              <dd>{runtimeCapabilities?.transport?.lastEventDispatchMs == null ? "—" : `${runtimeCapabilities.transport.lastEventDispatchMs} ms / max ${runtimeCapabilities.transport.maxEventDispatchMs} ms`}</dd>
              <dt>{t("herdrSettings.reason")}</dt>
              <dd>{runtimeCapabilities?.transport?.failure ?? runtimeError ?? t("herdrSettings.proxyUnavailable")}</dd>
            </dl>
          </>
        )}
        {wslError && <p role="alert" className="mt-[10px] text-[11.5px] text-destructive">{wslError}</p>}
      </div>

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
          <dd className="break-all font-mono text-[11px]">
            {info?.path ? workspacePathForDisplay(info.path) : "—"}
          </dd>
          <dt>{t("herdrSettings.version")}</dt>
          <dd>{info?.version ?? "—"}</dd>
          <dt>{t("herdrSettings.protocol")}</dt>
          <dd>{info?.protocol ?? "—"}</dd>
          <dt>{t("herdrSettings.availability")}</dt>
          <dd>{info ? info.available ? t("herdrSettings.available") : t("herdrSettings.unavailable") : "—"}</dd>
          <dt>{t("herdrSettings.reason")}</dt>
          <dd>{info?.reason ? formatHerdrDiagnosticReason(info.reason) : "—"}</dd>
          <dt>{t("herdrSettings.configuredTarget")}</dt>
          <dd>{info?.configured ?? "—"}</dd>
          <dt>{t("herdrSettings.targetPath")}</dt>
          <dd className="break-all font-mono text-[11px]">
            {info?.configuredPath ? workspacePathForDisplay(info.configuredPath) : "—"}
          </dd>
          <dt>{t("herdrSettings.version")}</dt>
          <dd>{info?.configuredVersion ?? "—"}</dd>
          <dt>{t("herdrSettings.protocol")}</dt>
          <dd>{info?.configuredProtocol ?? "—"}</dd>
          <dt>{t("herdrSettings.targetAvailability")}</dt>
          <dd>{info ? configuredAvailable ? t("herdrSettings.available") : t("herdrSettings.unavailable") : "—"}</dd>
          <dt>{t("herdrSettings.reason")}</dt>
          <dd>{info?.configuredReason ? formatHerdrDiagnosticReason(info.configuredReason) : "—"}</dd>
        </dl>
        {!configuredAvailable && info?.configured === "default" && (
          <p className="mt-[10px] text-[11.5px] text-(--ink-3)">{t("herdrSettings.defaultUnavailable")}</p>
        )}
        {info?.configurationError && (
          <p role="alert" className="mt-[10px] text-[11.5px] text-destructive">
            {t("herdrSettings.configurationError", { error: info.configurationError })}
          </p>
        )}
        {error && <p role="alert" className="mt-[10px] text-[11.5px] text-destructive">{error}</p>}
      </div>
    </div>
  )
}
