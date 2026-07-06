import { useEffect, useRef, useState } from "react"
import {
  AlertTriangle,
  ArrowLeft,
  ArrowRight,
  FileText,
  Monitor,
  MonitorPlay,
  Play,
  RotateCw,
  Smartphone,
  Square,
} from "lucide-react"

import { EmptyState } from "@/app/workbench/EmptyState"
import { loadPreviewSettings } from "@/app/workbench/SettingsDialog"
import { devServerDetect, devServerStart, devServerStop } from "@/lib/ipc"
import { strings } from "@/lib/i18n"
import type { DevServerCandidate, DevServerInfo, DevServerStatus } from "@/lib/types"
import { cn } from "@/lib/utils"
import { PreviewFrame } from "@/preview/PreviewFrame"
import { suppressContextMenu } from "@/state/contextMenuStore"
import { type PreviewNavState, usePreviewStore } from "@/state/previewStore"
import { useUiStore } from "@/state/uiStore"
import { useWorkspaceStore } from "@/state/workspaceStore"

const EMPTY_NAV: PreviewNavState = {
  url: null,
  backStack: [],
  forwardStack: [],
  reloadNonce: 0,
  frame: "full",
}

type FlowState = "idle" | "detecting" | "occupied" | "no-candidates"

function statusLabel(status: DevServerStatus | null): string {
  if (!status) return strings.preview.noDevServer
  if (status.status === "starting") return "Starting"
  if (status.status === "running") return "Running"
  if (status.status === "exited") return "Exited"
  return "Failed"
}

function statusTone(status: DevServerStatus | null): string {
  if (status?.status === "running") return "border-(--line-1) bg-[#e7f8ed] text-[#17753a]"
  if (status?.status === "starting") return "border-(--line-1) bg-[#eef5ff] text-[#2666b8]"
  if (status?.status === "failed") return "border-[#f0c4c4] bg-[#fff1f1] text-[#b4232a]"
  return "border-(--line-1) bg-(--yz-sunk) text-(--ink-3)"
}

function portUrl(port: number): string {
  return `http://localhost:${port}`
}

function localhostPort(rawUrl: string | null): number | null {
  if (!rawUrl) return null
  try {
    const url = new URL(rawUrl)
    if (url.protocol !== "http:" || !["localhost", "127.0.0.1"].includes(url.hostname)) return null
    return Number.parseInt(url.port, 10)
  } catch {
    return null
  }
}

function firstCandidatePort(candidate: DevServerCandidate | null): number | null {
  return candidate?.likelyPort ?? null
}

function parsePortOverride(value: string): { port: number | null; hint: string | null } {
  const trimmed = value.trim()
  if (!trimmed) return { port: null, hint: null }
  if (!/^\d+$/.test(trimmed)) {
    return { port: null, hint: `Preview port override "${trimmed}" 無效，已改用偵測到的 port。` }
  }

  const port = Number(trimmed)
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    return { port: null, hint: `Preview port override "${trimmed}" 無效，已改用偵測到的 port。` }
  }
  return { port, hint: null }
}

