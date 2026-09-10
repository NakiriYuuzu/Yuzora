import { useId, useLayoutEffect, useRef, useState, type ReactNode } from "react"
import { useTranslation } from "react-i18next"
import { AppWindow, Cpu, MemoryStick, Monitor, Terminal, X } from "lucide-react"
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover"
import { Button } from "@/components/ui/button"
import { ScrollArea } from "@/components/ui/scroll-area"
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group"
import { useOverlayPresence } from "@/state/overlayStore"
import { usePerfStore } from "@/state/perfStore"
import "./resource-usage.css"

export function ResourceUsagePopover({ trigger }: { trigger: ReactNode }) {
  const { t } = useTranslation("resourceUsage")
  const [open, setOpen] = useState(false)
  const [sort, setSort] = useState<"memory" | "cpu">("memory")
  const snapshot = usePerfStore((state) => state.snapshot)
  const outcomes = usePerfStore((state) => state.outcomes)
  const lastError = usePerfStore((state) => state.lastError)
  const listViewport = useRef<HTMLDivElement>(null)
  const titleId = useId(), descriptionId = useId()
  useOverlayPresence(open)
  useLayoutEffect(() => { listViewport.current?.scrollTo?.({ top: 0 }) }, [sort])
  const categories = snapshot ? [
    { id: "app", memory: snapshot.appMemoryBytes, cpu: snapshot.appCpuPercent, count: 1, icon: AppWindow },
    { id: "webview", memory: snapshot.webviewMemoryBytes, cpu: snapshot.webviewCpuPercent, count: snapshot.webviewCount, icon: Monitor },
    { id: "tools", memory: snapshot.managedToolsMemoryBytes, cpu: snapshot.managedToolsCpuPercent, count: snapshot.managedToolsCount, icon: Terminal },
  ] : []
  const rows = [...categories].sort((a, b) => b[sort] - a[sort])
  const total = snapshot ? sort === "memory" ? snapshot.memoryBytes : snapshot.cpuPercent : null
  const failures = outcomes.filter((value) => value === "failed").length
  const empty = outcomes.filter((value) => value === "empty").length
  const skipped = outcomes.filter((value) => value === "skipped_no_focus").length
  const mb = (bytes: number) => (bytes / 1_000_000).toFixed(1)

  return <Popover open={open} onOpenChange={setOpen}>
    <PopoverTrigger asChild>{trigger}</PopoverTrigger>
    <PopoverContent side="top" align="end" sideOffset={8} collisionPadding={12}
      className="resource-usage-popover" aria-labelledby={titleId} aria-describedby={descriptionId}>
      <header className="resource-usage-heading">
        <h2 id={titleId}>{t("title")}</h2>
        <Button variant="ghost" size="icon-xs" aria-label={t("close")} onClick={() => setOpen(false)}><X aria-hidden="true" /></Button>
      </header>
      <p id={descriptionId} className="resource-usage-scope">{t("scope")}</p>
      <div className="resource-usage-totals">
        <div><span><MemoryStick aria-hidden="true" />{t("memory")}</span><strong>{snapshot ? mb(snapshot.memoryBytes) : "—"}<small> MB</small></strong></div>
        <div><span><Cpu aria-hidden="true" />CPU</span><strong>{snapshot ? snapshot.cpuPercent.toFixed(1) : "—"}<small> %</small></strong></div>
      </div>
      <div className="resource-usage-categories" aria-label={t("categorySummary")}>
        {categories.map((category) => <div key={category.id}><span>{t(`category.${category.id}`)}</span><strong>{mb(category.memory)} <small>MB</small></strong></div>)}
      </div>
      <div className="resource-usage-sort">
        <span>{t("largestFirst")}</span>
        <ToggleGroup type="single" value={sort} onValueChange={(value) => { if (value === "memory" || value === "cpu") setSort(value) }} aria-label={t("sortLabel")}>
          <ToggleGroupItem value="memory">{t("memory")}</ToggleGroupItem><ToggleGroupItem value="cpu">CPU</ToggleGroupItem>
        </ToggleGroup>
      </div>
      <ScrollArea className="resource-usage-list" viewportRef={listViewport} focusable viewportProps={{ "aria-label": t("categoryList") }}>
        {!snapshot && <p className="resource-usage-scope" role="status">{t("noSample")}</p>}
        <ol aria-label={t("categoryList")}>
          {rows.map((row) => {
            const Icon = row.icon
            const share = total && total > 0 ? row[sort] / total * 100 : null
            return <li key={row.id} data-resource-id={row.id} className="resource-usage-row">
              <Icon aria-hidden="true" />
              <div className="resource-usage-source"><strong>{t(`category.${row.id}`)}</strong><span>{t("processCount", { count: row.count })}</span></div>
              <div className="resource-usage-values"><strong>{sort === "memory" ? `${mb(row.memory)} MB` : `${row.cpu.toFixed(1)} %`}</strong><span>{share === null ? t("noShare") : t("share", { value: share.toFixed(1) })}</span></div>
              <div className="resource-usage-meter" aria-hidden="true"><span style={{ width: `${share === null ? 0 : Math.min(100, Math.max(0, share))}%` }} /></div>
            </li>
          })}
        </ol>
      </ScrollArea>
      <footer className="resource-usage-footnote">
        <p>{t("detailLimit")}</p><p>{t("measurement")}</p>
        {outcomes.length > 0 && <p role="status">{t("sampling", { failures, empty, skipped, attempts: outcomes.length })}</p>}
        {failures > 0 && lastError && <p>{lastError}</p>}
      </footer>
    </PopoverContent>
  </Popover>
}
