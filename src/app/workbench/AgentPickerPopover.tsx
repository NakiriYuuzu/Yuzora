import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import {
  saveCustomAgentCommand,
  loadAgentSettings,
  resolveAgentCommandRoute,
} from "@/app/workbench/settingsStorage"
import {
  AGENT_PRESETS,
  AGENT_VISUALS,
  CUSTOM_AGENT_VISUAL,
  type AgentDescriptor,
  type AgentId,
} from "@/lib/agentPresets"
import { useAgentStore } from "@/state/agentStore"

type CardId = AgentId | "custom"
const CARD_ORDER: CardId[] = ["pi", "claude", "codex", "custom"]

/**
 * Nav "+ New session" popover — replaces the old native `<select>`. Three
 * brand-color preset cards plus a "custom command…" card that expands an
 * inline input (single-participant Settings-consistency flow: saves the
 * command as the global custom preset, then creates the session via the
 * agentId-omitted custom-command path). Interaction mirrors the SlashPopup /
 * InfoChip popovers in AgentZonePanel.tsx (absolute position, Esc/outside
 * click to close, arrow-key + Enter selection).
 */
export function AgentPickerPopover({
  cwd,
  onClose,
  dialogId,
}: {
  cwd: string
  onClose: () => void
  dialogId?: string
}) {
  const { t } = useTranslation("workbench")
  const newSession = useAgentStore((s) => s.newSession)
  const authRequired = useAgentStore((s) => s.authRequired)
  const rootRef = useRef<HTMLDivElement>(null)
  const inputRef = useRef<HTMLInputElement>(null)

  const settings = loadAgentSettings()
  const initialCard: CardId = settings.preset
  const [highlight, setHighlight] = useState<CardId>(initialCard)
  const [customExpanded, setCustomExpanded] = useState(initialCard === "custom")
  const [customValue, setCustomValue] = useState(() => (
    settings.preset === "custom" ? settings.command : ""
  ))

  function pickPreset(agentId: AgentId) {
    void newSession(cwd, agentId).catch(() => undefined)
    onClose()
  }

  function confirmCustom() {
    const command = customValue.trim()
    if (!command) return
    saveCustomAgentCommand(command)
    void newSession(cwd).catch(() => undefined)
    onClose()
  }

  useEffect(() => {
    rootRef.current?.focus()
  }, [])

  useEffect(() => {
    if (customExpanded) inputRef.current?.focus()
  }, [customExpanded])

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        if (customExpanded) {
          setCustomExpanded(false)
          queueMicrotask(() => rootRef.current?.focus())
          return
        }
        onClose()
        return
      }
      // Arrow/Enter navigation is card-list only — while the custom input is
      // focused, let the input own Enter (see its own onKeyDown) and default
      // caret movement for arrows.
      if (event.target !== rootRef.current) return
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault()
        const idx = CARD_ORDER.indexOf(highlight)
        const delta = event.key === "ArrowDown" ? 1 : -1
        setHighlight(CARD_ORDER[(idx + delta + CARD_ORDER.length) % CARD_ORDER.length])
        return
      }
      if (event.key === "Enter") {
        if (highlight === "custom") {
          if (!customExpanded) {
            event.preventDefault()
            setCustomExpanded(true)
          }
          return
        }
        event.preventDefault()
        pickPreset(highlight)
      }
    }
    function onMouseDown(event: MouseEvent) {
      if (rootRef.current && !rootRef.current.contains(event.target as Node)) onClose()
    }
    document.addEventListener("keydown", onKeyDown)
    document.addEventListener("mousedown", onMouseDown)
    return () => {
      document.removeEventListener("keydown", onKeyDown)
      document.removeEventListener("mousedown", onMouseDown)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [highlight, customExpanded, onClose])

  return (
    <div
      ref={rootRef}
      id={dialogId}
      data-testid="agent-picker-root"
      role="dialog"
      aria-label={t("agentNav.picker.ariaLabel")}
      tabIndex={-1}
      className="yz-pop absolute bottom-full left-0 z-20 mb-[8px] flex w-full flex-col gap-[4px] rounded-[14px] border border-(--line-2) bg-(--frost-light) p-[6px] shadow-(--shadow-xl) outline-none [backdrop-filter:var(--blur-frost)] focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-(--yz-accent)"
    >
      {AGENT_PRESETS.map((preset) => (
        <PresetCard
          key={preset.id}
          preset={preset}
          command={resolveAgentCommandRoute(preset.id, settings).command}
          highlighted={highlight === preset.id}
          needsLogin={authRequired?.agentId === preset.id}
          onMouseEnter={() => setHighlight(preset.id)}
          onFocus={() => setHighlight(preset.id)}
          onClick={() => pickPreset(preset.id)}
        />
      ))}
      <div
        data-testid="agent-picker-card-custom"
        data-highlighted={highlight === "custom"}
        onMouseEnter={() => setHighlight("custom")}
        className={
          "flex w-full flex-col gap-[6px] rounded-[10px] px-[9px] py-[8px] transition-colors " +
          (highlight === "custom" ? "bg-(--yz-active) shadow-[inset_0_0_0_1px_var(--line-1)]" : "hover:bg-(--yz-hover)")
        }
      >
        <button
          type="button"
          onFocus={() => setHighlight("custom")}
          onClick={() => {
            setHighlight("custom")
            setCustomExpanded(true)
          }}
          className="flex items-center gap-[9px] text-left"
        >
          <span
            aria-hidden="true"
            className="flex size-[18px] shrink-0 items-center justify-center rounded-[6px] text-[10px] font-bold text-(--agent-badge-ink)"
            style={{ background: CUSTOM_AGENT_VISUAL.colorVar }}
          >
            …
          </span>
          <span className="truncate text-[12.5px] font-medium text-(--ink-1)">
            {t("agentNav.picker.customLabel")}
          </span>
        </button>
        {customExpanded && (
          <div className="flex items-center gap-[6px] pl-[27px]">
            <input
              ref={inputRef}
              data-testid="agent-picker-custom-input"
              value={customValue}
              placeholder={t("agentNav.picker.customPlaceholder")}
              onChange={(event) => setCustomValue(event.currentTarget.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault()
                  confirmCustom()
                }
              }}
              className="h-[26px] min-w-0 flex-1 rounded-[7px] border border-(--line-1) bg-(--paper-0) px-[7px] font-mono text-[11px] text-(--ink-1) outline-none focus:border-(--yz-accent)"
            />
            <button
              type="button"
              data-testid="agent-picker-custom-confirm"
              onClick={confirmCustom}
              className="shrink-0 rounded-[7px] px-[8px] py-[5px] text-[10.5px] font-medium text-(--yz-accent-ink) hover:bg-(--yz-hover)"
            >
              {t("agentNav.picker.customConfirm")}
            </button>
          </div>
        )}
      </div>
    </div>
  )
}

