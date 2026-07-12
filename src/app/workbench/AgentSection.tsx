import { useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { agentSetTrace } from "@/lib/ipc"
import type { AgentCommandMode, AgentId } from "@/lib/agentPresets"
import { SettingCard, SettingsTextInput, ToggleRow } from "./settingsPrimitives"
import {
  AGENT_PRESETS,
  AGENT_SETTINGS_STORAGE_KEY,
  DEFAULT_AGENT_COMMAND,
  loadAgentSettings,
  resolveAgentCommandRoute,
  writeJsonSetting,
  type AgentPreset,
  type AgentSettings,
} from "./settingsStorage"

export function AgentSection() {
  const { t } = useTranslation("workbench")
  const [settings, setSettings] = useState(loadAgentSettings)
  const traceGenRef = useRef(0)

  const persist = (next: AgentSettings) => {
    setSettings(next)
    writeJsonSetting(AGENT_SETTINGS_STORAGE_KEY, next)
  }

  const update = (patch: Partial<AgentSettings>) => {
    persist({ ...settings, ...patch })
  }

  const updatePresetCommand = (agentId: AgentId, patch: Partial<AgentSettings["presetCommands"][AgentId]>) => {
    persist({
      ...settings,
      presetCommands: {
        ...settings.presetCommands,
        [agentId]: { ...settings.presetCommands[agentId], ...patch },
      },
    })
  }

  async function toggleTrace(next: boolean) {
    const prev = settings
    const gen = ++traceGenRef.current
    persist({ ...settings, traceEnabled: next })
    try {
      await agentSetTrace(next)
    } catch {
      if (gen === traceGenRef.current) persist(prev)
    }
  }

  const curatedPreset = settings.preset === "custom" ? null : settings.preset
  const effectiveCommand = resolveAgentCommandRoute(curatedPreset ?? undefined, settings).command
  const commandMode = curatedPreset ? settings.presetCommands[curatedPreset].mode : "custom"
  const commandEditable = settings.preset === "custom" || commandMode === "custom"

  return (
    <div className="flex flex-col gap-[14px]">
      <SettingCard label={t("settings.agentCommands.launchLabel")} sub={t("settings.agentCommands.launchSub")}>
        <div className="flex flex-col gap-[12px]">
          <label className="flex flex-col gap-[6px]">
            <span className="text-[11.5px] font-medium text-(--ink-2)">{t("settings.agentCommands.presetLabel")}</span>
            <select
              aria-label={t("settings.agentCommands.presetLabel")}
              value={settings.preset}
              onChange={(event) => {
                const preset = event.currentTarget.value as AgentPreset
                update({ preset })
              }}
              className="h-[30px] rounded-[8px] border border-(--line-1) bg-(--paper-0) px-[9px] text-[11.5px] text-(--ink-1) outline-none transition-colors focus:border-(--yz-accent)"
            >
              {AGENT_PRESETS.map((agent) => (
                <option key={agent.id} value={agent.id}>{agent.label}</option>
              ))}
              <option value="custom">{t("settings.agentCommands.topLevelCustom")}</option>
            </select>
          </label>
          {curatedPreset && (
            <label className="flex flex-col gap-[6px]">
              <span className="text-[11.5px] font-medium text-(--ink-2)">{t("settings.agentCommands.modeLabel")}</span>
              <select
                aria-label={t("settings.agentCommands.modeLabel")}
                value={commandMode}
                onChange={(event) => {
                  const mode = event.currentTarget.value as AgentCommandMode
                  updatePresetCommand(curatedPreset, {
                    mode,
                    ...(mode === "custom" && !settings.presetCommands[curatedPreset].customCommand
                      ? { customCommand: effectiveCommand }
                      : {}),
                  })
                }}
                className="h-[30px] rounded-[8px] border border-(--line-1) bg-(--paper-0) px-[9px] text-[11.5px] text-(--ink-1) outline-none transition-colors focus:border-(--yz-accent)"
              >
                <option value="verified">{t("settings.agentCommands.modeVerified")}</option>
                <option value="latest">{t("settings.agentCommands.modeLatest")}</option>
                <option value="custom">{t("settings.agentCommands.modeCustom")}</option>
              </select>
            </label>
          )}
          <SettingsTextInput
            label={t("settings.agentCommands.commandLabel")}
            value={settings.preset === "custom"
              ? settings.command
              : commandMode === "custom"
                ? settings.presetCommands[settings.preset].customCommand || effectiveCommand
                : effectiveCommand}
            placeholder={DEFAULT_AGENT_COMMAND}
            disabled={!commandEditable}
            onChange={(command) => {
              if (settings.preset === "custom") update({ command })
              else updatePresetCommand(settings.preset, { customCommand: command })
            }}
          />
        </div>
      </SettingCard>

      <div className="flex flex-col">
        <ToggleRow
          label={t("settings.agentCommands.traceLabel")}
          sub={t("settings.agentCommands.traceSub")}
          checked={settings.traceEnabled}
          onCheckedChange={toggleTrace}
        />
      </div>
    </div>
  )
}
