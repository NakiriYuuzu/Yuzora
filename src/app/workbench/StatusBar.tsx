import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronUp, GitBranch } from "lucide-react";

import { BranchPopover } from "@/workbench/git/BranchPopover";
import { contextMenuHandler } from "@/state/contextMenuStore";
import { changedPathSet, useGitStore } from "@/state/gitStore";
import { isFileTab } from "@/lib/markdownPreviewTab";
import { useWorkspaceStore } from "@/state/workspaceStore";
import { useLspStore } from "@/state/lspStore";
import { usePreviewStore } from "@/state/previewStore";
import { usePerfStore } from "@/state/perfStore";
import { useUiStore } from "@/state/uiStore";
import { documentGeneration, getDocument } from "@/editor/documentRegistry";
import { fileGradeOf, languageFromPath, lspLanguageOf } from "@/lib/types";
import type { FileGrade, LspDisplayState } from "@/lib/types";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

// Right-segment LSP labels/colours (design reference §6). State names stay in
// English (technical terms); colours reuse the shared status tokens.
const LSP_STATE_LABEL: Record<LspDisplayState, string> = {
  ready: "Ready",
  starting: "Starting",
  failed: "Failed",
  missing: "Missing",
  syntaxOnly: "Syntax only",
};

const LSP_STATE_COLOR: Record<LspDisplayState, string> = {
  ready: "var(--status-a)",
  starting: "var(--ink-3)",
  failed: "var(--status-d)",
  missing: "var(--status-r)",
  syntaxOnly: "var(--ink-3)",
};

// First non-empty line of a server's stderr, for the Failed-state tooltip.
function lspErrorSummary(message: string | null): string | undefined {
  if (!message) return undefined;
  const line = message
    .split("\n")
    .map((l) => l.trim())
    .find((l) => l.length > 0);
  return line ? line.slice(0, 200) : undefined;
}

/**
 * Status bar — design reference §6. Left side carries the workspace tag and the
 * Git branch chip (real data via useGitStore, opens BranchPopover); right side
 * shows the active file's language and live LSP state (useLspStore.displayFor);
 * Missing/Failed open the LSP settings for that language.
 */
