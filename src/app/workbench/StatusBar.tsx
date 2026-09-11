import { ResourceUsagePopover } from "./ResourceUsagePopover";
import { Button } from "@/components/ui/button";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronUp, GitBranch } from "lucide-react";

import { BranchPopover } from "@/workbench/git/BranchPopover";
import { contextMenuHandler } from "@/state/contextMenuStore";
import { changedPathSet, useGitStore } from "@/state/gitStore";
import { isFileTab } from "@/lib/markdownPreviewTab";
import { useWorkspaceStore } from "@/state/workspaceStore";
import { usePerfStore } from "@/state/perfStore";
import { languageFromPath } from "@/lib/types";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

export function StatusBar() {
  const { t } = useTranslation("workbench");
  const groups = useWorkspaceStore((s) => s.groups);
  const activeGroupIndex = useWorkspaceStore((s) => s.activeGroupIndex);
  const rawActivePath = groups[activeGroupIndex]?.activePath ?? null;
  const activeTab = rawActivePath
    ? groups[activeGroupIndex]?.tabs.find((tab) => tab.path === rawActivePath)
    : undefined;
  const activePath = activeTab && isFileTab(activeTab) ? activeTab.path : null;
  const lineEnding = activePath ? activeTab?.lineEnding : undefined;
  const setLineEnding = useWorkspaceStore((s) => s.setLineEnding);

  const environment = useGitStore((s) => s.environment);
  const status = useGitStore((s) => s.status);
  const remoteIncoming = useGitStore((s) => s.remoteIncoming);
  const remoteMode = useGitStore((s) => s.remoteCheck.mode);

  const [branchOpen, setBranchOpen] = useState(false);

  const ready = environment?.status === "ready";
  const branchName = !ready
    ? "main"
    : status?.detached
      ? status.headOid.slice(0, 7)
      : (status?.branch ?? "main");

  const ahead = status?.ahead ?? 0;
  const behind = status?.behind ?? 0;
  const conflictCount = status?.conflicted.length ?? 0;
  const conflicted = conflictCount > 0 || status?.inProgress != null;
  const changedCount = changedPathSet(status).size;

  // §6.3 behind indicator: autofetch renders the real count once fetched,
  // probe mode only knows "incoming yes/no" so it shows a dot.
  const showBehindCount = remoteMode === "autofetch" && behind > 0;
  const showIncomingDot = remoteMode === "probe" && remoteIncoming === "yes";

  // F1 perf chip：主要數字是 app 本體 + 所有 Yuzora-owned 子行程的總和（#22），
  // title 再拆出「App 本體 / 子行程數」。cpuPercent is sysinfo's raw value;
  // memory shows decimal MB to line up with Activity Monitor. Hidden until the
  // first poll produces a snapshot.
  const perf = usePerfStore((s) => s.snapshot);
  // #40 §3.5：採樣失敗是有界的滾動視窗狀態（見 perfStore），恢復後會自行過期。
  const perfOutcomes = usePerfStore((s) => s.outcomes);
  const perfLastError = usePerfStore((s) => s.lastError);
  const perfFailures = perfOutcomes.filter((outcome) => outcome === "failed").length;
  // `Ok(None)`（後端有回應但沒有資料）與 reject 一樣是「這次沒量到」，只是路徑
  // 不同。兩者都必須讓 chip 帶警示，否則 chip 會無聲消失、log 同時寫下「全 0、
  // 失敗 0 次」——與「真的用 0 bytes 且一切正常」無法區分。
  const perfEmpty = perfOutcomes.filter((outcome) => outcome === "empty").length;
  const perfUnhealthy = perfFailures + perfEmpty;
  const perfSkipped = perfOutcomes.filter(
    (outcome) => outcome === "skipped_no_focus",
  ).length;
  const perfText = perf
    ? `${Math.round(perf.cpuPercent)}% · ${Math.round(perf.memoryBytes / 1_000_000)}MB`
    : null;
  // 優先序：數值本身仍以最新快照為準（每次 poll 都更新），異常只**附加**一個
  // 標記而不取代它。快照還沒有時才單獨顯示異常——否則採樣一直失敗就會退回舊行為
  // （chip 隱藏），失敗又變回看不見。
  const perfChipText =
    perfText !== null
      ? perfUnhealthy > 0
        ? `${perfText} ⚠`
        : perfText
      : perfUnhealthy > 0
        ? t("statusBar.perfUnavailable")
        : null;
  const perfTitle = perf
    ? t("statusBar.perfTitle", {
        appCpu: Math.round(perf.appCpuPercent),
        appMemory: Math.round(perf.appMemoryBytes / 1_000_000),
        descendants: perf.descendantCount,
        webviewMemory: Math.round(perf.webviewMemoryBytes / 1_000_000),
        webviews: perf.webviewCount,
        toolsMemory: Math.round(perf.managedToolsMemoryBytes / 1_000_000),
        tools: perf.managedToolsCount,
      })
    : undefined;
  const perfSamplingTitle =
    perfUnhealthy > 0
      ? t("statusBar.perfSamplingFailed", {
          failures: perfFailures,
          empty: perfEmpty,
          attempts: perfOutcomes.length,
          skipped: perfSkipped,
          error: perfLastError ?? "—",
        })
      : undefined;
  const perfFullTitle = [perfTitle, perfSamplingTitle].filter(Boolean).join("\n") || undefined;
  const lineEndingLabel = lineEnding
    ? t(`statusBar.lineEnding.${lineEnding}`)
    : null;

  const branchButton = (
    <button
      type="button"
      title={branchName}
      disabled={!ready}
      aria-expanded={ready ? branchOpen : undefined}
      aria-haspopup={ready ? "dialog" : undefined}
      onClick={() => setBranchOpen((v) => !v)}
      onContextMenu={contextMenuHandler({ kind: "status", repositoryRoot: ready ? environment.root : null })}
      className="flex h-[22px] min-w-0 max-w-[35%] shrink items-center gap-[6px] whitespace-nowrap rounded-[7px] px-[9px] transition-colors duration-150 hover:bg-[rgba(var(--yz-accent-rgb),0.14)] disabled:cursor-default disabled:hover:bg-transparent"
    >
      <span className="size-[7px] shrink-0 rounded-full bg-(--yz-accent)" aria-hidden="true" />
      <GitBranch className="size-[12px] shrink-0" aria-hidden="true" />
      <span
        className="min-w-0 truncate font-medium"
        style={{ color: conflicted ? "var(--status-d)" : "var(--ink-1)" }}
      >
        {branchName}
      </span>
      {ready && ahead > 0 && (
        <span className="ml-[4px]" style={{ color: "var(--status-m)" }}>
          ↑{ahead}
        </span>
      )}
      {ready && showBehindCount && (
        <span className="ml-[5px]" style={{ color: "#c8521f" }}>
          ↓{behind}
        </span>
      )}
      {ready && showIncomingDot && (
        <span className="ml-[5px]" style={{ color: "#c8521f" }}>
          ↓•
        </span>
      )}
      <ChevronUp className={`ml-[2px] size-[11px] shrink-0 text-(--ink-3) transition-transform ${branchOpen ? "rotate-180" : ""}`} aria-hidden="true" />
    </button>
  );

  const langLabel = activePath ? languageFromPath(activePath) : "";

  return (
    <footer
      aria-label={t("statusBar.ariaLabel")}
      onContextMenu={contextMenuHandler({ kind: "general" })}
      className="flex h-[30px] min-w-0 shrink-0 whitespace-nowrap items-center gap-1 border-t border-(--line-1) bg-(--yz-glass-strong) px-2 font-mono text-[11.5px] text-(--ink-2) backdrop-blur-[20px] backdrop-saturate-[1.5]"
    >
      <span className="rounded-[6px] px-[6px] font-medium text-(--ink-1)">Yuzora</span>

      {ready ? (
        <BranchPopover open={branchOpen} onOpenChange={setBranchOpen} trigger={branchButton} />
      ) : (
        branchButton
      )}

      {/* §6 L1260 changed count — amber text, square dot (hidden at 0) */}
      {ready && changedCount > 0 && (
        <span
          className="ml-[13px] flex items-center gap-[5px]"
          style={{ color: "#9a6512" }}
        >
          <span
            className="size-[6px] rounded-[2px]"
            style={{ background: "#d68a0c" }}
            aria-hidden="true"
          />
          {changedCount}
        </span>
      )}

      {/* §6 L1261 conflict count — danger text, bold "!" (hidden at 0) */}
      {ready && conflictCount > 0 && (
        <span
          className="ml-[11px] flex items-center gap-[4px]"
          style={{ color: "#c2293f" }}
        >
          <span className="font-bold" aria-hidden="true">
            !
          </span>
          {conflictCount}
        </span>
      )}


      <div className="flex-1" />

      {perfChipText && (
        <ResourceUsagePopover trigger={<Button variant="ghost" size="sm"
          title={perfFullTitle}
          data-testid="status-perf-chip"
          data-perf-sampling-failures={perfFailures}
          data-perf-sampling-empty={perfEmpty}
          className="resource-usage-trigger h-6 px-1.5"
        >
          {perfChipText}<ChevronUp aria-hidden="true" />
        </Button>} />
      )}

      {activePath && lineEnding && lineEndingLabel && (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              aria-label={t("statusBar.lineEnding.ariaLabel", { value: lineEndingLabel })}
              className="flex h-[22px] items-center gap-1 rounded-[6px] px-[6px] transition-colors duration-150 hover:bg-[rgba(var(--yz-accent-rgb),0.14)]"
            >
              {lineEndingLabel}
              <ChevronUp className="size-[11px] text-(--ink-3)" aria-hidden="true" />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent side="top" align="end" className="w-40">
            <DropdownMenuLabel>{t("statusBar.lineEnding.menuLabel")}</DropdownMenuLabel>
            <DropdownMenuRadioGroup
              value={lineEnding === "mixed" ? "" : lineEnding}
              onValueChange={(value) => {
                if (value === "lf" || value === "crlf") {
                  setLineEnding(activePath, value);
                }
              }}
            >
              <DropdownMenuRadioItem value="lf">
                {t("statusBar.lineEnding.useLf")}
              </DropdownMenuRadioItem>
              <DropdownMenuRadioItem value="crlf">
                {t("statusBar.lineEnding.useCrlf")}
              </DropdownMenuRadioItem>
            </DropdownMenuRadioGroup>
          </DropdownMenuContent>
        </DropdownMenu>
      )}

      <span className="min-w-0 max-w-[30%] shrink truncate rounded-[6px] px-[6px]" title={langLabel}>
        {activePath ? langLabel : t("statusBar.noFileOpen")}
      </span>
    </footer>
  );
}