function PresetCard({
  preset,
  command,
  highlighted,
  needsLogin,
  onMouseEnter,
  onFocus,
  onClick,
}: {
  preset: AgentDescriptor
  command: string
  highlighted: boolean
  needsLogin: boolean
  onMouseEnter: () => void
  onFocus: () => void
  onClick: () => void
}) {
  const { t } = useTranslation("workbench")
  const visual = AGENT_VISUALS[preset.id]
  return (
    <button
      type="button"
      data-highlighted={highlighted}
      data-testid={`agent-picker-card-${preset.id}`}
      onMouseEnter={onMouseEnter}
      onFocus={onFocus}
      onClick={onClick}
      className={
        "flex w-full items-center gap-[9px] rounded-[10px] px-[9px] py-[8px] text-left transition-colors " +
        (highlighted ? "bg-(--yz-active) shadow-[inset_0_0_0_1px_var(--line-1)]" : "hover:bg-(--yz-hover)")
      }
    >
      <span
        aria-hidden="true"
        className="flex size-[18px] shrink-0 items-center justify-center rounded-[6px] text-[10px] font-bold text-(--agent-badge-ink)"
        style={{ background: visual.colorVar }}
      >
        {visual.glyph}
      </span>
      <span className="flex min-w-0 flex-1 flex-col gap-px">
        <span className="truncate text-[12.5px] font-medium text-(--ink-1)">{preset.label}</span>
        <span className="truncate font-mono text-[9.5px] text-(--ink-4)">{command}</span>
      </span>
      {needsLogin && (
        <span
          data-testid={`agent-picker-needslogin-${preset.id}`}
          className="shrink-0 rounded-(--r-pill) bg-(--paper-2) px-[6px] py-[2px] text-[9px] font-semibold text-(--ink-3)"
        >
          {t("agentNav.picker.needsLogin")}
        </span>
      )}
    </button>
  )
}
