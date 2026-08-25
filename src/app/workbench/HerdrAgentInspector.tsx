import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { AnsiText } from "@/app/workbench/AnsiText"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  dialogMinSize,
} from "@/components/ui/dialog"
import { ScrollArea } from "@/components/ui/scroll-area"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { herdrAgentGet, herdrAgentRead } from "@/lib/herdrIpc"
import { formatHerdrExecutionOrigin } from "@/lib/herdrNormalize"
import type {
  HerdrAgentDetails,
  HerdrAgentInfo,
  HerdrAgentReadResult,
  HerdrReadFormat,
  HerdrReadSource
} from "@/lib/herdrTypes"
import { useHerdrStore } from "@/state/herdrStore"

const SOURCES: HerdrReadSource[] = [
  "visible",
  "recent",
  "recent-unwrapped",
  "detection"
]

export function HerdrAgentInspector({
  open,
  onOpenChange,
  agent
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  agent: HerdrAgentInfo | null
}) {
  const { t } = useTranslation("workbench")
  const sessions = useHerdrStore((s) => s.sessions)
  const selectedSessionName = useHerdrStore((s) => s.selectedSessionName)
  const [details, setDetails] = useState<HerdrAgentDetails | null>(null)
  const [readResult, setReadResult] = useState<HerdrAgentReadResult | null>(null)
  const [source, setSource] = useState<HerdrReadSource>("recent")
  const [format, setFormat] = useState<HerdrReadFormat>("text")
  const [lines, setLines] = useState(120)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const requestGenerationRef = useRef(0)

  const sessionName = agent?.sessionName ?? selectedSessionName ?? null
  const canInspect = useHerdrStore((s) => s.canInspectAgent(sessionName))
  const target = agent?.paneId ?? null
  const stopped = useMemo(() => {
    if (!sessionName) return true
    const session = sessions.find((item) => item.name === sessionName)
    return session ? !session.running : true
  }, [sessionName, sessions])

  const load = useCallback(async () => {
    const generation = ++requestGenerationRef.current
    if (!open || !agent || !target || !sessionName || stopped || !canInspect) {
      setDetails(null)
      setReadResult(null)
      setLoading(false)
      setError(null)
      return
    }
    const requestedLines = Math.min(500, Math.max(20, lines))
    setLoading(true)
    setError(null)
    try {
      const [nextDetails, nextRead] = await Promise.all([
        herdrAgentGet({ sessionName, target }),
        herdrAgentRead({
          sessionName,
          target,
          source,
          format,
          lines: requestedLines,
          stripAnsi: format === "text"
        })
      ])
      if (requestGenerationRef.current !== generation) return
      setDetails(nextDetails)
      setReadResult(nextRead)
    } catch (err) {
      if (requestGenerationRef.current !== generation) return
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      if (requestGenerationRef.current === generation) setLoading(false)
    }
  }, [open, agent, target, sessionName, stopped, canInspect, source, format, lines])

  useEffect(() => {
    let cancelled = false
    queueMicrotask(() => {
      if (!cancelled) void load()
    })
    return () => {
      cancelled = true
      requestGenerationRef.current += 1
    }
  }, [load])

  const originLabel = formatHerdrExecutionOrigin(agent?.executionOrigin)
  const disabledReason = stopped
    ? t("herdrInspector.sessionStopped")
    : !canInspect
      ? t("herdrInspector.unavailable")
      : !target
        ? t("herdrInspector.missingPane")
        : null

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
<DialogContent
        resizeId="herdr-agent-inspector"
        minSize={dialogMinSize(480, 320)}
        className="flex min-h-0 flex-col gap-0 overflow-hidden p-0"
      >
        <DialogHeader className="border-b border-(--line-1) px-[20px] py-[16px]">
          <DialogTitle>{t("herdrInspector.title")}</DialogTitle>
          <div className="flex items-center gap-[8px]">
            <DialogDescription>
              {agent?.title ?? agent?.name ?? t("herdrInspector.untitled")}
            </DialogDescription>
            {originLabel && (
              <Badge
                variant="outline"
                data-testid="herdr-inspector-origin"
                className="h-[18px] border-(--line-2) px-[6px] text-[9px] font-normal text-(--ink-3)"
              >
                {originLabel}
              </Badge>
            )}
          </div>
        </DialogHeader>

        <div className="flex min-h-0 flex-1 flex-col gap-[12px] px-[20px] py-[16px]">
          {disabledReason ? (
            <p role="status" className="text-[12px] text-(--ink-3)">
              {disabledReason}
            </p>
          ) : (
            <>
              <div className="grid grid-cols-2 gap-[8px] text-[12px]">
                <Meta label={t("herdrInspector.status")} value={details?.agentStatus ?? agent?.status ?? "—"} />
                <Meta label={t("herdrInspector.pane")} value={target ?? "—"} />
                <Meta label={t("herdrInspector.space")} value={details?.workspaceId ?? agent?.workspaceId ?? "—"} />
                <Meta label={t("herdrInspector.tab")} value={details?.tabId ?? agent?.tabId ?? "—"} />
                <Meta label={t("herdrInspector.cwd")} value={details?.cwd ?? details?.foregroundCwd ?? "—"} />
                <Meta
                  label={t("herdrInspector.ready")}
                  value={
                    details?.interactiveReady == null
                      ? "—"
                      : details.interactiveReady
                        ? t("herdrInspector.yes")
                        : t("herdrInspector.no")
                  }
                />
                <Meta label={t("herdrInspector.revision")} value={String(details?.revision ?? "—")} />
                <Meta
                  label={t("herdrInspector.labels")}
                  value={
                    details?.stateLabels && Object.keys(details.stateLabels).length > 0
                      ? Object.entries(details.stateLabels)
                          .map(([key, value]) => `${key}=${value}`)
                          .join(", ")
                      : "—"
                  }
                />
              </div>

              <div className="flex flex-wrap items-center gap-[8px]">
                <label className="text-[11px] text-(--ink-3)" htmlFor="herdr-read-source">
                  {t("herdrInspector.source")}
                </label>
                <select
                  id="herdr-read-source"
                  className="h-[30px] rounded-[8px] border border-(--line-2) bg-(--paper-0) px-[8px] text-[12px]"
                  value={source}
                  onChange={(event) => setSource(event.target.value as HerdrReadSource)}
                >
                  {SOURCES.map((item) => (
                    <option key={item} value={item}>
                      {item}
                    </option>
                  ))}
                </select>
                <Tabs value={format} onValueChange={(value) => setFormat(value as HerdrReadFormat)}>
                  <TabsList>
                    <TabsTrigger value="text">text</TabsTrigger>
                    <TabsTrigger value="ansi">ansi</TabsTrigger>
                  </TabsList>
                </Tabs>
                <label className="text-[11px] text-(--ink-3)" htmlFor="herdr-read-lines">
                  {t("herdrInspector.lines")}
                </label>
                <input
                  id="herdr-read-lines"
                  type="number"
                  min={20}
                  max={500}
                  value={lines}
                  onChange={(event) => {
                    const next = Number(event.target.value)
                    setLines(Number.isFinite(next) ? Math.min(500, Math.max(20, next)) : 120)
                  }}
                  className="h-[30px] w-[72px] rounded-[8px] border border-(--line-2) bg-(--paper-0) px-[8px] text-[12px]"
                />
                <Button type="button" size="sm" onClick={() => void load()} disabled={loading}>
                  {loading ? t("herdrInspector.loading") : t("herdrInspector.refresh")}
                </Button>
              </div>

              {error && (
                <p role="alert" className="text-[12px] text-destructive">
                  {error}
                </p>
              )}

              <Tabs defaultValue="output" className="min-h-0 flex-1">
                <TabsList>
                  <TabsTrigger value="output">{t("herdrInspector.output")}</TabsTrigger>
                </TabsList>
                <TabsContent value="output" className="min-h-0">
<ScrollArea className="min-h-0 h-full rounded-[10px] border border-(--line-2)">
                    <pre className="whitespace-pre-wrap break-words p-[12px] font-mono text-[11px] text-(--ink-1)">
                      {loading ? (
                        t("herdrInspector.loading")
                      ) : readResult?.text ? (
                        readResult.format === "ansi" ? (
                          <AnsiText text={readResult.text} />
                        ) : (
                          readResult.text
                        )
                      ) : (
                        t("herdrInspector.emptyOutput")
                      )}
                    </pre>
                  </ScrollArea>
                  {readResult?.truncated && (
                    <p className="mt-[6px] text-[11px] text-(--ink-3)">
                      {t("herdrInspector.truncated")}
                    </p>
                  )}
                </TabsContent>
              </Tabs>
            </>
          )}
        </div>
      </DialogContent>
    </Dialog>
  )
}

function Meta({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0 rounded-[8px] border border-(--line-1) bg-(--paper-0) px-[10px] py-[8px]">
      <div className="text-[10px] font-semibold tracking-[0.06em] text-(--ink-4) uppercase">
        {label}
      </div>
      <div className="mt-[2px] truncate text-[12px] text-(--ink-1)">{value}</div>
    </div>
  )
}
