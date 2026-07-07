import { useState } from "react"
import { RefreshCw } from "lucide-react"

import { cn } from "@/lib/utils"
import { useGitStore, type RemoteCheckMode } from "@/state/gitStore"
import { useWorkspaceStore } from "@/state/workspaceStore"
import { SettingCard } from "./settingsPrimitives"

// Remote-check modes for the Git pane — three-way aria-pressed segmented
// control (M1 Theme/Accent simplified radiogroup pattern). "probe" is default.
const REMOTE_CHECK_MODES: { id: RemoteCheckMode; label: string }[] = [
  { id: "off", label: "關閉" },
  { id: "probe", label: "唯讀檢查" },
  { id: "autofetch", label: "自動 fetch" },
]

/**
 * Git pane — detection state card + remote-check card. Reads the live git
 * environment / remote-check config from useGitStore (T11) and re-detects
 * against the current workspace path. Visual language extends SettingCard;
 * no upstream design for this pane.
 */
export function GitSection() {
  const environment = useGitStore((s) => s.environment)
  const remoteCheck = useGitStore((s) => s.remoteCheck)
  const setRemoteCheck = useGitStore((s) => s.setRemoteCheck)
  const detect = useGitStore((s) => s.detect)
  const workspacePath = useWorkspaceStore((s) => s.workspacePath)

  // Free-typing draft for the interval field: onChange no longer rejects
  // intermediate sub-minimum values (e.g. "4" on the way to "45"); clamping and
  // commit happen on blur (T19).
  const [intervalText, setIntervalText] = useState(String(remoteCheck.intervalSec))

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

  return (
    <div className="flex flex-col gap-[14px]">
      <SettingCard label="偵測狀態" sub="Git executable and repository root">
        {(!environment || environment.status === "missing") && (
          <div className="flex flex-col gap-[10px]">
            <span className="text-[12.5px] leading-[1.45] text-(--ink-2)">
              {environment?.status === "missing" ? environment.reason : "尚未偵測 Git"}
            </span>
            <div>
              <button
                type="button"
                onClick={redetect}
                className="flex h-[28px] items-center gap-[6px] rounded-[8px] bg-(--yz-solid) px-[11px] text-[11.5px] font-semibold text-(--ink-0) shadow-(--shadow-xs) transition-colors hover:bg-(--yz-hover)"
              >
                <RefreshCw className="size-[12px]" aria-hidden="true" />
                重新偵測
              </button>
            </div>
          </div>
        )}

        {environment?.status === "notARepo" && (
          <span className="text-[12.5px] leading-[1.45] text-(--ink-2)">
            目前的工作區不是 Git repository。
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

      <SettingCard label="遠端檢查" sub="How Yuzora looks for upstream changes">
        <div
          role="group"
          aria-label="遠端檢查"
          className="flex gap-[4px] rounded-[10px] bg-(--paper-2) p-[3px]"
        >
          {REMOTE_CHECK_MODES.map((option) => {
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
          <span className="text-[12px] text-(--ink-2)">檢查間隔</span>
          <span className="flex items-center gap-[6px]">
            <input
              type="number"
              min={30}
              value={intervalText}
              onChange={(e) => setIntervalText(e.target.value)}
              onBlur={commitInterval}
              className="h-[28px] w-[76px] rounded-[8px] border border-(--line-1) bg-(--paper-0) px-[9px] text-right font-mono text-[11.5px] text-(--ink-1) tabular-nums outline-none focus:border-(--yz-accent)"
            />
            <span className="text-[11px] text-(--ink-3)">秒</span>
          </span>
        </label>
      </SettingCard>
    </div>
  )
}