export function StatusBar() {
  const { t } = useTranslation("workbench");
  const workspacePath = useWorkspaceStore((s) => s.workspacePath);
  const groups = useWorkspaceStore((s) => s.groups);
  const activeGroupIndex = useWorkspaceStore((s) => s.activeGroupIndex);
  const rawActivePath = groups[activeGroupIndex]?.activePath ?? null;
  const activeTab = rawActivePath
    ? groups[activeGroupIndex]?.tabs.find((tab) => tab.path === rawActivePath)
    : undefined;
  const activePath = activeTab && isFileTab(activeTab) ? activeTab.path : null;
  const lineEnding = activePath ? activeTab?.lineEnding : undefined;
  const setLineEnding = useWorkspaceStore((s) => s.setLineEnding);
  const devServer = usePreviewStore((s) =>
    workspacePath ? s.devServerForWorkspace(workspacePath) : null
  );

  const environment = useGitStore((s) => s.environment);
  const status = useGitStore((s) => s.status);
  const remoteIncoming = useGitStore((s) => s.remoteIncoming);
  const remoteMode = useGitStore((s) => s.remoteCheck.mode);

  const [branchOpen, setBranchOpen] = useState(false);

  const openSettings = useUiStore((s) => s.openSettings);
  const displayFor = useLspStore((s) => s.displayFor);
  // Subscribed so the segment re-renders when a server's process state changes;
  // displayFor reads servers/initialized, and the Failed tooltip reads servers.
  const lspServers = useLspStore((s) => s.servers);
  const lspInitialized = useLspStore((s) => s.initialized);

  // The active file's LSP grade decides Ready/… vs Syntax only. Only the four
  // LSP languages need it; other files are Syntax only regardless, so skip the
  // read for them. The active file is already open, so getDocument hits the
  // documentRegistry cache — no extra openFile IPC. reloadDocument bumps
  // documentGeneration only AFTER a successful re-fetch, and its callers
  // (ExternalChangeBridge / the resolver's takeDiskReload) flip a workspaceStore
  // field on both outcomes. So a successful reload re-renders this bar with a new
  // generation and the effect re-derives the grade; a failed reload leaves the
  // generation — and the live pane/buffer — untouched, so the segment keeps its
  // current state.
  const generation = activePath ? documentGeneration(activePath) : 0;
  const [grade, setGrade] = useState<FileGrade | null>(null);
  useEffect(() => {
    if (!activePath || !lspLanguageOf(activePath)) {
      setGrade(null);
      return;
    }
    let disposed = false;
    void getDocument(activePath)
      .then((entry) => {
        if (!disposed) setGrade(fileGradeOf(entry.result));
      })
      .catch(() => {
        // Read failed (e.g. a stale tab whose file was deleted): fall back to a
        // non-full grade so the segment shows Syntax only rather than an
        // optimistic live LSP state. displayFor maps every non-full grade to
        // syntaxOnly; "limited" is the neutral stand-in for "nothing to mount on".
        if (!disposed) setGrade("limited");
      });
    return () => {
      disposed = true;
    };
  }, [activePath, generation]);

  const lsp = useMemo(
    () => (activePath ? displayFor(activePath, grade ?? "full") : null),
    // lspServers/lspInitialized feed displayFor; list them so the memo
    // recomputes when the LSP layer reports a new state.
    [activePath, grade, displayFor, lspServers, lspInitialized],
  );

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
  const devServerPort =
    devServer?.status.status === "running"
      ? (devServer.status.port ?? devServer.port)
      : null;

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
      className="flex h-[22px] items-center gap-[6px] rounded-[7px] px-[9px] transition-colors duration-150 hover:bg-[rgba(var(--yz-accent-rgb),0.14)] disabled:cursor-default disabled:hover:bg-transparent"
    >
      <span className="size-[7px] rounded-full bg-(--yz-accent)" aria-hidden="true" />
      <GitBranch className="size-[12px]" aria-hidden="true" />
      <span
        className="font-medium"
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
      <ChevronUp className={`ml-[2px] size-[11px] text-(--ink-3) transition-transform ${branchOpen ? "rotate-180" : ""}`} aria-hidden="true" />
    </button>
  );

  const lspLang = activePath ? lspLanguageOf(activePath) : null;
  const langLabel = activePath ? languageFromPath(activePath) : "";
  const lspState = lsp?.state ?? "syntaxOnly";
  const lspServerName = lsp?.serverId ?? "";
  const lspText =
    lspState === "syntaxOnly"
      ? `${langLabel} · Syntax only`
      : lspServerName
        ? `${langLabel} · ${lspServerName} ${LSP_STATE_LABEL[lspState]}`
        : `${langLabel} · ${LSP_STATE_LABEL[lspState]}`;
  const lspClickable = lspState === "failed" || lspState === "missing";
  const lspServerInfo = lspLang ? lspServers[lspLang] : undefined;
  const lspTitle =
    lspState === "failed"
      ? lspErrorSummary(lspServerInfo?.lastError ?? null)
      : lspState === "missing" && lspServerInfo?.status.status === "missing"
        ? lspServerInfo.status.installHint
        : undefined;

  return (
    <footer
      aria-label={t("statusBar.ariaLabel")}
      onContextMenu={contextMenuHandler({
        kind: "status",
        repositoryRoot: environment?.status === "ready" ? environment.root : null,
      })}
      className="flex h-[30px] shrink-0 items-center gap-1 border-t border-(--line-1) bg-(--yz-glass-strong) px-2 font-mono text-[11.5px] text-(--ink-2) backdrop-blur-[20px] backdrop-saturate-[1.5]"
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

      {devServerPort != null && (
        <span
          className="ml-[13px] rounded-[6px] px-[6px] py-[2px]"
          style={{ color: "var(--status-a)", background: "rgba(var(--yz-accent-rgb),0.10)" }}
        >
          {t("statusBar.devPort", { port: devServerPort })}
        </span>
      )}

      <div className="flex-1" />

      {perfChipText && (
        <span
          title={perfFullTitle}
          data-testid="status-perf-chip"
          data-perf-sampling-failures={perfFailures}
          data-perf-sampling-empty={perfEmpty}
          className="rounded-[6px] px-[6px] text-(--ink-3)"
        >
          {perfChipText}
        </span>
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

      {!activePath ? (
        <span className="rounded-[6px] px-[6px] text-(--ink-3)">{t("statusBar.noFileOpen")}</span>
      ) : lspClickable ? (
        <button
          type="button"
          title={lspTitle}
          onClick={() => openSettings("lsp", lsp?.language ?? "")}
          className="cursor-pointer rounded-[6px] px-[6px] transition-colors duration-150 hover:bg-[rgba(var(--yz-accent-rgb),0.14)]"
          style={{ color: LSP_STATE_COLOR[lspState] }}
        >
          {lspText}
        </button>
      ) : (
        <span
          className="rounded-[6px] px-[6px]"
          style={{ color: LSP_STATE_COLOR[lspState] }}
        >
          {lspText}
        </span>
      )}
    </footer>
  );
}
