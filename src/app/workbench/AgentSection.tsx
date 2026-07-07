import { useRef, useState } from "react"

import { agentSetTrace } from "@/lib/ipc"
import { SettingCard, SettingsTextInput, ToggleRow } from "./settingsPrimitives"
import {
  AGENT_SETTINGS_STORAGE_KEY,
  DEFAULT_AGENT_COMMAND,
  loadAgentSettings,
  writeJsonSetting,
  type AgentPreset,
  type AgentSettings,
} from "./settingsStorage"

export function AgentSection() {
  const [settings, setSettings] = useState(loadAgentSettings)
  const traceGenRef = useRef(0)

  const persist = (next: AgentSettings) => {
    setSettings(next)
    writeJsonSetting(AGENT_SETTINGS_STORAGE_KEY, next)
  }

  const update = (patch: Partial<AgentSettings>) => {
    persist({ ...settings, ...patch })
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

  return (
    <div className="flex flex-col gap-[14px]">
      <SettingCard label="Agent 啟動" sub="ACP agent process command">
        <div className="flex flex-col gap-[12px]">
          <label className="flex flex-col gap-[6px]">
            <span className="text-[11.5px] font-medium text-(--ink-2)">Agent preset</span>
            <select
              aria-label="Agent preset"
              value={settings.preset}
              onChange={(event) => update({ preset: event.currentTarget.value as AgentPreset })}
              className="h-[30px] rounded-[8px] border border-(--line-1) bg-(--paper-0) px-[9px] text-[11.5px] text-(--ink-1) outline-none transition-colors focus:border-(--yz-accent)"
            >
              <option value="pi">pi · bunx pi-acp@0.0.31</option>
              <option value="custom">自訂</option>
            </select>
          </label>
          <SettingsTextInput
            label="自訂 command"
            value={settings.preset === "pi" ? DEFAULT_AGENT_COMMAND : settings.command}
            placeholder={DEFAULT_AGENT_COMMAND}
            disabled={settings.preset !== "custom"}
            onChange={(command) => update({ command })}
          />
        </div>
      </SettingCard>

      <div className="flex flex-col">
        <ToggleRow
          label="ACP trace"
          sub="將 ACP JSON-RPC 原始行寫入 debug log（最多 500 字）"
          checked={settings.traceEnabled}
          onCheckedChange={toggleTrace}
        />
      </div>
    </div>
  )
}