export function PreviewPanel() {
  const workspace = useWorkspaceStore((s) => s.workspacePath)
  const devServer = usePreviewStore((s) =>
    workspace ? s.devServerForWorkspace(workspace) : null
  )
  const navMap = usePreviewStore((s) => s.nav)
  const nav = workspace ? navMap[workspace] ?? EMPTY_NAV : EMPTY_NAV
  const navigate = usePreviewStore((s) => s.navigate)
  const openSettings = useUiStore((s) => s.openSettings)
  const [flowState, setFlowState] = useState<FlowState>("idle")
  const [candidates, setCandidates] = useState<DevServerCandidate[]>([])
  const [runningPorts, setRunningPorts] = useState<number[]>([])
  const [portOverride, setPortOverride] = useState("")
  const [portOverrideHint, setPortOverrideHint] = useState<string | null>(null)
  const [localError, setLocalError] = useState<string | null>(null)

  const status = devServer?.status ?? null
  const primaryCandidate = candidates[0] ?? null
  const canGoBack = !!workspace && nav.backStack.length > 0
  const canGoForward = !!workspace && nav.forwardStack.length > 0
  const canReload = !!workspace && !!nav.url
  const frameWidth = nav.frame === "mobile" ? 390 : "100%"
  const attemptRef = useRef(0)

  const nextAttempt = () => {
    attemptRef.current += 1
    return attemptRef.current
  }

  const isAttemptLive = (attemptWorkspace: string, attempt: number) =>
    attemptWorkspace === useWorkspaceStore.getState().workspacePath && attempt === attemptRef.current

  useEffect(() => {
    if (!workspace || !devServer || devServer.status.status !== "running") return
    const port = devServer.status.port ?? devServer.port
    if (port === null) return
    if (localhostPort(nav.url) === port) return
    navigate(workspace, portUrl(port))
  }, [devServer, navigate, nav.url, workspace])

  const startCandidate = async (
    candidate: DevServerCandidate,
    port: number | null,
    attempt = nextAttempt()
  ) => {
    const attemptWorkspace = workspace
    if (!attemptWorkspace) return
    setLocalError(null)
    const startingInfo: DevServerInfo = {
      workspace: attemptWorkspace,
      command: candidate.command,
      port,
      status: { status: "starting" },
    }
    usePreviewStore.getState().setDevServer(startingInfo)
    try {
      const info = await devServerStart(attemptWorkspace, candidate.command, port, () => {})
      if (!isAttemptLive(attemptWorkspace, attempt)) {
        if (useWorkspaceStore.getState().workspacePath !== attemptWorkspace) {
          try {
            await devServerStop(attemptWorkspace)
          } catch {
            // Best effort cleanup for a process that started after its workspace was left.
          }
        }
        return
      }
      usePreviewStore.getState().setDevServer(info)
      setFlowState("idle")
      const runningPort =
        info.status.status === "running" ? info.status.port ?? info.port : info.port
      if (runningPort !== null)
        usePreviewStore.getState().navigate(attemptWorkspace, portUrl(runningPort))
    } catch (error) {
      if (!isAttemptLive(attemptWorkspace, attempt)) return
      const reason = error instanceof Error ? error.message : String(error)
      usePreviewStore.getState().setDevServer({
        workspace: attemptWorkspace,
        command: candidate.command,
        port,
        status: { status: "failed", reason },
      })
    }
  }

  const detectAndStart = async () => {
    const attemptWorkspace = workspace
    if (!attemptWorkspace || flowState === "detecting") return
    const attempt = nextAttempt()
    const settings = loadPreviewSettings()
    const commandOverride = settings.command.trim()
    const configuredPort = parsePortOverride(settings.port)
    setFlowState("detecting")
    setLocalError(null)
    setPortOverrideHint(configuredPort.hint)
    try {
      const extraPorts = configuredPort.port === null ? undefined : [configuredPort.port]
      const result = extraPorts
        ? await devServerDetect(attemptWorkspace, extraPorts)
        : await devServerDetect(attemptWorkspace)
      if (!isAttemptLive(attemptWorkspace, attempt)) return
      const detectedCandidate = result.candidates[0] ?? null
      const candidate =
        commandOverride.length > 0
          ? {
              scriptName: "settings",
              command: commandOverride,
              likelyPort: configuredPort.port ?? firstCandidatePort(detectedCandidate),
          }
          : detectedCandidate
      setCandidates(candidate ? [candidate, ...result.candidates.filter((c) => c.command !== candidate.command)] : [])
      const candidatePort = configuredPort.port ?? firstCandidatePort(candidate)
      const conflictingPort =
        candidatePort !== null && result.runningPorts.includes(candidatePort) ? candidatePort : null
      setRunningPorts(conflictingPort === null ? [] : [conflictingPort])
      const suggestedPort = candidatePort ?? result.runningPorts[0] ?? 5173
      setPortOverride(String(suggestedPort + (result.runningPorts.includes(suggestedPort) ? 1 : 0)))

      if (conflictingPort !== null) {
        setFlowState("occupied")
        return
      }
      if (!candidate) {
        setFlowState("no-candidates")
        return
      }
      await startCandidate(candidate, candidatePort, attempt)
      if (!isAttemptLive(attemptWorkspace, attempt)) return
    } catch (error) {
      if (!isAttemptLive(attemptWorkspace, attempt)) return
      setFlowState("idle")
      setLocalError(error instanceof Error ? error.message : String(error))
    }
  }

  const connectExisting = (port: number) => {
    if (!workspace) return
    usePreviewStore.getState().navigate(workspace, portUrl(port))
    setFlowState("idle")
  }

  const startOnOverridePort = () => {
    const nextPort = Number.parseInt(portOverride, 10)
    if (!primaryCandidate || !Number.isFinite(nextPort)) return
    void startCandidate(primaryCandidate, nextPort)
  }

  const stopServer = async () => {
    const attemptWorkspace = workspace
    const server = devServer
    if (!attemptWorkspace || !server) return
    const attempt = nextAttempt()
    await devServerStop(attemptWorkspace)
    if (!isAttemptLive(attemptWorkspace, attempt)) return
    usePreviewStore.getState().setDevServer({
      workspace: attemptWorkspace,
      command: server.command,
      port: server.port,
      status: { status: "exited", code: null },
    })
  }

  const body = (() => {
    if (nav.url && status?.status !== "exited" && status?.status !== "failed") {
      return (
        <div className="flex min-h-0 flex-1 justify-center bg-(--paper-1) p-[10px]">
          <div
            data-testid="preview-frame-shell"
            className="flex min-h-0 max-w-full flex-1 overflow-hidden rounded-[8px] border border-(--line-1) bg-(--paper-0)"
            style={{ width: frameWidth, flex: nav.frame === "mobile" ? "0 1 auto" : "1 1 auto" }}
          >
            <PreviewFrame url={nav.url} reloadNonce={nav.reloadNonce} />
          </div>
        </div>
      )
    }

    if (status?.status === "failed") {
      return (
        <div className="flex min-h-0 flex-1 items-center justify-center p-[18px]">
          <div className="flex max-w-[360px] flex-col items-center gap-[10px] text-center">
            <AlertTriangle className="size-[28px] text-[#b4232a]" aria-hidden="true" />
            <p className="text-[13px] font-medium text-(--ink-1)">{strings.preview.failedTitle}</p>
            <p className="text-[12.5px] text-(--ink-3)">{status.reason}</p>
            <div className="flex flex-wrap items-center justify-center gap-[8px]">
              <button
                type="button"
                onClick={() => void detectAndStart()}
                className="h-[26px] rounded-[7px] bg-(--ink-1) px-[10px] text-[12px] text-(--paper-0)"
              >
                {strings.preview.retryStart}
              </button>
              <button
                type="button"
                onClick={() => openSettings("logs", { source: "dev_server" })}
                className="flex h-[26px] items-center gap-[5px] rounded-[7px] border border-(--line-1) px-[10px] text-[12px] text-(--ink-2) hover:bg-(--paper-2)"
              >
                <FileText className="size-[12px]" aria-hidden="true" />
                檢視 logs
              </button>
            </div>
          </div>
        </div>
      )
    }

    if (flowState === "occupied") {
      return (
        <div className="flex min-h-0 flex-1 items-center justify-center p-[18px]">
          <div className="flex max-w-[420px] flex-col items-center gap-[10px] text-center">
            <Monitor className="size-[28px] text-(--ink-3)" aria-hidden="true" />
            <p className="text-[13px] font-medium text-(--ink-1)">{strings.preview.portOccupied}</p>
            <p className="text-[12.5px] text-(--ink-3)">
              port {runningPorts.join(", ")} 已有服務在執行，可以直接連接、改 port 後啟動，或重新偵測。
            </p>
            <div className="flex flex-wrap items-center justify-center gap-[8px]">
              {runningPorts.map((port) => (
                <button
                  key={port}
                  type="button"
                  onClick={() => connectExisting(port)}
                  className="h-[26px] rounded-[7px] bg-(--ink-1) px-[10px] text-[12px] text-(--paper-0)"
                >
                  {strings.preview.connectExisting} {port}
                </button>
              ))}
              <label className="flex items-center gap-[5px] text-[12px] text-(--ink-3)">
                {strings.preview.alternatePort}
                <input
                  type="number"
                  aria-label={strings.preview.alternatePort}
                  value={portOverride}
                  onChange={(event) => setPortOverride(event.currentTarget.value)}
                  className="h-[26px] w-[78px] rounded-[7px] border border-(--line-1) bg-(--yz-sunk) px-[7px] font-mono text-[12px] text-(--ink-2)"
                />
              </label>
              <button
                type="button"
                onClick={startOnOverridePort}
                className="h-[26px] rounded-[7px] border border-(--line-1) px-[10px] text-[12px] text-(--ink-2)"
              >
                {strings.preview.startChangedPort}
              </button>
              <button
                type="button"
                onClick={() => void detectAndStart()}
                className="h-[26px] rounded-[7px] border border-(--line-1) px-[10px] text-[12px] text-(--ink-2)"
              >
                {strings.preview.retryDetect}
              </button>
            </div>
          </div>
        </div>
      )
    }

    if (flowState === "no-candidates") {
      return (
        <div className="flex min-h-0 flex-1 items-center justify-center p-[18px]">
          <div className="flex max-w-[360px] flex-col items-center gap-[10px] text-center">
            <MonitorPlay className="size-[28px] text-(--ink-3)" aria-hidden="true" />
            <p className="text-[13px] font-medium text-(--ink-1)">{strings.preview.noCandidates}</p>
            <p className="text-[12.5px] text-(--ink-3)">{strings.preview.settingsHint}</p>
            <button
              type="button"
              onClick={() => void detectAndStart()}
              className="h-[26px] rounded-[7px] border border-(--line-1) px-[10px] text-[12px] text-(--ink-2)"
            >
              {strings.preview.retryDetect}
            </button>
          </div>
        </div>
      )
    }

    if (status?.status === "exited") {
      return (
        <div className="flex min-h-0 flex-1 items-center justify-center">
          <div className="flex flex-col items-center gap-[12px]">
            <EmptyState
              icon={MonitorPlay}
              title={strings.preview.exitedTitle}
              description={strings.preview.emptyDescription}
            />
            <button
              type="button"
              disabled={!workspace || flowState === "detecting"}
              onClick={() => void detectAndStart()}
              className="flex h-[28px] items-center gap-[6px] rounded-[7px] bg-(--ink-1) px-[10px] text-[12px] text-(--paper-0)"
            >
              <Play className="size-[13px]" aria-hidden="true" />
              {strings.preview.start}
            </button>
          </div>
        </div>
      )
    }

    return (
      <div className="flex min-h-0 flex-1 items-center justify-center">
        <div className="flex flex-col items-center gap-[12px]">
          <EmptyState
            icon={MonitorPlay}
            title={strings.preview.emptyTitle}
            description={localError ?? strings.preview.emptyDescription}
          />
          <button
            type="button"
            disabled={!workspace || flowState === "detecting"}
            onClick={() => void detectAndStart()}
            className={cn(
              "flex h-[28px] items-center gap-[6px] rounded-[7px] px-[10px] text-[12px]",
              workspace && flowState !== "detecting"
                ? "bg-(--ink-1) text-(--paper-0)"
                : "cursor-not-allowed bg-(--yz-sunk) text-(--ink-4)"
            )}
          >
            <Play className="size-[13px]" aria-hidden="true" />
            {flowState === "detecting" ? strings.preview.detecting : strings.preview.start}
          </button>
        </div>
      </div>
    )
  })()

  return (
    <div
      data-testid="preview-panel"
      onContextMenu={suppressContextMenu}
      className="flex min-h-0 flex-1 flex-col"
    >
      <div className="flex h-[38px] shrink-0 items-center gap-[6px] border-b border-(--line-1) px-[8px]">
        <button
          type="button"
          disabled={!canGoBack}
          aria-label="Back"
          onClick={() => workspace && usePreviewStore.getState().goBack(workspace)}
          className={cn(
            "flex size-[24px] shrink-0 items-center justify-center rounded-[7px]",
            canGoBack ? "text-(--ink-2) hover:bg-(--paper-2)" : "cursor-not-allowed text-(--ink-4)"
          )}
        >
          <ArrowLeft className="size-[14px]" aria-hidden="true" />
        </button>
        <button
          type="button"
          disabled={!canGoForward}
          aria-label="Forward"
          onClick={() => workspace && usePreviewStore.getState().goForward(workspace)}
          className={cn(
            "flex size-[24px] shrink-0 items-center justify-center rounded-[7px]",
            canGoForward ? "text-(--ink-2) hover:bg-(--paper-2)" : "cursor-not-allowed text-(--ink-4)"
          )}
        >
          <ArrowRight className="size-[14px]" aria-hidden="true" />
        </button>
        <button
          type="button"
          disabled={!canReload}
          aria-label="Reload"
          onClick={() => workspace && usePreviewStore.getState().reload(workspace)}
          className={cn(
            "flex size-[24px] shrink-0 items-center justify-center rounded-[7px]",
            canReload ? "text-(--ink-2) hover:bg-(--paper-2)" : "cursor-not-allowed text-(--ink-4)"
          )}
        >
          <RotateCw className="size-[13px]" aria-hidden="true" />
        </button>

        <input
          readOnly
          aria-label="Preview URL"
          value={nav.url ?? "http://localhost:—"}
          className="h-[24px] min-w-0 flex-1 rounded-[7px] border border-(--line-1) bg-(--yz-sunk) px-[8px] font-mono text-[11px] text-(--ink-3)"
        />

        <button
          type="button"
          disabled={!workspace}
          aria-label="Toggle responsive frame"
          onClick={() =>
            workspace &&
            usePreviewStore.getState().setFrame(workspace, nav.frame === "full" ? "mobile" : "full")
          }
          className={cn(
            "flex size-[24px] shrink-0 items-center justify-center rounded-[7px]",
            workspace ? "text-(--ink-2) hover:bg-(--paper-2)" : "cursor-not-allowed text-(--ink-4)"
          )}
        >
          {nav.frame === "mobile" ? (
            <Monitor className="size-[13px]" aria-hidden="true" />
          ) : (
            <Smartphone className="size-[13px]" aria-hidden="true" />
          )}
        </button>

        {status?.status === "running" && (
          <button
            type="button"
            onClick={() => void stopServer()}
            className="flex h-[24px] shrink-0 items-center gap-[5px] rounded-[7px] border border-(--line-1) px-[8px] text-[11px] text-(--ink-2) hover:bg-(--paper-2)"
          >
            <Square className="size-[11px]" aria-hidden="true" />
            Stop
          </button>
        )}

        <span
          className={cn(
            "shrink-0 rounded-[6px] border px-[7px] py-[3px] text-[10.5px]",
            statusTone(status)
          )}
        >
          {statusLabel(status)}
        </span>
      </div>

      {portOverrideHint ? (
        <div
          role="status"
          className="shrink-0 border-b border-[#ecd7a8] bg-[#fff7df] px-[10px] py-[5px] text-[11px] text-[#7a4b00]"
        >
          {portOverrideHint}
        </div>
      ) : null}

      {body}
    </div>
  )
}
